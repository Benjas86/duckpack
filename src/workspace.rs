use anyhow::{Context, Result};
use duckdb::Connection;
use std::path::{Path, PathBuf};

use crate::engine::{
    format_quack_endpoint, load_env_files, load_quack_extension, open_duckdb_connection,
    redact_connection_credentials, resolve_target_config, should_disable_quack_ssl,
};

/// Forbidden plaintext credential keys in `[[secrets]]` and `[[attach]]` inside `duckpack.toml`.
const FORBIDDEN_SECRET_KEYS: &[(&str, &str)] = &[
    ("token", "token_env"),
    ("admin_token", "admin_token_env"),
    ("bearer_token", "token_env"),
    ("access_token", "token_env"),
    ("session_token", "token_env"),
    ("sas_token", "token_env"),
    ("secret", "secret_env"),
    ("secret_key", "secret_env"),
    ("secret_access_key", "secret_env"),
    ("client_secret", "secret_env"),
    ("account_key", "secret_env"),
    ("api_key", "secret_env"),
    ("password", "password_env"),
    ("pwd", "password_env"),
    ("key_id", "key_id_env"),
    ("access_key_id", "key_id_env"),
    ("dsn", "dsn_env"),
    ("connection_string", "dsn_env"),
];

/// Declarative secret entry from `[[secrets]]` or `[[env.<profile>.secrets]]` in `duckpack.toml`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceSecret {
    pub name: String,
    pub secret_type: String,
    pub scope: Option<String>,
    pub key_id_env: Option<String>,
    pub secret_env: Option<String>,
    pub token_env: Option<String>,
    pub region: Option<String>,
    pub region_env: Option<String>,
    pub endpoint: Option<String>,
    pub endpoint_env: Option<String>,
    pub url_style: Option<String>,
    pub use_ssl: Option<bool>,
    pub bucket: Option<String>,
    pub enabled: bool,
}

impl Default for WorkspaceSecret {
    fn default() -> Self {
        Self {
            name: String::new(),
            secret_type: "s3".to_string(),
            scope: None,
            key_id_env: None,
            secret_env: None,
            token_env: None,
            region: None,
            region_env: None,
            endpoint: None,
            endpoint_env: None,
            url_style: None,
            use_ssl: None,
            bucket: None,
            enabled: true,
        }
    }
}

/// Declarative database / DuckLake / Quack attachment from `[[attach]]` or `[[env.<profile>.attach]]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceAttachment {
    pub alias: String,
    pub attach_type: String,
    pub backend: Option<String>,
    pub metadata: Option<String>,
    pub path: Option<String>,
    pub host: Option<String>,
    pub port: Option<u16>,
    pub database: Option<String>,
    pub username: Option<String>,
    pub username_env: Option<String>,
    pub password_env: Option<String>,
    pub token_env: Option<String>,
    pub admin_token_env: Option<String>,
    pub dsn_env: Option<String>,
    pub client_id: Option<String>,
    pub data_path: Option<String>,
    pub read_only: bool,
    pub insecure: bool,
    pub disable_ssl: bool,
    pub trusted_connection: bool,
    pub default: bool,
    pub enabled: bool,
}

impl Default for WorkspaceAttachment {
    fn default() -> Self {
        Self {
            alias: String::new(),
            attach_type: "duckdb".to_string(),
            backend: None,
            metadata: None,
            path: None,
            host: None,
            port: None,
            database: None,
            username: None,
            username_env: None,
            password_env: None,
            token_env: None,
            admin_token_env: None,
            dsn_env: None,
            client_id: None,
            data_path: None,
            read_only: false,
            insecure: false,
            disable_ssl: false,
            trusted_connection: false,
            default: false,
            enabled: true,
        }
    }
}

/// Merged declarative workspace configuration for a given project directory & environment profile.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorkspaceConfig {
    pub secrets: Vec<WorkspaceSecret>,
    pub attachments: Vec<WorkspaceAttachment>,
}

/// Categorizes why a declarative `[[attach]]` entry failed to mount at startup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttachmentIssueKind {
    MissingEnv(Vec<String>),
    ConnectionFailed(String),
}

/// Represents a declarative `[[attach]]` entry that could not be attached (offline or missing env).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FailedAttachment {
    pub alias: String,
    pub attach_type: String,
    pub kind: AttachmentIssueKind,
    pub entry: WorkspaceAttachment,
}

impl FailedAttachment {
    pub fn explorer_display(&self) -> String {
        match &self.kind {
            AttachmentIssueKind::MissingEnv(vars) => {
                format!("🔒 {} (missing {})", self.alias, vars.join(", "))
            }
            AttachmentIssueKind::ConnectionFailed(_) => {
                format!("⚠️ {} (offline — Enter/r to retry)", self.alias)
            }
        }
    }

    pub fn detail_message(&self) -> String {
        match &self.kind {
            AttachmentIssueKind::MissingEnv(vars) => format!(
                "Attachment '{}' skipped: missing env var(s) {} in .env or environment.",
                self.alias,
                vars.join(", ")
            ),
            AttachmentIssueKind::ConnectionFailed(err) => {
                format!("Attachment '{}' offline/failed: {}", self.alias, err)
            }
        }
    }
}

/// Summary returned after bootstrapping a workspace's `[[secrets]]` and `[[attach]]` declarations.
#[derive(Debug, Clone, Default)]
pub struct WorkspaceApplyReport {
    pub loaded_secrets: Vec<String>,
    pub secret_warnings: Vec<String>,
    pub attached_aliases: Vec<String>,
    pub failed_attachments: Vec<FailedAttachment>,
    pub default_alias: Option<String>,
}

impl WorkspaceApplyReport {
    pub fn status_summary(&self) -> Option<String> {
        let mut parts = Vec::new();
        if !self.attached_aliases.is_empty() {
            parts.push(format!(
                "Mounted {} workspace catalog(s)",
                self.attached_aliases.len()
            ));
        }
        if !self.failed_attachments.is_empty() {
            let locked = self
                .failed_attachments
                .iter()
                .filter(|f| matches!(f.kind, AttachmentIssueKind::MissingEnv(_)))
                .count();
            let offline = self.failed_attachments.len() - locked;
            let mut fail_bits = Vec::new();
            if offline > 0 {
                fail_bits.push(format!("{} offline", offline));
            }
            if locked > 0 {
                fail_bits.push(format!("{} missing env", locked));
            }
            parts.push(format!("⚠️ {} skipped ({})", self.failed_attachments.len(), fail_bits.join(", ")));
        }
        if !self.secret_warnings.is_empty() {
            parts.push(format!(
                "{} secret warning(s)",
                self.secret_warnings.len()
            ));
        }
        if parts.is_empty() {
            None
        } else {
            Some(parts.join(" | "))
        }
    }
}

