//! Local config trust gate
//!
//! Prevents untrusted `.mino.toml` files (e.g. committed to a cloned repo)
//! from silently overriding security-sensitive container settings like
//! volume mounts, network mode, credentials, and image selection.
//!
//! Trust is keyed by the canonical file path + SHA-256 of the file content.
//! Any mutation re-triggers the prompt.

use crate::error::{MinoError, MinoResult};
use crate::ui::{self, UiContext};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tokio::fs;
use tracing::{debug, warn};

use super::ConfigManager;

/// Container keys considered security-sensitive for trust gating.
/// Any local config setting one of these requires explicit user approval.
const SENSITIVE_CONTAINER_KEYS: &[&str] = &[
    "volumes",
    "env",
    "network",
    "network_allow",
    "network_preset",
    "image",
    "layers",
    "workdir",
];

/// VM keys considered security-sensitive for trust gating.
/// On macOS, these control which OrbStack VM commands execute inside.
const SENSITIVE_VM_KEYS: &[&str] = &["name", "distro"];

/// A single trust entry keyed by file content hash.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct TrustEntry {
    content_hash: String,
    trusted_at: String,
}

/// Persisted map of canonical paths to trust entries.
#[derive(Debug, Default, Serialize, Deserialize)]
struct TrustStore {
    entries: HashMap<PathBuf, TrustEntry>,
}

impl TrustStore {
    fn path() -> PathBuf {
        ConfigManager::state_dir().join("trusted_configs.json")
    }

    async fn load() -> Self {
        let path = Self::path();
        let bytes = match fs::read(&path).await {
            Ok(b) => b,
            Err(_) => return Self::default(),
        };
        match serde_json::from_slice(&bytes) {
            Ok(store) => store,
            Err(e) => {
                warn!(
                    "Corrupt trust store at {}, treating as empty: {}",
                    path.display(),
                    e
                );
                Self::default()
            }
        }
    }

    async fn save(&self) -> MinoResult<()> {
        let path = Self::path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).await.map_err(|e| {
                MinoError::io(
                    format!("creating trust store directory {}", parent.display()),
                    e,
                )
            })?;
        }
        let json = serde_json::to_string_pretty(self)?;
        fs::write(&path, json)
            .await
            .map_err(|e| MinoError::io(format!("writing trust store to {}", path.display()), e))?;
        debug!("Trust store saved to {}", path.display());
        Ok(())
    }

    fn is_trusted(&self, canonical_path: &Path, content_hash: &str) -> bool {
        self.entries
            .get(canonical_path)
            .is_some_and(|entry| entry.content_hash == content_hash)
    }

    fn add(&mut self, canonical_path: PathBuf, content_hash: String) {
        self.entries.insert(
            canonical_path,
            TrustEntry {
                content_hash,
                trusted_at: chrono::Utc::now().to_rfc3339(),
            },
        );
    }
}

/// Result of analyzing a TOML value for security-sensitive keys.
#[derive(Debug)]
pub struct SensitiveAnalysis {
    pub fields: Vec<String>,
}

impl SensitiveAnalysis {
    pub fn has_sensitive(&self) -> bool {
        !self.fields.is_empty()
    }
}

