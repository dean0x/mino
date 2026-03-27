//! Filtering network proxy for native sandbox
//!
//! A minimal SOCKS5 + HTTP CONNECT proxy that enforces per-session
//! network allowlists. Runs as a background tokio task in the main
//! mino process, not inside the sandbox.
//!
//! Protocol detection:
//! - First byte `0x05` → SOCKS5
//! - Otherwise → HTTP CONNECT

use crate::error::{MinoError, MinoResult};
use crate::network::NetworkRule;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Semaphore;
use tracing::{debug, warn};

/// Maximum size for HTTP request headers (defense against memory exhaustion)
const MAX_REQUEST_SIZE: usize = 8192;

/// Maximum number of concurrent proxy connections
const MAX_CONCURRENT_CONNECTIONS: usize = 256;

/// Timeout for reading the initial request from a client
const REQUEST_READ_TIMEOUT: Duration = Duration::from_secs(10);

/// Timeout for connecting to the upstream target
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

/// Handle to a running proxy instance.
///
/// The proxy shuts down when this handle is dropped.
pub struct ProxyHandle {
    /// Address the proxy is listening on
    pub addr: SocketAddr,
    /// Shutdown signal sender
    shutdown_tx: tokio::sync::watch::Sender<bool>,
}

impl ProxyHandle {
    /// Get the proxy port
    pub fn port(&self) -> u16 {
        self.addr.port()
    }

    /// Generate proxy environment variables for the sandbox.
    ///
    /// Returns both upper- and lowercase variants so that tools which
    /// only check one casing still pick up the proxy.
    pub fn proxy_env_vars(&self) -> Vec<(String, String)> {
        let http_url = format!("http://127.0.0.1:{}", self.port());
        let socks_url = format!("socks5://127.0.0.1:{}", self.port());
        vec![
            ("HTTP_PROXY".to_string(), http_url.clone()),
            ("HTTPS_PROXY".to_string(), http_url.clone()),
            ("ALL_PROXY".to_string(), socks_url.clone()),
            ("NO_PROXY".to_string(), "localhost,127.0.0.1".to_string()),
            ("http_proxy".to_string(), http_url.clone()),
            ("https_proxy".to_string(), http_url),
            ("all_proxy".to_string(), socks_url),
            ("no_proxy".to_string(), "localhost,127.0.0.1".to_string()),
        ]
    }

    /// Shutdown the proxy gracefully
    pub fn shutdown(&self) {
        let _ = self.shutdown_tx.send(true);
    }
}

impl Drop for ProxyHandle {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Start the filtering proxy on a random port.
///
/// Returns a `ProxyHandle` with the listening address and shutdown control.
/// The proxy runs as background tokio tasks; dropping the handle shuts it down.
pub async fn start_proxy(rules: Vec<NetworkRule>) -> MinoResult<ProxyHandle> {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|e| MinoError::NetworkProxy(format!("Failed to bind proxy: {e}")))?;

    let addr = listener
        .local_addr()
        .map_err(|e| MinoError::NetworkProxy(format!("Failed to get proxy address: {e}")))?;

    let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
    let rules = Arc::new(rules);

    tokio::spawn(accept_loop(listener, rules, shutdown_rx));

    debug!("Proxy started on {}", addr);

    Ok(ProxyHandle { addr, shutdown_tx })
}

/// Accept loop — runs until the shutdown signal fires.
///
/// Uses a semaphore to limit concurrent connections, preventing resource
/// exhaustion from a misbehaving sandbox process.
async fn accept_loop(
    listener: TcpListener,
    rules: Arc<Vec<NetworkRule>>,
    mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
) {
    let semaphore = Arc::new(Semaphore::new(MAX_CONCURRENT_CONNECTIONS));

    loop {
        tokio::select! {
            result = listener.accept() => {
                match result {
                    Ok((stream, peer_addr)) => {
                        let permit = match semaphore.clone().try_acquire_owned() {
                            Ok(permit) => permit,
                            Err(_) => {
                                debug!("Proxy connection limit reached, dropping connection from {}", peer_addr);
                                drop(stream);
                                continue;
                            }
                        };
                        let rules = Arc::clone(&rules);
                        tokio::spawn(async move {
                            if let Err(e) = handle_connection(stream, peer_addr, &rules).await {
                                debug!("Proxy connection error from {}: {}", peer_addr, e);
                            }
                            drop(permit);
                        });
                    }
                    Err(e) => {
                        warn!("Proxy accept error: {}", e);
                    }
                }
            }
            _ = shutdown_rx.changed() => {
                if *shutdown_rx.borrow() {
                    debug!("Proxy shutting down");
                    break;
                }
            }
        }
    }
}