/// Target Quack control server for `Ctrl+U` (either directly attached or a DuckLake's Quack metadata server).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuackControlTarget {
    pub label: String,
    pub attached_alias: Option<String>,
    pub quack_endpoint: Option<String>,
    pub token_env: Option<String>,
    pub client_id: Option<String>,
    pub disable_ssl: bool,
}

impl QuackControlTarget {
    /// Executes `op(conn, alias)` against this Quack control target, reusing `base_conn` when
    /// already attached or transparently opening a transient control connection when backed by
    /// an unattached Quack endpoint (e.g., DuckLake-on-Quack metadata server).
    pub fn with_connection<T, F>(
        &self,
        base_conn: &Connection,
        project_dir: &Path,
        env_profile: Option<&str>,
        op: F,
    ) -> Result<T>
    where
        F: FnOnce(&Connection, &str) -> Result<T>,
    {
        if let Some(ref alias) = self.attached_alias {
            return op(base_conn, alias);
        }
        if self.quack_endpoint.is_some() {
            let (ctrl_conn, ctrl_alias) =
                open_quack_control_connection(self, project_dir, env_profile)?;
            return op(&ctrl_conn, &ctrl_alias);
        }
        anyhow::bail!(
            "Quack control target '{}' has neither an attached alias nor a Quack endpoint.",
            self.label
        )
    }
}

fn validate_no_raw_secret_keys(tbl: &toml::value::Table, section_label: &str) -> Result<()> {
    for (bad_key, suggested_key) in FORBIDDEN_SECRET_KEYS {
        if tbl.contains_key(*bad_key) {
            anyhow::bail!(
                "Secret Hygiene Error in {}: raw '{}' literals are not allowed in duckpack.toml. Store the credential in .env and reference it via '{} = \"YOUR_ENV_VAR\"'.",
                section_label,
                bad_key,
                suggested_key
            );
        }
    }
    // Reject inline credentials embedded in string fields (including scheme://user:pass@ URIs) (M5)
    static URI_CRED_RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let uri_cred_re = URI_CRED_RE
        .get_or_init(|| regex::Regex::new(r"[a-zA-Z0-9+.-]+://[^:/@\s]+:[^@\s]+@").unwrap());
    for field in ["metadata", "path", "host", "data_path", "database", "endpoint", "scope"] {
        if let Some(val) = tbl.get(field).and_then(|v| v.as_str()) {
            let lower = val.to_lowercase();
            if lower.contains("password=")
                || lower.contains("pwd=")
                || lower.contains("token=")
                || lower.contains("secret=")
                || uri_cred_re.is_match(val)
            {
                anyhow::bail!(
                    "Secret Hygiene Error in {}.{}: inline credentials are not allowed in duckpack.toml. Store credentials in .env and use *_env fields.",
                    section_label,
                    field
                );
            }
        }
    }
    Ok(())
}

fn parse_secret_entry(val: &toml::Value, section_label: &str) -> Result<WorkspaceSecret> {
    let tbl = val
        .as_table()
        .ok_or_else(|| anyhow::anyhow!("{} entry must be a TOML table.", section_label))?;
    validate_no_raw_secret_keys(tbl, section_label)?;

    let name = tbl
        .get("name")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow::anyhow!("{} entry is missing required 'name' field.", section_label))?;

    if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        anyhow::bail!(
            "Invalid secret name '{}' in {}: only ASCII letters, digits, and underscores are allowed.",
            name,
            section_label
        );
    }

    let secret_type = tbl
        .get("type")
        .or_else(|| tbl.get("provider"))
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_lowercase())
        .unwrap_or_else(|| "s3".to_string());

    let get_str = |k: &str| -> Option<String> {
        tbl.get(k)
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    };

    Ok(WorkspaceSecret {
        name,
        secret_type,
        scope: get_str("scope"),
        key_id_env: get_str("key_id_env").or_else(|| get_str("access_key_env")),
        secret_env: get_str("secret_env").or_else(|| get_str("secret_key_env")),
        token_env: get_str("token_env"),
        region: get_str("region"),
        region_env: get_str("region_env"),
        endpoint: get_str("endpoint"),
        endpoint_env: get_str("endpoint_env"),
        url_style: get_str("url_style"),
        use_ssl: tbl.get("use_ssl").and_then(|v| v.as_bool()),
        bucket: get_str("bucket"),
        enabled: tbl.get("enabled").and_then(|v| v.as_bool()).unwrap_or(true),
    })
}

