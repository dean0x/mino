//! Mino sandbox helper - macOS privileged helper binary
//!
//! This binary is installed to /usr/local/bin/ during `mino setup --native`
//! and is called via sudoers.d to perform privileged sandbox operations:
//! - Creating/deleting the sandbox system user
//! - Managing pf (packet filter) firewall rules
//! - Setting up filesystem jails
//!
//! TODO(Phase 4): Real implementation — ACL management, pf rules, process spawning as _mino_agent.

fn main() {
    eprintln!("mino-sandbox-helper: not yet implemented. Run `mino setup --native` first.");
    std::process::exit(1);
}