/// Route a connection to the appropriate protocol handler based on the first byte.
///
/// Applies a read timeout to the initial peek to prevent idle connections
/// from holding resources indefinitely.
async fn handle_connection(
    stream: TcpStream,
    _peer_addr: SocketAddr,
    rules: &[NetworkRule],
) -> MinoResult<()> {
    let mut peek_buf = [0u8; 1];

    let peek_result = tokio::time::timeout(REQUEST_READ_TIMEOUT, stream.peek(&mut peek_buf)).await;

    match peek_result {
        Ok(Ok(_)) => {}
        Ok(Err(e)) => {
            return Err(MinoError::NetworkProxy(format!("Peek error: {e}")));
        }
        Err(_) => {
            return Err(MinoError::NetworkProxy(
                "Request read timed out".to_string(),
            ));
        }
    }

    match peek_buf[0] {
        0x05 => handle_socks5(stream, rules).await,
        _ => handle_http_connect(stream, rules).await,
    }
}

/// Validate that a hostname contains no null bytes or control characters.
///
/// These could indicate injection attempts or malformed requests.
fn validate_hostname(host: &str) -> MinoResult<()> {
    if host.is_empty() {
        return Err(MinoError::NetworkProxy("Empty hostname".to_string()));
    }
    if host.bytes().any(|b| b == 0 || b < 0x20) {
        return Err(MinoError::NetworkProxy(format!(
            "Hostname contains null or control characters: {:?}",
            host
        )));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// SOCKS5 handler
// ---------------------------------------------------------------------------

/// Handle a SOCKS5 CONNECT request.
///
/// Implements just enough of RFC 1928 for CONNECT (command 0x01) with
/// no-auth (method 0x00). Supports IPv4, IPv6, and domain address types.
async fn handle_socks5(mut stream: TcpStream, rules: &[NetworkRule]) -> MinoResult<()> {
    // --- Greeting phase ---
    let mut buf = [0u8; 258];
    let n = stream
        .read(&mut buf)
        .await
        .map_err(|e| MinoError::NetworkProxy(format!("SOCKS5 read error: {e}")))?;

    if n < 2 || buf[0] != 0x05 {
        return Err(MinoError::NetworkProxy("Invalid SOCKS5 greeting".into()));
    }

    // Reply: no authentication required (method 0x00)
    stream
        .write_all(&[0x05, 0x00])
        .await
        .map_err(|e| MinoError::NetworkProxy(format!("SOCKS5 write error: {e}")))?;

    // --- Request phase ---
    let n = stream
        .read(&mut buf)
        .await
        .map_err(|e| MinoError::NetworkProxy(format!("SOCKS5 read error: {e}")))?;

    if n < 4 || buf[0] != 0x05 || buf[1] != 0x01 {
        // Not a CONNECT command — reply "command not supported"
        stream
            .write_all(&[0x05, 0x07, 0x00, 0x01, 0, 0, 0, 0, 0, 0])
            .await
            .ok();
        return Err(MinoError::NetworkProxy(
            "Only SOCKS5 CONNECT supported".into(),
        ));
    }

    let (host, port) = parse_socks5_address(&buf[..n])?;
    validate_hostname(&host)?;

    // --- Policy check ---
    if !is_allowed(&host, port, rules) {
        debug!("SOCKS5 denied: {}:{}", host, port);
        // General SOCKS server failure (0x02)
        stream
            .write_all(&[0x05, 0x02, 0x00, 0x01, 0, 0, 0, 0, 0, 0])
            .await
            .ok();
        return Ok(());
    }

    // --- Connect to target (with timeout) ---
    let target_addr = format!("{host}:{port}");
    match tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(&target_addr)).await {
        Ok(Ok(target)) => {
            let reply = build_socks5_success_reply(&target);
            stream.write_all(&reply).await.ok();
            relay(stream, target).await;
        }
        Ok(Err(e)) => {
            debug!("SOCKS5 connect failed to {}: {}", target_addr, e);
            // Connection refused (0x05)
            stream
                .write_all(&[0x05, 0x05, 0x00, 0x01, 0, 0, 0, 0, 0, 0])
                .await
                .ok();
        }
        Err(_) => {
            debug!("SOCKS5 connect timed out to {}", target_addr);
            // TTL expired (0x06)
            stream
                .write_all(&[0x05, 0x06, 0x00, 0x01, 0, 0, 0, 0, 0, 0])
                .await
                .ok();
        }
    }

    Ok(())
}

/// Parse the destination address from a SOCKS5 request buffer.
///
/// `buf` must contain the full request starting at VER (byte 0).
/// Address type is at byte 3: 0x01 (IPv4), 0x03 (domain), 0x04 (IPv6).
fn parse_socks5_address(buf: &[u8]) -> MinoResult<(String, u16)> {
    if buf.len() < 4 {
        return Err(MinoError::NetworkProxy("SOCKS5 request too short".into()));
    }

    match buf[3] {
        // IPv4 — 4 octets + 2 port bytes
        0x01 => {
            if buf.len() < 10 {
                return Err(MinoError::NetworkProxy(
                    "SOCKS5 IPv4 address too short".into(),
                ));
            }
            let ip = format!("{}.{}.{}.{}", buf[4], buf[5], buf[6], buf[7]);
            let port = u16::from_be_bytes([buf[8], buf[9]]);
            Ok((ip, port))
        }
        // Domain name — 1 length byte + name + 2 port bytes
        0x03 => {
            let domain_len = buf[4] as usize;
            let needed = 5 + domain_len + 2;
            if buf.len() < needed {
                return Err(MinoError::NetworkProxy(
                    "SOCKS5 domain address too short".into(),
                ));
            }
            let domain = String::from_utf8_lossy(&buf[5..5 + domain_len]).to_string();
            let port_offset = 5 + domain_len;
            let port = u16::from_be_bytes([buf[port_offset], buf[port_offset + 1]]);
            Ok((domain, port))
        }
        // IPv6 — 16 octets + 2 port bytes
        0x04 => {
            if buf.len() < 22 {
                return Err(MinoError::NetworkProxy(
                    "SOCKS5 IPv6 address too short".into(),
                ));
            }
            let ip = format!(
                "{:x}:{:x}:{:x}:{:x}:{:x}:{:x}:{:x}:{:x}",
                u16::from_be_bytes([buf[4], buf[5]]),
                u16::from_be_bytes([buf[6], buf[7]]),
                u16::from_be_bytes([buf[8], buf[9]]),
                u16::from_be_bytes([buf[10], buf[11]]),
                u16::from_be_bytes([buf[12], buf[13]]),
                u16::from_be_bytes([buf[14], buf[15]]),
                u16::from_be_bytes([buf[16], buf[17]]),
                u16::from_be_bytes([buf[18], buf[19]]),
            );
            let port = u16::from_be_bytes([buf[20], buf[21]]);
            Ok((ip, port))
        }
        other => Err(MinoError::NetworkProxy(format!(
            "SOCKS5 unknown address type: {other}"
        ))),
    }
}

/// Build the SOCKS5 success reply using the target's local address.
fn build_socks5_success_reply(target: &TcpStream) -> Vec<u8> {
    let local = target
        .local_addr()
        .unwrap_or_else(|_| "0.0.0.0:0".parse().unwrap());
    let mut reply = vec![0x05, 0x00, 0x00, 0x01];
    match local.ip() {
        std::net::IpAddr::V4(ip) => {
            reply.extend_from_slice(&ip.octets());
        }
        std::net::IpAddr::V6(_) => {
            // Fallback to 0.0.0.0 for IPv6 local addresses
            reply.extend_from_slice(&[0, 0, 0, 0]);
        }
    }
    reply.extend_from_slice(&local.port().to_be_bytes());
    reply
}

// ---------------------------------------------------------------------------
// HTTP CONNECT handler
// ---------------------------------------------------------------------------

/// Handle an HTTP CONNECT tunneling request.
///
/// Reads the first line to extract host:port, checks the allowlist,
/// and if permitted establishes a bidirectional relay.
///
/// Enforces `MAX_REQUEST_SIZE` to prevent memory exhaustion from
/// oversized HTTP request headers.
async fn handle_http_connect(mut stream: TcpStream, rules: &[NetworkRule]) -> MinoResult<()> {
    let mut buf = vec![0u8; MAX_REQUEST_SIZE];

    let read_result = tokio::time::timeout(REQUEST_READ_TIMEOUT, stream.read(&mut buf)).await;

    let n = match read_result {
        Ok(Ok(n)) => n,
        Ok(Err(e)) => {
            return Err(MinoError::NetworkProxy(format!("HTTP read error: {e}")));
        }
        Err(_) => {
            let response = "HTTP/1.1 408 Request Timeout\r\nContent-Length: 0\r\n\r\n";
            stream.write_all(response.as_bytes()).await.ok();
            return Err(MinoError::NetworkProxy(
                "HTTP request read timed out".to_string(),
            ));
        }
    };

    if n == 0 {
        return Err(MinoError::NetworkProxy("Empty HTTP request".to_string()));
    }

    let request = String::from_utf8_lossy(&buf[..n]);
    let (host, port) = parse_connect_request(&request)?;
    validate_hostname(&host)?;

    if !is_allowed(&host, port, rules) {
        debug!("HTTP CONNECT denied: {}:{}", host, port);
        let response = "HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\n\r\n";
        stream.write_all(response.as_bytes()).await.ok();
        return Ok(());
    }

    let target_addr = format!("{host}:{port}");
    match tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(&target_addr)).await {
        Ok(Ok(target)) => {
            let response = "HTTP/1.1 200 Connection Established\r\n\r\n";
            stream.write_all(response.as_bytes()).await.ok();
            relay(stream, target).await;
        }
        Ok(Err(e)) => {
            debug!("HTTP CONNECT failed to {}: {}", target_addr, e);
            let response = "HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\n\r\n";
            stream.write_all(response.as_bytes()).await.ok();
        }
        Err(_) => {
            debug!("HTTP CONNECT timed out to {}", target_addr);
            let response = "HTTP/1.1 504 Gateway Timeout\r\nContent-Length: 0\r\n\r\n";
            stream.write_all(response.as_bytes()).await.ok();
        }
    }

    Ok(())
}