fn parse_attachment_entry(val: &toml::Value, section_label: &str) -> Result<WorkspaceAttachment> {
    let tbl = val
        .as_table()
        .ok_or_else(|| anyhow::anyhow!("{} entry must be a TOML table.", section_label))?;
    validate_no_raw_secret_keys(tbl, section_label)?;

    let alias = tbl
        .get("alias")
        .or_else(|| tbl.get("name"))
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow::anyhow!("{} entry is missing required 'alias' field.", section_label))?;

    if !alias
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        anyhow::bail!(
            "Invalid attachment alias '{}' in {}: only ASCII letters, digits, underscores, and hyphens are allowed.",
            alias,
            section_label
        );
    }

    let attach_type = tbl
        .get("type")
        .or_else(|| tbl.get("provider"))
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_lowercase())
        .unwrap_or_else(|| "duckdb".to_string());

    let get_str = |k: &str| -> Option<String> {
        tbl.get(k)
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
    };

    let port = tbl
        .get("port")
        .and_then(|v| v.as_integer())
        .and_then(|p| u16::try_from(p).ok());

    Ok(WorkspaceAttachment {
        alias,
        attach_type,
        backend: get_str("backend").map(|s| s.to_lowercase()),
        metadata: get_str("metadata").or_else(|| get_str("catalog")),
        path: get_str("path").or_else(|| get_str("file")),
        host: get_str("host").or_else(|| get_str("server")),
        port,
        database: get_str("database").or_else(|| get_str("dbname")),
        username: get_str("username").or_else(|| get_str("user")),
        username_env: get_str("username_env").or_else(|| get_str("user_env")),
        password_env: get_str("password_env"),
        token_env: get_str("token_env"),
        admin_token_env: get_str("admin_token_env"),
        dsn_env: get_str("dsn_env"),
        client_id: get_str("client_id"),
        data_path: get_str("data_path").or_else(|| get_str("storage")),
        read_only: tbl.get("read_only").and_then(|v| v.as_bool()).unwrap_or(false),
        insecure: tbl.get("insecure").and_then(|v| v.as_bool()).unwrap_or(false),
        disable_ssl: tbl
            .get("disable_ssl")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        trusted_connection: tbl
            .get("trusted_connection")
            .or_else(|| tbl.get("windows_auth"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        default: tbl.get("default").and_then(|v| v.as_bool()).unwrap_or(false),
        enabled: tbl.get("enabled").and_then(|v| v.as_bool()).unwrap_or(true),
    })
}

fn merge_secrets(base: Vec<WorkspaceSecret>, overrides: Vec<WorkspaceSecret>) -> Vec<WorkspaceSecret> {
    let mut merged = base;
    for ov in overrides {
        if let Some(pos) = merged
            .iter()
            .position(|s| s.name.eq_ignore_ascii_case(&ov.name))
        {
            merged[pos] = ov;
        } else {
            merged.push(ov);
        }
    }
    merged.into_iter().filter(|s| s.enabled).collect()
}

fn merge_attachments(
    base: Vec<WorkspaceAttachment>,
    overrides: Vec<WorkspaceAttachment>,
) -> Vec<WorkspaceAttachment> {
    let mut merged = base;
    for ov in overrides {
        if let Some(pos) = merged
            .iter()
            .position(|a| a.alias.eq_ignore_ascii_case(&ov.alias))
        {
            merged[pos] = ov;
        } else {
            merged.push(ov);
        }
    }
    merged.into_iter().filter(|a| a.enabled).collect()
}

/// Loads and merges top-level `[[secrets]]` / `[[attach]]` with optional `[[env.<profile>.secrets]]`
/// and `[[env.<profile>.attach]]` from `duckpack.toml`, enforcing strict `*_env` secret hygiene.
pub fn load_workspace_config(
    project_dir: &Path,
    env_profile: Option<&str>,
) -> Result<WorkspaceConfig> {
    load_env_files(project_dir, env_profile);
    let toml_path = project_dir.join("duckpack.toml");
    if !toml_path.exists() {
        return Ok(WorkspaceConfig::default());
    }
    let content = std::fs::read_to_string(&toml_path)
        .with_context(|| format!("Failed reading {}", toml_path.display()))?;
    let parsed: toml::Value = toml::from_str(&content)
        .with_context(|| format!("Invalid TOML syntax in {}", toml_path.display()))?;

    let mut base_secrets = Vec::new();
    if let Some(arr) = parsed.get("secrets").and_then(|v| v.as_array()) {
        for item in arr {
            base_secrets.push(parse_secret_entry(item, "[[secrets]]")?);
        }
    }

    let mut base_attachments = Vec::new();
    if let Some(arr) = parsed.get("attach").and_then(|v| v.as_array()) {
        for item in arr {
            base_attachments.push(parse_attachment_entry(item, "[[attach]]")?);
        }
    }

    let mut env_secrets = Vec::new();
    let mut env_attachments = Vec::new();
    if let Some(profile) = env_profile.map(|s| s.trim()).filter(|s| !s.is_empty()) {
        if let Some(env_tbl) = parsed
            .get("env")
            .and_then(|e| e.get(profile))
            .and_then(|v| v.as_table())
        {
            if let Some(arr) = env_tbl.get("secrets").and_then(|v| v.as_array()) {
                let lbl = format!("[[env.{}.secrets]]", profile);
                for item in arr {
                    env_secrets.push(parse_secret_entry(item, &lbl)?);
                }
            }
            if let Some(arr) = env_tbl.get("attach").and_then(|v| v.as_array()) {
                let lbl = format!("[[env.{}.attach]]", profile);
                for item in arr {
                    env_attachments.push(parse_attachment_entry(item, &lbl)?);
                }
            }
        }
    }

    Ok(WorkspaceConfig {
        secrets: merge_secrets(base_secrets, env_secrets),
        attachments: merge_attachments(base_attachments, env_attachments),
    })
}

/// Returns `true` if `duckpack.toml` defines at least one active `[[attach]]` entry for `env_profile`.
pub fn has_workspace_attachments(project_dir: &Path, env_profile: Option<&str>) -> bool {
    load_workspace_config(project_dir, env_profile)
        .map(|c| !c.attachments.is_empty())
        .unwrap_or(false)
}

/// Returns `true` when `duckpack explore` should launch in clean in-memory Workspace Mode
/// (no explicit `--db`, no `[target].db` / `[env.<profile>].db`, and at least one `[[attach]]` entry).
pub fn should_use_workspace_mode(
    project_dir: &Path,
    env_profile: Option<&str>,
    explicit_db: Option<&str>,
) -> bool {
    let has_explicit_db = explicit_db
        .map(|s| !s.trim().is_empty())
        .unwrap_or(false);
    if has_explicit_db {
        return false;
    }
    let target_cfg = resolve_target_config(project_dir, env_profile);
    if target_cfg.db.is_some() {
        return false;
    }
    has_workspace_attachments(project_dir, env_profile)
}

/// Resolves an env var by checking process environment first, then `.env.<profile>` (validated), then `.env`.
pub fn resolve_workspace_env_var(
    project_dir: &Path,
    env_profile: Option<&str>,
    key: &str,
) -> Option<String> {
    crate::env_store::resolve_env_var(Some(project_dir), env_profile, key)
}

/// Registers all declarative `[[secrets]]` (and `[[env.<profile>.secrets]]`) on `conn`.
/// Safe to invoke across `explore`, `diff`, `apply`, and Python API.
/// Returns `(loaded_secret_names, warnings)` and never panics on missing optional env vars.
pub fn apply_workspace_secrets(
    conn: &Connection,
    project_dir: &Path,
    env_profile: Option<&str>,
) -> Result<(Vec<String>, Vec<String>)> {
    let cfg = load_workspace_config(project_dir, env_profile)?;
    let mut loaded = Vec::new();
    let mut warnings = Vec::new();

    for sec in &cfg.secrets {
        let kind = match crate::connectors::SecretKind::from_type_str(&sec.secret_type) {
            Ok(k) => k,
            Err(_) => {
                warnings.push(format!(
                    "Secret '{}' has unsupported type '{}'",
                    sec.name, sec.secret_type
                ));
                continue;
            }
        };

        let mut missing = Vec::new();
        let mut resolve_req = |env_opt: &Option<String>| -> Option<String> {
            let var = env_opt.as_ref()?;
            match resolve_workspace_env_var(project_dir, env_profile, var) {
                Some(v) => Some(v),
                None => {
                    missing.push(var.clone());
                    None
                }
            }
        };

        let key_id = if kind == crate::connectors::SecretKind::Azure {
            sec.key_id_env
                .as_deref()
                .and_then(|k| resolve_workspace_env_var(project_dir, env_profile, k))
        } else {
            resolve_req(&sec.key_id_env)
        };
        let secret = resolve_req(&sec.secret_env);
        let token = if kind == crate::connectors::SecretKind::Quack && sec.token_env.is_none() {
            warnings.push(format!("Quack secret '{}' missing token_env", sec.name));
            continue;
        } else {
            resolve_req(&sec.token_env)
        };

        if !missing.is_empty() {
            warnings.push(format!(
                "Secret '{}' skipped (missing env: {})",
                sec.name,
                missing.join(", ")
            ));
            continue;
        }

        let endpoint = sec
            .endpoint_env
            .as_deref()
            .and_then(|e| resolve_workspace_env_var(project_dir, env_profile, e))
            .or_else(|| sec.endpoint.clone());
        let region = sec
            .region_env
            .as_deref()
            .and_then(|r| resolve_workspace_env_var(project_dir, env_profile, r))
            .or_else(|| sec.region.clone());

        let spec = crate::connectors::SecretSpec {
            name: sec.name.clone(),
            kind,
            persistent: false,
            key_id,
            secret,
            token,
            region,
            endpoint,
            url_style: sec.url_style.clone(),
            use_ssl: sec.use_ssl,
            scope: sec.scope.clone(),
        };

        match crate::connectors::plan_secret(&spec)
            .and_then(|plan| crate::connectors::execute_secret_plan(conn, &plan))
        {
            Ok(_) => {
                loaded.push(sec.name.clone());
                if matches!(
                    kind,
                    crate::connectors::SecretKind::S3 | crate::connectors::SecretKind::R2
                ) {
                    if let Some(b) = sec
                        .bucket
                        .as_deref()
                        .and_then(crate::connectors::normalize_bucket_uri)
                    {
                        let existing = std::env::var("DUCKPACK_S3_BUCKETS").unwrap_or_default();
                        let already = existing
                            .split(',')
                            .any(|p| p.trim().eq_ignore_ascii_case(&b));
                        if !already {
                            let updated = if existing.trim().is_empty() {
                                b
                            } else {
                                format!("{},{}", existing.trim_end_matches(','), b)
                            };
                            unsafe {
                                std::env::set_var("DUCKPACK_S3_BUCKETS", &updated);
                            }
                        }
                    }
                }
            }
            Err(e) => warnings.push(format!(
                "Secret '{}' failed: {}",
                sec.name,
                redact_connection_credentials(&e.to_string())
            )),
        }
    }

    Ok((loaded, warnings))
}

/// Resolves the normalized `quack:<host>:<port>` endpoint for either a `quack` attachment
/// or a `ducklake` attachment backed by `quack`.
pub fn resolve_attachment_quack_endpoint(entry: &WorkspaceAttachment) -> Option<String> {
    let t = entry.attach_type.to_lowercase();
    if t == "quack" {
        if let Some(ref meta) = entry.metadata {
            return Some(format_quack_endpoint(meta, ""));
        }
        if let Some(ref path) = entry.path {
            return Some(format_quack_endpoint(path, ""));
        }
        if let Some(ref host) = entry.host {
            let port_str = entry.port.map(|p| p.to_string()).unwrap_or_default();
            return Some(format_quack_endpoint(host, &port_str));
        }
        return None;
    }
    if t == "ducklake" {
        let is_quack_backend = entry
            .backend
            .as_deref()
            .map(|b| b.eq_ignore_ascii_case("quack"))
            .unwrap_or(false)
            || entry
                .metadata
                .as_deref()
                .map(|m| m.contains("quack:"))
                .unwrap_or(false);
        if !is_quack_backend {
            return None;
        }
        if let Some(ref meta) = entry.metadata {
            let stripped = meta.trim().strip_prefix("ducklake:").unwrap_or(meta.trim());
            return Some(format_quack_endpoint(stripped, ""));
        }
        if let Some(ref host) = entry.host {
            let port_str = entry.port.map(|p| p.to_string()).unwrap_or_default();
            return Some(format_quack_endpoint(host, &port_str));
        }
    }
    None
}

/// Attaches a single `WorkspaceAttachment` into `conn` with strict per-alias credential isolation.
pub fn attach_single_workspace_entry(
    conn: &Connection,
    entry: &WorkspaceAttachment,
    project_dir: &Path,
    env_profile: Option<&str>,
) -> std::result::Result<(), AttachmentIssueKind> {
    let provider = crate::connectors::AttachProviderKind::from_type_str(&entry.attach_type)
        .map_err(|e| AttachmentIssueKind::ConnectionFailed(e.to_string()))?;

    // Check required *_env vars up front
    let mut missing = Vec::new();
    let resolve_opt = |env_name: &Option<String>, missing_list: &mut Vec<String>| -> Option<String> {
        if let Some(var) = env_name {
            match resolve_workspace_env_var(project_dir, env_profile, var) {
                Some(val) => Some(val),
                None => {
                    missing_list.push(var.clone());
                    None
                }
            }
        } else {
            None
        }
    };

    let token = resolve_opt(&entry.token_env, &mut missing);
    let password = resolve_opt(&entry.password_env, &mut missing);
    let dsn_from_env = resolve_opt(&entry.dsn_env, &mut missing);
    let user_from_env = resolve_opt(&entry.username_env, &mut missing);

    if !missing.is_empty() {
        return Err(AttachmentIssueKind::MissingEnv(missing));
    }

    let effective_user = user_from_env.or_else(|| entry.username.clone());
    let spec = crate::connectors::AttachSpec {
        provider: Some(provider),
        alias: entry.alias.clone(),
        path: entry.path.clone(),
        metadata: entry.metadata.clone(),
        data_path: entry.data_path.clone(),
        ducklake_backend: crate::connectors::DuckLakeBackendKind::from_str_opt(
            entry.backend.as_deref(),
        ),
        host: entry.host.clone(),
        port: entry.port.map(|p| p.to_string()),
        database: entry.database.clone(),
        username: effective_user,
        password,
        token,
        client_id: entry.client_id.clone(),
        dsn: dsn_from_env,
        read_only: entry.read_only,
        disable_ssl: entry.disable_ssl,
        insecure: entry.insecure,
        trusted_connection: entry.trusted_connection,
        detach_first: true,
        base_dir: Some(project_dir.to_path_buf()),
    };

    let plan = crate::connectors::plan_attach(&spec)
        .map_err(|e| AttachmentIssueKind::ConnectionFailed(e.to_string()))?;
    crate::connectors::execute_attach_plan(conn, &plan, Some(project_dir))
        .map_err(|e| AttachmentIssueKind::ConnectionFailed(e.to_string()))?;
    Ok(())
}

/// Applies all declarative `[[secrets]]` followed by all declarative `[[attach]]` entries
/// resiliently (never aborting if an individual catalog is offline or missing an env var).
pub fn apply_workspace_attachments(
    conn: &Connection,
    project_dir: &Path,
    env_profile: Option<&str>,
) -> Result<WorkspaceApplyReport> {
    let cfg = load_workspace_config(project_dir, env_profile)?;
    let (loaded_secrets, secret_warnings) = apply_workspace_secrets(conn, project_dir, env_profile)?;

    let mut report = WorkspaceApplyReport {
        loaded_secrets,
        secret_warnings,
        attached_aliases: Vec::new(),
        failed_attachments: Vec::new(),
        default_alias: None,
    };

    for entry in &cfg.attachments {
        match attach_single_workspace_entry(conn, entry, project_dir, env_profile) {
            Ok(()) => {
                report.attached_aliases.push(entry.alias.clone());
                if entry.default || report.default_alias.is_none() {
                    if entry.default {
                        report.default_alias = Some(entry.alias.clone());
                    } else if report.default_alias.is_none() {
                        report.default_alias = Some(entry.alias.clone());
                    }
                }
            }
            Err(kind) => {
                report.failed_attachments.push(FailedAttachment {
                    alias: entry.alias.clone(),
                    attach_type: entry.attach_type.clone(),
                    kind,
                    entry: entry.clone(),
                });
            }
        }
    }

    Ok(report)
}

/// Discovers all Quack control targets for `Ctrl+U`:
/// 1. Every directly attached `quack:` database in `conn` (`duckdb_databases()`).
/// 2. Every `ducklake` attachment backed by `quack:` declared in `duckpack.toml`
///    (connected lazily on-demand by `Ctrl+U` without cluttering Explorer!).
pub fn discover_quack_control_targets(
    conn: &Connection,
    project_dir: &Path,
    env_profile: Option<&str>,
) -> Vec<QuackControlTarget> {
    let mut targets: Vec<QuackControlTarget> = Vec::new();
    let mut seen_endpoints: Vec<String> = Vec::new();

    if let Ok(mut stmt) = conn.prepare(
        "SELECT database_name, COALESCE(path, '') FROM duckdb_databases() \
         WHERE internal = false AND (lower(type) = 'quack' OR path LIKE 'quack:%') \
         ORDER BY database_name",
    ) {
        if let Ok(rows) = stmt.query_map([], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
        }) {
            for r in rows.flatten() {
                let (db_name, path) = r;
                let ep = if path.starts_with("quack:") {
                    Some(path.clone())
                } else {
                    None
                };
                if let Some(ref e) = ep {
                    seen_endpoints.push(e.to_lowercase());
                }
                targets.push(QuackControlTarget {
                    label: format!("{} (quack)", db_name),
                    attached_alias: Some(db_name),
                    quack_endpoint: ep,
                    token_env: None,
                    client_id: None,
                    disable_ssl: false,
                });
            }
        }
    }

    if let Ok(cfg) = load_workspace_config(project_dir, env_profile) {
        for entry in &cfg.attachments {
            if entry.attach_type.eq_ignore_ascii_case("ducklake") {
                if let Some(ep) = resolve_attachment_quack_endpoint(entry) {
                    let disable_ssl =
                        should_disable_quack_ssl(&ep, entry.insecure || entry.disable_ssl);
                    let token_env = entry
                        .admin_token_env
                        .clone()
                        .or_else(|| entry.token_env.clone());
                    let label = format!("{} (ducklake→quack)", entry.alias);
                    if !targets.iter().any(|t| t.label == label) {
                        targets.push(QuackControlTarget {
                            label,
                            attached_alias: None,
                            quack_endpoint: Some(ep),
                            token_env,
                            client_id: entry.client_id.clone(),
                            disable_ssl,
                        });
                    }
                }
            } else if entry.attach_type.eq_ignore_ascii_case("quack") {
                // Enrich existing attached alias metadata if needed
                if let Some(existing) = targets
                    .iter_mut()
                    .find(|t| t.attached_alias.as_deref() == Some(entry.alias.as_str()))
                {
                    existing.token_env = entry
                        .admin_token_env
                        .clone()
                        .or_else(|| entry.token_env.clone());
                    existing.client_id = entry.client_id.clone();
                    if let Some(ep) = resolve_attachment_quack_endpoint(entry) {
                        existing.disable_ssl =
                            should_disable_quack_ssl(&ep, entry.insecure || entry.disable_ssl);
                    }
                } else if let Some(ep) = resolve_attachment_quack_endpoint(entry) {
                    if !seen_endpoints.contains(&ep.to_lowercase()) {
                        let disable_ssl =
                            should_disable_quack_ssl(&ep, entry.insecure || entry.disable_ssl);
                        targets.push(QuackControlTarget {
                            label: format!("{} (quack)", entry.alias),
                            attached_alias: None,
                            quack_endpoint: Some(ep),
                            token_env: entry
                                .admin_token_env
                                .clone()
                                .or_else(|| entry.token_env.clone()),
                            client_id: entry.client_id.clone(),
                            disable_ssl,
                        });
                    }
                }
            }
        }
    }

    targets
}