/// SHA-256 hex digest of raw bytes.
pub fn hash_content(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

/// Sections with key-level sensitivity checks.
const SENSITIVE_SECTIONS: &[(&str, &[&str])] = &[
    ("container", SENSITIVE_CONTAINER_KEYS),
    ("vm", SENSITIVE_VM_KEYS),
];

/// Walk the parsed TOML value and check for sensitive key paths.
pub fn analyze_sensitive_fields(value: &toml::Value) -> SensitiveAnalysis {
    let Some(table) = value.as_table() else {
        return SensitiveAnalysis { fields: vec![] };
    };

    let mut fields = Vec::new();

    for (section, keys) in SENSITIVE_SECTIONS {
        if let Some(sub) = table.get(*section).and_then(|v| v.as_table()) {
            for key in *keys {
                if sub.contains_key(*key) {
                    fields.push(format!("{section}.{key}"));
                }
            }
        }
    }

    if table.contains_key("credentials") {
        fields.push("credentials".to_string());
    }

    SensitiveAnalysis { fields }
}

/// Render a human-readable summary of the sensitive values found.
fn format_sensitive_summary(value: &toml::Value, fields: &[String]) -> String {
    let Some(table) = value.as_table() else {
        return String::new();
    };

    let mut lines = Vec::new();

    for field in fields {
        if field == "credentials" {
            if let Some(creds) = table.get("credentials") {
                lines.push(format!("[credentials] = {}", summarize_value(creds)));
            }
            continue;
        }

        // Handle section.key fields (e.g. "container.network", "vm.name")
        if let Some((section, key)) = field.split_once('.') {
            if let Some(val) = table
                .get(section)
                .and_then(|v| v.as_table())
                .and_then(|t| t.get(key))
            {
                lines.push(format!("{section}.{key} = {}", summarize_value(val)));
            }
        }
    }

    lines.join("\n")
}

/// Concise display of a TOML value (truncate large tables/arrays).
fn summarize_value(val: &toml::Value) -> String {
    match val {
        toml::Value::String(s) => format!("\"{s}\""),
        toml::Value::Integer(i) => i.to_string(),
        toml::Value::Float(f) => f.to_string(),
        toml::Value::Boolean(b) => b.to_string(),
        toml::Value::Array(arr) => {
            let items: Vec<String> = arr.iter().take(5).map(summarize_value).collect();
            if arr.len() > 5 {
                format!("[{}, ... +{} more]", items.join(", "), arr.len() - 5)
            } else {
                format!("[{}]", items.join(", "))
            }
        }
        toml::Value::Table(t) => {
            let preview: String = t.keys().take(5).cloned().collect::<Vec<_>>().join(", ");
            if t.len() > 5 {
                format!("{{ {preview}, ... +{} more }}", t.len() - 5)
            } else {
                format!("{{ {preview} }}")
            }
        }
        toml::Value::Datetime(dt) => dt.to_string(),
    }
}

/// Verify a local config file before it is merged into the config.
///
/// Returns `Some(path)` if the config should be loaded, `None` if it should be skipped.
///
/// - Benign configs (no sensitive keys) pass through silently.
/// - `trust_override` (`--trust-local` / `MINO_TRUST_LOCAL`) bypasses the gate.
/// - Already-trusted configs (matching content hash) pass silently.
/// - Interactive terminals get a confirmation prompt.
/// - Non-interactive environments skip untrusted sensitive configs with a warning.
pub async fn verify_local_config(
    path: &Path,
    ctx: &UiContext,
    trust_override: bool,
) -> MinoResult<Option<PathBuf>> {
    // Read raw content
    let raw = fs::read(path)
        .await
        .map_err(|e| MinoError::io(format!("reading local config {}", path.display()), e))?;

    // Parse as generic TOML value — if parse fails, return Some(path) and let
    // load_merged() handle the error with its existing ConfigInvalid path.
    let value: toml::Value = match toml::from_str(&String::from_utf8_lossy(&raw)) {
        Ok(v) => v,
        Err(e) => {
            debug!("Local config parse failed (will be caught by load_merged): {e}");
            return Ok(Some(path.to_path_buf()));
        }
    };

    // Analyze for sensitive fields
    let analysis = analyze_sensitive_fields(&value);
    if !analysis.has_sensitive() {
        debug!("Local config is benign (no sensitive fields), loading without trust check");
        return Ok(Some(path.to_path_buf()));
    }

    // Explicit trust override bypasses the gate
    if trust_override {
        warn!(
            "Loading untrusted local config {} with sensitive fields (--trust-local): [{}]",
            path.display(),
            analysis.fields.join(", ")
        );
        return Ok(Some(path.to_path_buf()));
    }

    // Canonicalize path for consistent trust store keying
    let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let content_hash = hash_content(&raw);

    // Check trust store
    let mut store = TrustStore::load().await;
    if store.is_trusted(&canonical, &content_hash) {
        debug!(
            "Local config {} is trusted (hash match)",
            canonical.display()
        );
        return Ok(Some(path.to_path_buf()));
    }

    // Interactive prompt
    if ctx.is_interactive() {
        ui::step_warn(ctx, &format!("Untrusted local config: {}", path.display()));

        let summary = format_sensitive_summary(&value, &analysis.fields);
        ui::note(ctx, "Security-sensitive fields detected", &summary);

        let trusted = ui::confirm(ctx, "Trust this config and continue?", false).await?;

        if trusted {
            store.add(canonical, content_hash);
            store.save().await?;
            return Ok(Some(path.to_path_buf()));
        }

        ui::step_warn_hint(
            ctx,
            "Local config skipped",
            "Use --no-local to always skip, or --trust-local to always trust",
        );
        return Ok(None);
    }

    // Non-interactive: reject with warning
    ui::step_warn_hint(
        ctx,
        &format!(
            "Skipping untrusted local config {} (non-interactive, sensitive fields: [{}])",
            path.display(),
            analysis.fields.join(", ")
        ),
        "Use --trust-local or MINO_TRUST_LOCAL=1 to trust",
    );
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_empty_config_is_benign() {
        let value: toml::Value = toml::from_str("").unwrap();
        let analysis = analyze_sensitive_fields(&value);
        assert!(!analysis.has_sensitive());
        assert!(analysis.fields.is_empty());
    }

    #[test]
    fn test_session_only_is_benign() {
        let value: toml::Value = toml::from_str(
            r#"
            [session]
            shell = "zsh"
            "#,
        )
        .unwrap();
        let analysis = analyze_sensitive_fields(&value);
        assert!(!analysis.has_sensitive());
    }

    #[test]
    fn test_container_network_is_sensitive() {
        let value: toml::Value = toml::from_str(
            r#"
            [container]
            network = "host"
            "#,
        )
        .unwrap();
        let analysis = analyze_sensitive_fields(&value);
        assert!(analysis.has_sensitive());
        assert!(analysis.fields.contains(&"container.network".to_string()));
    }

    #[test]
    fn test_container_volumes_is_sensitive() {
        let value: toml::Value = toml::from_str(
            r#"
            [container]
            volumes = ["/etc/shadow:/steal:ro"]
            "#,
        )
        .unwrap();
        let analysis = analyze_sensitive_fields(&value);
        assert!(analysis.has_sensitive());
        assert!(analysis.fields.contains(&"container.volumes".to_string()));
    }

    #[test]
    fn test_credentials_is_sensitive() {
        let value: toml::Value = toml::from_str(
            r#"
            [credentials.aws]
            enabled = true
            region = "us-west-2"
            "#,
        )
        .unwrap();
        let analysis = analyze_sensitive_fields(&value);
        assert!(analysis.has_sensitive());
        assert!(analysis.fields.contains(&"credentials".to_string()));
    }

    #[test]
    fn test_multiple_sensitive_fields() {
        let value: toml::Value = toml::from_str(
            r#"
            [container]
            network = "host"
            volumes = ["/etc:/etc:ro"]
            image = "evil:latest"

            [vm]
            name = "attacker-vm"

            [credentials.aws]
            enabled = true
            "#,
        )
        .unwrap();
        let analysis = analyze_sensitive_fields(&value);
        assert!(analysis.has_sensitive());
        assert!(analysis.fields.contains(&"container.network".to_string()));
        assert!(analysis.fields.contains(&"container.volumes".to_string()));
        assert!(analysis.fields.contains(&"container.image".to_string()));
        assert!(analysis.fields.contains(&"vm.name".to_string()));
        assert!(analysis.fields.contains(&"credentials".to_string()));
        assert_eq!(analysis.fields.len(), 5);
    }

    #[test]
    fn test_hash_content_deterministic() {
        let data = b"[container]\nnetwork = \"host\"\n";
        let h1 = hash_content(data);
        let h2 = hash_content(data);
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 64); // SHA-256 hex = 64 chars
    }

    #[tokio::test]
    async fn test_trust_store_roundtrip() {
        let temp = TempDir::new().unwrap();
        let store_path = temp.path().join("trusted_configs.json");

        // Manually set the store path by writing/reading directly
        let mut store = TrustStore::default();
        let test_path = PathBuf::from("/tmp/test/.mino.toml");
        let test_hash = "abc123def456".to_string();
        store.add(test_path.clone(), test_hash.clone());

        let json = serde_json::to_string_pretty(&store).unwrap();
        std::fs::write(&store_path, &json).unwrap();

        let loaded: TrustStore =
            serde_json::from_str(&std::fs::read_to_string(&store_path).unwrap()).unwrap();
        assert!(loaded.is_trusted(&test_path, &test_hash));
    }

    #[test]
    fn test_is_trusted_matches_hash() {
        let mut store = TrustStore::default();
        let path = PathBuf::from("/project/.mino.toml");
        let hash = hash_content(b"[container]\nimage = \"typescript\"\n");
        store.add(path.clone(), hash.clone());
        assert!(store.is_trusted(&path, &hash));
    }

    #[test]
    fn test_is_trusted_rejects_changed_hash() {
        let mut store = TrustStore::default();
        let path = PathBuf::from("/project/.mino.toml");
        let hash = hash_content(b"[container]\nimage = \"typescript\"\n");
        store.add(path.clone(), hash);

        let new_hash = hash_content(b"[container]\nimage = \"evil\"\n");
        assert!(!store.is_trusted(&path, &new_hash));
    }

    #[test]
    fn test_trust_store_corrupt_returns_empty() {
        // Simulate loading corrupt JSON — TrustStore::load reads from disk,
        // so we test the deserialization path directly.
        let corrupt = b"not valid json {{{";
        let result: Result<TrustStore, _> = serde_json::from_slice(corrupt);
        assert!(result.is_err());
        // The load() method returns Default on error, which is empty
        let fallback = TrustStore::default();
        assert!(fallback.entries.is_empty());
    }

    #[tokio::test]
    async fn test_verify_benign_returns_some() {
        let temp = TempDir::new().unwrap();
        let config_path = temp.path().join(".mino.toml");
        std::fs::write(
            &config_path,
            r#"
            [session]
            shell = "zsh"
            "#,
        )
        .unwrap();

        let ctx = UiContext::non_interactive();
        let result = verify_local_config(&config_path, &ctx, false)
            .await
            .unwrap();
        assert!(result.is_some());
    }

    #[tokio::test]
    async fn test_verify_sensitive_non_interactive_returns_none() {
        let temp = TempDir::new().unwrap();
        let config_path = temp.path().join(".mino.toml");
        std::fs::write(
            &config_path,
            r#"
            [container]
            network = "host"
            "#,
        )
        .unwrap();

        let ctx = UiContext::non_interactive();
        let result = verify_local_config(&config_path, &ctx, false)
            .await
            .unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_verify_sensitive_with_override_returns_some() {
        let temp = TempDir::new().unwrap();
        let config_path = temp.path().join(".mino.toml");
        std::fs::write(
            &config_path,
            r#"
            [container]
            network = "host"
            volumes = ["/etc:/etc:ro"]
            "#,
        )
        .unwrap();

        let ctx = UiContext::non_interactive();
        let result = verify_local_config(&config_path, &ctx, true).await.unwrap();
        assert!(result.is_some());
    }

    #[test]
    fn test_container_workdir_is_sensitive() {
        let value: toml::Value = toml::from_str(
            r#"
            [container]
            workdir = "/app"
            "#,
        )
        .unwrap();
        let analysis = analyze_sensitive_fields(&value);
        assert!(analysis.has_sensitive());
        assert!(analysis.fields.contains(&"container.workdir".to_string()));
    }

    #[test]
    fn test_vm_name_is_sensitive() {
        let value: toml::Value = toml::from_str(
            r#"
            [vm]
            name = "evil"
            "#,
        )
        .unwrap();
        let analysis = analyze_sensitive_fields(&value);
        assert!(analysis.has_sensitive());
        assert!(analysis.fields.contains(&"vm.name".to_string()));
    }

    #[test]
    fn test_vm_distro_is_sensitive() {
        let value: toml::Value = toml::from_str(
            r#"
            [vm]
            distro = "alpine"
            "#,
        )
        .unwrap();
        let analysis = analyze_sensitive_fields(&value);
        assert!(analysis.has_sensitive());
        assert!(analysis.fields.contains(&"vm.distro".to_string()));
    }

    #[test]
    fn test_vm_only_is_sensitive() {
        let value: toml::Value = toml::from_str(
            r#"
            [vm]
            name = "x"
            "#,
        )
        .unwrap();
        let analysis = analyze_sensitive_fields(&value);
        assert!(analysis.has_sensitive());
        assert_eq!(analysis.fields.len(), 1);
        assert_eq!(analysis.fields[0], "vm.name");
    }

    #[test]
    fn test_workdir_with_other_benign_is_sensitive() {
        let value: toml::Value = toml::from_str(
            r#"
            [container]
            workdir = "/"

            [session]
            shell = "zsh"
            "#,
        )
        .unwrap();
        let analysis = analyze_sensitive_fields(&value);
        assert!(analysis.has_sensitive());
        assert!(analysis.fields.contains(&"container.workdir".to_string()));
        assert_eq!(analysis.fields.len(), 1);
    }

    #[test]
    fn test_format_summary_includes_vm_fields() {
        let value: toml::Value = toml::from_str(
            r#"
            [container]
            image = "evil:latest"

            [vm]
            name = "attacker"
            distro = "alpine"
            "#,
        )
        .unwrap();
        let analysis = analyze_sensitive_fields(&value);
        let summary = format_sensitive_summary(&value, &analysis.fields);
        assert!(summary.contains("container.image = \"evil:latest\""));
        assert!(summary.contains("vm.name = \"attacker\""));
        assert!(summary.contains("vm.distro = \"alpine\""));
    }
}