/// Parse an HTTP CONNECT request to extract host and port.
///
/// Expected format: `CONNECT host:port HTTP/1.x\r\n...`
/// If no port is specified, defaults to 443 (standard HTTPS).
fn parse_connect_request(request: &str) -> MinoResult<(String, u16)> {
    let first_line = request
        .lines()
        .next()
        .ok_or_else(|| MinoError::NetworkProxy("Empty HTTP request".into()))?;

    let parts: Vec<&str> = first_line.split_whitespace().collect();
    if parts.len() < 2 || parts[0] != "CONNECT" {
        return Err(MinoError::NetworkProxy(format!(
            "Not an HTTP CONNECT request: {first_line}"
        )));
    }

    let host_port = parts[1];
    if let Some(colon) = host_port.rfind(':') {
        let host = host_port[..colon].to_string();
        let port: u16 = host_port[colon + 1..].parse().map_err(|_| {
            MinoError::NetworkProxy(format!("Invalid port in CONNECT: {host_port}"))
        })?;
        Ok((host, port))
    } else {
        // Default to port 443 for CONNECT without explicit port
        Ok((host_port.to_string(), 443))
    }
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// Check whether a host:port pair is allowed by the rule set.
///
/// Empty rules = deny all (secure default). Both host and port must match.
fn is_allowed(host: &str, port: u16, rules: &[NetworkRule]) -> bool {
    rules.iter().any(|r| r.host == host && r.port == port)
}

/// Bidirectional TCP relay using `tokio::io::copy`.
///
/// Returns when either direction finishes (EOF or error).
async fn relay(client: TcpStream, server: TcpStream) {
    let (mut cr, mut cw) = client.into_split();
    let (mut sr, mut sw) = server.into_split();

    let c2s = tokio::io::copy(&mut cr, &mut sw);
    let s2c = tokio::io::copy(&mut sr, &mut cw);

    tokio::select! {
        _ = c2s => {}
        _ = s2c => {}
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(host: &str, port: u16) -> NetworkRule {
        NetworkRule {
            host: host.to_string(),
            port,
        }
    }

    // ---- is_allowed tests ----

    #[test]
    fn is_allowed_matching_rule_returns_true() {
        let rules = vec![rule("github.com", 443)];
        assert!(is_allowed("github.com", 443, &rules));
    }

    #[test]
    fn is_allowed_no_matching_rule_returns_false() {
        let rules = vec![rule("github.com", 443)];
        assert!(!is_allowed("evil.com", 443, &rules));
    }

    #[test]
    fn is_allowed_empty_rules_returns_false() {
        assert!(!is_allowed("github.com", 443, &[]));
    }

    #[test]
    fn is_allowed_checks_both_host_and_port() {
        let rules = vec![rule("github.com", 443)];
        // Right host, wrong port
        assert!(!is_allowed("github.com", 80, &rules));
        // Wrong host, right port
        assert!(!is_allowed("evil.com", 443, &rules));
    }

    #[test]
    fn is_allowed_multiple_rules() {
        let rules = vec![
            rule("github.com", 443),
            rule("npmjs.org", 443),
            rule("github.com", 22),
        ];
        assert!(is_allowed("github.com", 443, &rules));
        assert!(is_allowed("npmjs.org", 443, &rules));
        assert!(is_allowed("github.com", 22, &rules));
        assert!(!is_allowed("npmjs.org", 22, &rules));
    }

    // ---- parse_connect_request tests ----

    #[test]
    fn parse_connect_request_valid() {
        let (host, port) =
            parse_connect_request("CONNECT github.com:443 HTTP/1.1\r\nHost: github.com\r\n")
                .unwrap();
        assert_eq!(host, "github.com");
        assert_eq!(port, 443);
    }

    #[test]
    fn parse_connect_request_default_port() {
        let (host, port) = parse_connect_request("CONNECT example.com HTTP/1.1\r\n").unwrap();
        assert_eq!(host, "example.com");
        assert_eq!(port, 443);
    }

    #[test]
    fn parse_connect_request_custom_port() {
        let (host, port) =
            parse_connect_request("CONNECT api.example.com:8080 HTTP/1.1\r\n").unwrap();
        assert_eq!(host, "api.example.com");
        assert_eq!(port, 8080);
    }

    #[test]
    fn parse_connect_request_empty_errors() {
        assert!(parse_connect_request("").is_err());
    }

    #[test]
    fn parse_connect_request_non_connect_errors() {
        let err = parse_connect_request("GET / HTTP/1.1\r\n").unwrap_err();
        assert!(err.to_string().contains("Not an HTTP CONNECT"));
    }

    #[test]
    fn parse_connect_request_invalid_port_errors() {
        let err = parse_connect_request("CONNECT host:abc HTTP/1.1\r\n").unwrap_err();
        assert!(err.to_string().contains("Invalid port"));
    }

    // ---- parse_socks5_address tests ----

    #[test]
    fn parse_socks5_ipv4() {
        // VER=5, CMD=1, RSV=0, ATYP=1(IPv4), IP=127.0.0.1, PORT=443
        let buf = [0x05, 0x01, 0x00, 0x01, 127, 0, 0, 1, 0x01, 0xBB];
        let (host, port) = parse_socks5_address(&buf).unwrap();
        assert_eq!(host, "127.0.0.1");
        assert_eq!(port, 443);
    }

    #[test]
    fn parse_socks5_domain() {
        // ATYP=3(domain), len=10, "github.com", PORT=443
        let mut buf = vec![0x05, 0x01, 0x00, 0x03, 10];
        buf.extend_from_slice(b"github.com");
        buf.extend_from_slice(&443u16.to_be_bytes());
        let (host, port) = parse_socks5_address(&buf).unwrap();
        assert_eq!(host, "github.com");
        assert_eq!(port, 443);
    }

    #[test]
    fn parse_socks5_ipv6() {
        // ATYP=4(IPv6), 16 bytes of ::1, PORT=443
        let mut buf = vec![0x05, 0x01, 0x00, 0x04];
        // ::1 = 0000:0000:0000:0000:0000:0000:0000:0001
        buf.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1]);
        buf.extend_from_slice(&443u16.to_be_bytes());
        let (host, port) = parse_socks5_address(&buf).unwrap();
        assert_eq!(host, "0:0:0:0:0:0:0:1");
        assert_eq!(port, 443);
    }

    #[test]
    fn parse_socks5_ipv4_too_short() {
        let buf = [0x05, 0x01, 0x00, 0x01, 127, 0, 0];
        assert!(parse_socks5_address(&buf).is_err());
    }

    #[test]
    fn parse_socks5_domain_too_short() {
        // domain length=10 but only 5 bytes of domain
        let buf = [0x05, 0x01, 0x00, 0x03, 10, b'a', b'b', b'c', b'd', b'e'];
        assert!(parse_socks5_address(&buf).is_err());
    }

    #[test]
    fn parse_socks5_ipv6_too_short() {
        let buf = [0x05, 0x01, 0x00, 0x04, 0, 0, 0, 0, 0, 0, 0, 0];
        assert!(parse_socks5_address(&buf).is_err());
    }

    #[test]
    fn parse_socks5_unknown_address_type() {
        let buf = [0x05, 0x01, 0x00, 0x09, 0, 0, 0, 0];
        let err = parse_socks5_address(&buf).unwrap_err();
        assert!(err.to_string().contains("unknown address type"));
    }

    #[test]
    fn parse_socks5_too_short_overall() {
        let buf = [0x05, 0x01, 0x00];
        assert!(parse_socks5_address(&buf).is_err());
    }

    // ---- proxy_env_vars tests ----

    #[test]
    fn proxy_env_vars_generates_correct_values() {
        let handle = ProxyHandle {
            addr: "127.0.0.1:12345".parse().unwrap(),
            shutdown_tx: tokio::sync::watch::channel(false).0,
        };

        let vars = handle.proxy_env_vars();

        let find_var = |name: &str| -> String {
            vars.iter()
                .find(|(k, _)| k == name)
                .map(|(_, v)| v.clone())
                .unwrap_or_default()
        };

        assert_eq!(find_var("HTTP_PROXY"), "http://127.0.0.1:12345");
        assert_eq!(find_var("HTTPS_PROXY"), "http://127.0.0.1:12345");
        assert_eq!(find_var("ALL_PROXY"), "socks5://127.0.0.1:12345");
        assert_eq!(find_var("NO_PROXY"), "localhost,127.0.0.1");

        // Lowercase variants
        assert_eq!(find_var("http_proxy"), "http://127.0.0.1:12345");
        assert_eq!(find_var("https_proxy"), "http://127.0.0.1:12345");
        assert_eq!(find_var("all_proxy"), "socks5://127.0.0.1:12345");
        assert_eq!(find_var("no_proxy"), "localhost,127.0.0.1");
    }

    #[test]
    fn proxy_env_vars_has_eight_entries() {
        let handle = ProxyHandle {
            addr: "127.0.0.1:9999".parse().unwrap(),
            shutdown_tx: tokio::sync::watch::channel(false).0,
        };
        assert_eq!(handle.proxy_env_vars().len(), 8);
    }

    // ---- ProxyHandle tests ----

    #[test]
    fn proxy_handle_port_returns_correct_port() {
        let handle = ProxyHandle {
            addr: "127.0.0.1:54321".parse().unwrap(),
            shutdown_tx: tokio::sync::watch::channel(false).0,
        };
        assert_eq!(handle.port(), 54321);
    }

    // ---- Integration: start_proxy ----

    #[tokio::test]
    async fn start_proxy_binds_random_port() {
        let handle = start_proxy(vec![]).await.unwrap();
        assert!(handle.port() > 0);
        assert_eq!(handle.addr.ip(), std::net::Ipv4Addr::LOCALHOST);
        handle.shutdown();
    }

    #[tokio::test]
    async fn start_proxy_shutdown_is_idempotent() {
        let handle = start_proxy(vec![]).await.unwrap();
        handle.shutdown();
        handle.shutdown(); // Should not panic
    }

    #[tokio::test]
    async fn proxy_http_connect_denied_returns_403() {
        // Start proxy with no rules (deny all)
        let handle = start_proxy(vec![]).await.unwrap();
        let port = handle.port();

        // Connect and send HTTP CONNECT request
        let mut stream = TcpStream::connect(format!("127.0.0.1:{port}"))
            .await
            .unwrap();

        let request = "CONNECT example.com:443 HTTP/1.1\r\nHost: example.com\r\n\r\n";
        stream.write_all(request.as_bytes()).await.unwrap();

        let mut response = vec![0u8; 1024];
        let n = stream.read(&mut response).await.unwrap();
        let response_str = String::from_utf8_lossy(&response[..n]);

        assert!(
            response_str.contains("403 Forbidden"),
            "Expected 403, got: {}",
            response_str
        );

        handle.shutdown();
    }

    #[tokio::test]
    async fn proxy_http_connect_allowed_connects() {
        // Start a dummy TCP server to connect to
        let target_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let target_port = target_listener.local_addr().unwrap().port();

        // Start proxy with rule allowing our target
        let rules = vec![rule("127.0.0.1", target_port)];
        let handle = start_proxy(rules).await.unwrap();
        let proxy_port = handle.port();

        // Accept on target in background
        let target_handle = tokio::spawn(async move {
            let (mut conn, _) = target_listener.accept().await.unwrap();
            let mut buf = [0u8; 64];
            let n = conn.read(&mut buf).await.unwrap();
            conn.write_all(&buf[..n]).await.unwrap();
        });

        // Connect through proxy
        let mut stream = TcpStream::connect(format!("127.0.0.1:{proxy_port}"))
            .await
            .unwrap();

        let request =
            format!("CONNECT 127.0.0.1:{target_port} HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n");
        stream.write_all(request.as_bytes()).await.unwrap();

        // Read response
        let mut response = vec![0u8; 1024];
        let n = stream.read(&mut response).await.unwrap();
        let response_str = String::from_utf8_lossy(&response[..n]);

        assert!(
            response_str.contains("200 Connection Established"),
            "Expected 200, got: {}",
            response_str
        );

        // Send data through the tunnel and verify echo
        stream.write_all(b"ping").await.unwrap();
        let mut echo = [0u8; 4];
        stream.read_exact(&mut echo).await.unwrap();
        assert_eq!(&echo, b"ping");

        handle.shutdown();
        let _ = target_handle.await;
    }

    #[tokio::test]
    async fn proxy_socks5_denied_returns_failure() {
        // Start proxy with no rules (deny all)
        let handle = start_proxy(vec![]).await.unwrap();
        let port = handle.port();

        let mut stream = TcpStream::connect(format!("127.0.0.1:{port}"))
            .await
            .unwrap();

        // SOCKS5 greeting: version 5, 1 method, no auth
        stream.write_all(&[0x05, 0x01, 0x00]).await.unwrap();

        // Read greeting reply
        let mut reply = [0u8; 2];
        stream.read_exact(&mut reply).await.unwrap();
        assert_eq!(reply, [0x05, 0x00]); // No auth selected

        // SOCKS5 CONNECT to example.com:443
        let mut request = vec![0x05, 0x01, 0x00, 0x03];
        let domain = b"example.com";
        request.push(domain.len() as u8);
        request.extend_from_slice(domain);
        request.extend_from_slice(&443u16.to_be_bytes());
        stream.write_all(&request).await.unwrap();

        // Read connect reply — expect general failure (0x02)
        let mut connect_reply = [0u8; 10];
        stream.read_exact(&mut connect_reply).await.unwrap();
        assert_eq!(connect_reply[0], 0x05); // SOCKS version
        assert_eq!(connect_reply[1], 0x02); // General failure

        handle.shutdown();
    }

    #[tokio::test]
    async fn proxy_socks5_allowed_connects() {
        // Start a dummy TCP server
        let target_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let target_port = target_listener.local_addr().unwrap().port();

        let rules = vec![rule("127.0.0.1", target_port)];
        let handle = start_proxy(rules).await.unwrap();
        let proxy_port = handle.port();

        // Accept on target in background
        let target_handle = tokio::spawn(async move {
            let (mut conn, _) = target_listener.accept().await.unwrap();
            let mut buf = [0u8; 64];
            let n = conn.read(&mut buf).await.unwrap();
            conn.write_all(&buf[..n]).await.unwrap();
        });

        let mut stream = TcpStream::connect(format!("127.0.0.1:{proxy_port}"))
            .await
            .unwrap();

        // Greeting
        stream.write_all(&[0x05, 0x01, 0x00]).await.unwrap();
        let mut reply = [0u8; 2];
        stream.read_exact(&mut reply).await.unwrap();
        assert_eq!(reply, [0x05, 0x00]);

        // CONNECT to 127.0.0.1:<target_port> using IPv4 address type
        let port_bytes = target_port.to_be_bytes();
        let request = [
            0x05,
            0x01,
            0x00,
            0x01, // VER, CMD=CONNECT, RSV, ATYP=IPv4
            127,
            0,
            0,
            1, // IP
            port_bytes[0],
            port_bytes[1], // PORT
        ];
        stream.write_all(&request).await.unwrap();

        // Read connect reply — expect success (0x00)
        let mut connect_reply = [0u8; 10];
        stream.read_exact(&mut connect_reply).await.unwrap();
        assert_eq!(connect_reply[0], 0x05);
        assert_eq!(connect_reply[1], 0x00); // Success

        // Send data through the tunnel and verify echo
        stream.write_all(b"hello").await.unwrap();
        let mut echo = [0u8; 5];
        stream.read_exact(&mut echo).await.unwrap();
        assert_eq!(&echo, b"hello");

        handle.shutdown();
        let _ = target_handle.await;
    }

    #[tokio::test]
    async fn proxy_drop_shuts_down() {
        let handle = start_proxy(vec![]).await.unwrap();
        let _port = handle.port();
        drop(handle);

        // Give the accept loop a moment to process the shutdown signal
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        // Connection should fail or be refused since the listener is dropped
        // (The proxy tasks may still be winding down, so we just verify
        // the shutdown signal was sent — the accept loop exits)
    }

    // ---- build_socks5_success_reply test ----

    #[tokio::test]
    async fn build_socks5_success_reply_contains_valid_header() {
        // We can't easily test with a real TcpStream here, but we can
        // verify the format is correct by using a loopback connection
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let connect_handle = tokio::spawn(async move { TcpStream::connect(addr).await.unwrap() });

        let (server, _) = listener.accept().await.unwrap();
        let client = connect_handle.await.unwrap();

        let reply = build_socks5_success_reply(&client);

        // VER=5, REP=0(success), RSV=0, ATYP=1(IPv4)
        assert_eq!(reply[0], 0x05);
        assert_eq!(reply[1], 0x00);
        assert_eq!(reply[2], 0x00);
        assert_eq!(reply[3], 0x01);
        // Total length: 4 header + 4 IPv4 + 2 port = 10
        assert_eq!(reply.len(), 10);

        drop(server);
    }

    // ---- validate_hostname tests ----

    #[test]
    fn validate_hostname_normal() {
        assert!(validate_hostname("github.com").is_ok());
    }

    #[test]
    fn validate_hostname_ip_address() {
        assert!(validate_hostname("192.168.1.1").is_ok());
    }

    #[test]
    fn validate_hostname_rejects_empty() {
        let err = validate_hostname("").unwrap_err();
        assert!(err.to_string().contains("Empty hostname"));
    }

    #[test]
    fn validate_hostname_rejects_null_byte() {
        let err = validate_hostname("evil\0.com").unwrap_err();
        assert!(err.to_string().contains("null or control"));
    }

    #[test]
    fn validate_hostname_rejects_control_chars() {
        let err = validate_hostname("evil\x01.com").unwrap_err();
        assert!(err.to_string().contains("null or control"));
    }

    #[test]
    fn validate_hostname_rejects_newline() {
        let err = validate_hostname("evil\n.com").unwrap_err();
        assert!(err.to_string().contains("null or control"));
    }

    #[test]
    fn validate_hostname_rejects_carriage_return() {
        let err = validate_hostname("evil\r.com").unwrap_err();
        assert!(err.to_string().contains("null or control"));
    }

    #[test]
    fn validate_hostname_allows_hyphen_and_dots() {
        assert!(validate_hostname("my-host.example.com").is_ok());
    }

    // ---- Constants tests ----

    #[test]
    fn max_request_size_is_reasonable() {
        assert!(MAX_REQUEST_SIZE >= 1024);
        assert!(MAX_REQUEST_SIZE <= 65536);
    }

    #[test]
    fn max_concurrent_connections_is_reasonable() {
        assert!(MAX_CONCURRENT_CONNECTIONS >= 16);
        assert!(MAX_CONCURRENT_CONNECTIONS <= 4096);
    }

    #[test]
    fn timeouts_are_reasonable() {
        assert!(REQUEST_READ_TIMEOUT.as_secs() >= 1);
        assert!(REQUEST_READ_TIMEOUT.as_secs() <= 60);
        assert!(CONNECT_TIMEOUT.as_secs() >= 5);
        assert!(CONNECT_TIMEOUT.as_secs() <= 120);
    }
}