/// Opens a transient on-demand Quack connection for a DuckLake-on-Quack (or non-directly-attached)
/// `QuackControlTarget` so `Ctrl+U` can inspect sessions & deploy RBAC without cluttering Explorer.
pub fn open_quack_control_connection(
    target: &QuackControlTarget,
    project_dir: &Path,
    env_profile: Option<&str>,
) -> Result<(Connection, String)> {
    let endpoint = target.quack_endpoint.as_deref().ok_or_else(|| {
        anyhow::anyhow!("Quack control target '{}' has no quack endpoint.", target.label)
    })?;
    let conn = open_duckdb_connection()?;
    load_quack_extension(&conn)?;

    let token = target
        .token_env
        .as_deref()
        .and_then(|var| resolve_workspace_env_var(project_dir, env_profile, var));

    let mut opts = Vec::new();
    if should_disable_quack_ssl(endpoint, target.disable_ssl) {
        opts.push("DISABLE_SSL true".to_string());
    }
    if let Some(ref tok) = token {
        opts.push(format!("TOKEN '{}'", tok.replace('\'', "''")));
    }
    let cid = target
        .client_id
        .clone()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| token.clone());
    if let Some(ref c) = cid {
        opts.push(format!("CLIENT_ID '{}'", c.replace('\'', "''")));
    }

    let alias = "quack_ctrl".to_string();
    let sql = if opts.is_empty() {
        format!("ATTACH '{}' AS {};", endpoint.replace('\'', "''"), alias)
    } else {
        format!(
            "ATTACH '{}' AS {} ({});",
            endpoint.replace('\'', "''"),
            alias,
            opts.join(", ")
        )
    };
    conn.execute(&sql, []).map_err(|e| {
        anyhow::anyhow!(
            "Failed connecting to Quack metadata server '{}': {}",
            endpoint,
            redact_connection_credentials(&e.to_string())
        )
    })?;
    Ok((conn, alias))
}

/// Upserts key/value pairs into the target `.env` (or validated `.env.<profile>` if it exists)
/// atomically with `0600` perms and newline/control-char sanitization (M1, M4).
pub fn save_workspace_env_pairs(
    project_dir: &Path,
    env_profile: Option<&str>,
    pairs: &[(&str, &str)],
) -> Result<PathBuf> {
    crate::env_store::upsert_env_pairs(project_dir, env_profile, pairs)
}

fn secret_to_toml_value(sec: &WorkspaceSecret) -> toml::Value {
    let mut tbl = toml::map::Map::new();
    tbl.insert("name".to_string(), toml::Value::String(sec.name.clone()));
    tbl.insert(
        "type".to_string(),
        toml::Value::String(sec.secret_type.clone()),
    );
    if let Some(ref v) = sec.scope {
        tbl.insert("scope".to_string(), toml::Value::String(v.clone()));
    }
    if let Some(ref v) = sec.endpoint {
        tbl.insert("endpoint".to_string(), toml::Value::String(v.clone()));
    }
    if let Some(ref v) = sec.endpoint_env {
        tbl.insert("endpoint_env".to_string(), toml::Value::String(v.clone()));
    }
    if let Some(ref v) = sec.region {
        tbl.insert("region".to_string(), toml::Value::String(v.clone()));
    }
    if let Some(ref v) = sec.region_env {
        tbl.insert("region_env".to_string(), toml::Value::String(v.clone()));
    }
    if let Some(ref v) = sec.key_id_env {
        tbl.insert("key_id_env".to_string(), toml::Value::String(v.clone()));
    }
    if let Some(ref v) = sec.secret_env {
        tbl.insert("secret_env".to_string(), toml::Value::String(v.clone()));
    }
    if let Some(ref v) = sec.token_env {
        tbl.insert("token_env".to_string(), toml::Value::String(v.clone()));
    }
    if let Some(ref v) = sec.url_style {
        tbl.insert("url_style".to_string(), toml::Value::String(v.clone()));
    }
    if let Some(v) = sec.use_ssl {
        tbl.insert("use_ssl".to_string(), toml::Value::Boolean(v));
    }
    if let Some(ref v) = sec.bucket {
        tbl.insert("bucket".to_string(), toml::Value::String(v.clone()));
    }
    if !sec.enabled {
        tbl.insert("enabled".to_string(), toml::Value::Boolean(false));
    }
    toml::Value::Table(tbl)
}

fn attachment_to_toml_value(att: &WorkspaceAttachment) -> toml::Value {
    let mut tbl = toml::map::Map::new();
    tbl.insert("alias".to_string(), toml::Value::String(att.alias.clone()));
    tbl.insert(
        "type".to_string(),
        toml::Value::String(att.attach_type.clone()),
    );
    if let Some(ref v) = att.backend {
        tbl.insert("backend".to_string(), toml::Value::String(v.clone()));
    }
    if let Some(ref v) = att.metadata {
        tbl.insert("metadata".to_string(), toml::Value::String(v.clone()));
    }
    if let Some(ref v) = att.path {
        tbl.insert("path".to_string(), toml::Value::String(v.clone()));
    }
    if let Some(ref v) = att.host {
        tbl.insert("host".to_string(), toml::Value::String(v.clone()));
    }
    if let Some(p) = att.port {
        tbl.insert("port".to_string(), toml::Value::Integer(p as i64));
    }
    if let Some(ref v) = att.database {
        tbl.insert("database".to_string(), toml::Value::String(v.clone()));
    }
    if let Some(ref v) = att.username {
        tbl.insert("username".to_string(), toml::Value::String(v.clone()));
    }
    if let Some(ref v) = att.username_env {
        tbl.insert("username_env".to_string(), toml::Value::String(v.clone()));
    }
    if let Some(ref v) = att.password_env {
        tbl.insert("password_env".to_string(), toml::Value::String(v.clone()));
    }
    if let Some(ref v) = att.token_env {
        tbl.insert("token_env".to_string(), toml::Value::String(v.clone()));
    }
    if let Some(ref v) = att.admin_token_env {
        tbl.insert(
            "admin_token_env".to_string(),
            toml::Value::String(v.clone()),
        );
    }
    if let Some(ref v) = att.dsn_env {
        tbl.insert("dsn_env".to_string(), toml::Value::String(v.clone()));
    }
    if let Some(ref v) = att.client_id {
        tbl.insert("client_id".to_string(), toml::Value::String(v.clone()));
    }
    if let Some(ref v) = att.data_path {
        tbl.insert("data_path".to_string(), toml::Value::String(v.clone()));
    }
    if att.read_only {
        tbl.insert("read_only".to_string(), toml::Value::Boolean(true));
    }
    if att.insecure {
        tbl.insert("insecure".to_string(), toml::Value::Boolean(true));
    }
    if att.disable_ssl {
        tbl.insert("disable_ssl".to_string(), toml::Value::Boolean(true));
    }
    if att.trusted_connection {
        tbl.insert(
            "trusted_connection".to_string(),
            toml::Value::Boolean(true),
        );
    }
    if att.default {
        tbl.insert("default".to_string(), toml::Value::Boolean(true));
    }
    if !att.enabled {
        tbl.insert("enabled".to_string(), toml::Value::Boolean(false));
    }
    toml::Value::Table(tbl)
}

fn upsert_toml_array_entry(
    project_dir: &Path,
    env_profile: Option<&str>,
    array_key: &str,
    match_key: &str,
    match_val: &str,
    new_entry: toml::Value,
) -> Result<PathBuf> {
    let toml_path = project_dir.join("duckpack.toml");
    let mut root: toml::Value = if toml_path.exists() {
        let content = std::fs::read_to_string(&toml_path)?;
        if content.trim().is_empty() {
            toml::Value::Table(toml::map::Map::new())
        } else {
            toml::from_str(&content)
                .with_context(|| format!("Failed parsing {}", toml_path.display()))?
        }
    } else {
        toml::Value::Table(toml::map::Map::new())
    };

    let root_tbl = root
        .as_table_mut()
        .ok_or_else(|| anyhow::anyhow!("duckpack.toml root must be a table"))?;

    let target_tbl = if let Some(profile) = env_profile.map(|s| s.trim()).filter(|s| !s.is_empty())
    {
        let env_map = root_tbl
            .entry("env".to_string())
            .or_insert_with(|| toml::Value::Table(toml::map::Map::new()))
            .as_table_mut()
            .ok_or_else(|| anyhow::anyhow!("[env] in duckpack.toml must be a table"))?;
        env_map
            .entry(profile.to_string())
            .or_insert_with(|| toml::Value::Table(toml::map::Map::new()))
            .as_table_mut()
            .ok_or_else(|| anyhow::anyhow!("[env.{}] in duckpack.toml must be a table", profile))?
    } else {
        root_tbl
    };

    let arr = target_tbl
        .entry(array_key.to_string())
        .or_insert_with(|| toml::Value::Array(Vec::new()))
        .as_array_mut()
        .ok_or_else(|| anyhow::anyhow!("'{}' in duckpack.toml must be an array of tables", array_key))?;

    if let Some(pos) = arr.iter().position(|item| {
        item.get(match_key)
            .and_then(|v| v.as_str())
            .map(|s| s.eq_ignore_ascii_case(match_val))
            .unwrap_or(false)
    }) {
        arr[pos] = new_entry;
    } else {
        arr.push(new_entry);
    }

    let serialized = toml::to_string_pretty(&root)
        .with_context(|| "Failed serializing updated duckpack.toml")?;
    std::fs::write(&toml_path, serialized)
        .with_context(|| format!("Failed writing {}", toml_path.display()))?;
    Ok(toml_path)
}

/// Persists a declarative `WorkspaceAttachment` to `duckpack.toml` (under `[[env.<profile>.attach]]`
/// when `env_profile` is active, else `[[attach]]`) and writes any secret values to `.env` (`0600`).
pub fn save_attachment_to_workspace(
    project_dir: &Path,
    env_profile: Option<&str>,
    entry: &WorkspaceAttachment,
    secret_env_pairs: &[(&str, &str)],
) -> Result<(PathBuf, PathBuf)> {
    let env_path = save_workspace_env_pairs(project_dir, env_profile, secret_env_pairs)?;
    let toml_val = attachment_to_toml_value(entry);
    let toml_path = upsert_toml_array_entry(
        project_dir,
        env_profile,
        "attach",
        "alias",
        &entry.alias,
        toml_val,
    )?;
    Ok((toml_path, env_path))
}

/// Persists a declarative `WorkspaceSecret` to `duckpack.toml` (under `[[env.<profile>.secrets]]`
/// when `env_profile` is active, else `[[secrets]]`) and writes any secret values to `.env` (`0600`).
pub fn save_secret_to_workspace(
    project_dir: &Path,
    env_profile: Option<&str>,
    secret: &WorkspaceSecret,
    secret_env_pairs: &[(&str, &str)],
) -> Result<(PathBuf, PathBuf)> {
    let env_path = save_workspace_env_pairs(project_dir, env_profile, secret_env_pairs)?;
    let toml_val = secret_to_toml_value(secret);
    let toml_path = upsert_toml_array_entry(
        project_dir,
        env_profile,
        "secrets",
        "name",
        &secret.name,
        toml_val,
    )?;
    Ok((toml_path, env_path))
}

/// Converts a catalog alias or secret name into a clean uppercase env var token fragment.
pub fn env_var_slug(name: &str) -> String {
    let slug: String = name
        .trim()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect();
    if slug.is_empty() {
        "DEFAULT".to_string()
    } else {
        slug
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_temp_dir(prefix: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "{}_{}",
            prefix,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn test_workspace_env_layering_and_disable_override() {
        let dir = make_temp_dir("dp_ws_layer");
        std::fs::write(
            dir.join("duckpack.toml"),
            r#"
[[secrets]]
name = "lake_s3"
type = "s3"
scope = "s3://dev-bucket/"
key_id_env = "DEV_S3_KEY"
secret_env = "DEV_S3_SECRET"

[[attach]]
alias = "sales_lake"
type = "duckdb"
path = "sales_dev.duckdb"
default = true

[[attach]]
alias = "scratch_db"
type = "duckdb"
path = ":memory:"

[[env.prod.secrets]]
name = "lake_s3"
type = "s3"
scope = "s3://prod-bucket/"
key_id_env = "PROD_S3_KEY"
secret_env = "PROD_S3_SECRET"

[[env.prod.attach]]
alias = "sales_lake"
type = "duckdb"
path = "sales_prod.duckdb"
read_only = true

[[env.prod.attach]]
alias = "scratch_db"
type = "duckdb"
enabled = false

[[env.prod.attach]]
alias = "finance_lake"
type = "ducklake"
backend = "quack"
metadata = "quack:finance.internal:9494"
data_path = "s3://prod-bucket/finance/"
token_env = "QUACK_FINANCE_TOKEN"
client_id = "ben_admin"
insecure = true
"#,
        )
        .unwrap();

        let default_cfg = load_workspace_config(&dir, None).unwrap();
        assert_eq!(default_cfg.secrets.len(), 1);
        assert_eq!(
            default_cfg.secrets[0].scope.as_deref(),
            Some("s3://dev-bucket/")
        );
        assert_eq!(default_cfg.attachments.len(), 2);
        assert_eq!(
            default_cfg.attachments[0].path.as_deref(),
            Some("sales_dev.duckdb")
        );

        let prod_cfg = load_workspace_config(&dir, Some("prod")).unwrap();
        assert_eq!(prod_cfg.secrets.len(), 1);
        assert_eq!(
            prod_cfg.secrets[0].scope.as_deref(),
            Some("s3://prod-bucket/")
        );
        // scratch_db disabled via enabled = false; sales_lake overridden; finance_lake appended
        assert_eq!(prod_cfg.attachments.len(), 2);
        assert_eq!(prod_cfg.attachments[0].alias, "sales_lake");
        assert_eq!(
            prod_cfg.attachments[0].path.as_deref(),
            Some("sales_prod.duckdb")
        );
        assert!(prod_cfg.attachments[0].read_only);
        assert_eq!(prod_cfg.attachments[1].alias, "finance_lake");

        // Workspace mode should activate when no explicit --db and no [target].db
        assert!(should_use_workspace_mode(&dir, Some("prod"), None));
        assert!(!should_use_workspace_mode(
            &dir,
            Some("prod"),
            Some("explicit.duckdb")
        ));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_workspace_secret_hygiene_rejects_raw_credentials() {
        let dir = make_temp_dir("dp_ws_hygiene");
        std::fs::write(
            dir.join("duckpack.toml"),
            r#"
[[attach]]
alias = "bad_quack"
type = "quack"
host = "localhost"
port = 9494
token = "raw_secret_should_fail"
"#,
        )
        .unwrap();

        let err = load_workspace_config(&dir, None).unwrap_err().to_string();
        assert!(
            err.contains("Secret Hygiene Error") && err.contains("token_env"),
            "Expected Secret Hygiene Error pointing to token_env, got: {}",
            err
        );

        std::fs::write(
            dir.join("duckpack.toml"),
            r#"
[[secrets]]
name = "bad_s3"
type = "s3"
secret = "plaintext_aws_secret"
"#,
        )
        .unwrap();
        let err2 = load_workspace_config(&dir, None).unwrap_err().to_string();
        assert!(err2.contains("Secret Hygiene Error") && err2.contains("secret_env"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_workspace_resilient_attach_and_roundtrip_save() {
        let dir = make_temp_dir("dp_ws_apply");
        // Create one real local DuckDB file to attach cleanly
        let good_db_path = dir.join("sales.duckdb");
        {
            let c = Connection::open(&good_db_path).unwrap();
            c.execute("CREATE TABLE orders (id INT);", []).unwrap();
        }

        // Save 1 healthy attachment, 1 missing-env attachment, and 1 offline attachment
        let good_att = WorkspaceAttachment {
            alias: "sales".to_string(),
            attach_type: "duckdb".to_string(),
            path: Some("sales.duckdb".to_string()),
            default: true,
            ..Default::default()
        };
        let locked_att = WorkspaceAttachment {
            alias: "hr_quack".to_string(),
            attach_type: "quack".to_string(),
            host: Some("127.0.0.1".to_string()),
            port: Some(19999),
            token_env: Some("MISSING_HR_TOKEN_XYZ".to_string()),
            ..Default::default()
        };
        save_attachment_to_workspace(&dir, Some("prod"), &good_att, &[]).unwrap();
        save_attachment_to_workspace(&dir, Some("prod"), &locked_att, &[]).unwrap();

        let sec = WorkspaceSecret {
            name: "s3_sales".to_string(),
            secret_type: "s3".to_string(),
            scope: Some("s3://sales-bucket/".to_string()),
            key_id_env: Some("WS_TEST_S3_KEY".to_string()),
            secret_env: Some("WS_TEST_S3_SECRET".to_string()),
            endpoint: Some("localhost:9000".to_string()),
            use_ssl: Some(false),
            ..Default::default()
        };
        save_secret_to_workspace(
            &dir,
            Some("prod"),
            &sec,
            &[("WS_TEST_S3_KEY", "minioadmin"), ("WS_TEST_S3_SECRET", "miniosecret")],
        )
        .unwrap();

        // Confirm plaintext secrets were NOT written to duckpack.toml
        let toml_text = std::fs::read_to_string(dir.join("duckpack.toml")).unwrap();
        assert!(!toml_text.contains("miniosecret"));
        assert!(toml_text.contains("WS_TEST_S3_SECRET"));
        assert!(toml_text.contains("env.prod"));

        // Apply workspace attachments resiliently
        let conn = open_duckdb_connection().unwrap();
        let report = apply_workspace_attachments(&conn, &dir, Some("prod")).unwrap();
        assert_eq!(report.attached_aliases, vec!["sales".to_string()]);
        assert_eq!(report.default_alias.as_deref(), Some("sales"));
        assert_eq!(report.failed_attachments.len(), 1);
        assert_eq!(report.failed_attachments[0].alias, "hr_quack");
        assert!(matches!(
            report.failed_attachments[0].kind,
            AttachmentIssueKind::MissingEnv(_)
        ));
        assert!(report.failed_attachments[0]
            .explorer_display()
            .contains("MISSING_HR_TOKEN_XYZ"));

        // Verify mounted catalog is queryable
        let cnt: i64 = conn
            .query_row("SELECT count(*) FROM sales.orders;", [], |r| r.get(0))
            .unwrap();
        assert_eq!(cnt, 0);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
