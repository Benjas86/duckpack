use anyhow::{Context, Result};
use duckdb::Connection;
use std::path::{Path, PathBuf};

use crate::engine::{ensure_extension_loaded, load_quack_extension, should_disable_quack_ssl};
use crate::sql_utils::{quote_ident, quote_literal, redact_connection_credentials};

/// Supported database providers across CLI `--db`, `duckpack.toml` `[[attach]]`, IDE `Ctrl+A`, and Python.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttachProviderKind {
    DuckDb,
    Sqlite,
    Quack,
    DuckLake,
    MotherDuck,
    Postgres,
    Mysql,
    Mssql,
}

impl AttachProviderKind {
    pub fn from_type_str(s: &str) -> Result<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "duckdb" | "local" | "file" => Ok(Self::DuckDb),
            "sqlite" => Ok(Self::Sqlite),
            "quack" => Ok(Self::Quack),
            "ducklake" => Ok(Self::DuckLake),
            "motherduck" | "md" => Ok(Self::MotherDuck),
            "postgres" | "postgresql" => Ok(Self::Postgres),
            "mysql" => Ok(Self::Mysql),
            "mssql" | "sqlserver" => Ok(Self::Mssql),
            other => anyhow::bail!("Unsupported attachment type '{}'", other),
        }
    }
}

/// Supported DuckLake metadata catalog backends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DuckLakeBackendKind {
    File,
    Quack,
    Postgres,
    Sqlite,
}

impl DuckLakeBackendKind {
    pub fn from_str_opt(s: Option<&str>) -> Option<Self> {
        match s?.trim().to_ascii_lowercase().as_str() {
            "quack" => Some(Self::Quack),
            "postgres" | "postgresql" => Some(Self::Postgres),
            "sqlite" => Some(Self::Sqlite),
            "file" | "duckdb" | "local" => Some(Self::File),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::File => "file",
            Self::Quack => "quack",
            Self::Postgres => "postgres",
            Self::Sqlite => "sqlite",
        }
    }
}

/// Provider-normalized specification for attaching any catalog into DuckDB.
#[derive(Debug, Clone, Default)]
pub struct AttachSpec {
    pub provider: Option<AttachProviderKind>,
    pub alias: String,
    pub path: Option<String>,
    pub metadata: Option<String>,
    pub data_path: Option<String>,
    pub ducklake_backend: Option<DuckLakeBackendKind>,
    pub host: Option<String>,
    pub port: Option<String>,
    pub database: Option<String>,
    pub username: Option<String>,
    pub password: Option<String>,
    pub token: Option<String>,
    pub client_id: Option<String>,
    pub dsn: Option<String>,
    pub read_only: bool,
    pub disable_ssl: bool,
    pub insecure: bool,
    pub trusted_connection: bool,
    pub detach_first: bool,
    pub base_dir: Option<PathBuf>,
}

/// Pure, compiled attachment plan (zero I/O, fully unit-testable).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachPlan {
    pub extensions: Vec<String>,
    pub target_uri: String,
    pub attach_sql: String,
    pub detach_first: bool,
    pub alias: String,
    pub needs_s3_env_fallback: bool,
}

/// Supported cloud / authentication secret providers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecretKind {
    S3,
    R2,
    Gcs,
    Azure,
    Quack,
    Http,
}

impl SecretKind {
    pub fn from_type_str(s: &str) -> Result<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "s3" => Ok(Self::S3),
            "r2" => Ok(Self::R2),
            "gcs" => Ok(Self::Gcs),
            "azure" => Ok(Self::Azure),
            "quack" => Ok(Self::Quack),
            "http" => Ok(Self::Http),
            other => anyhow::bail!("Unsupported secret type '{}'", other),
        }
    }
}

/// Provider-normalized specification for `CREATE OR REPLACE [PERSISTENT] SECRET`.
#[derive(Debug, Clone)]
pub struct SecretSpec {
    pub name: String,
    pub kind: SecretKind,
    pub persistent: bool,
    pub key_id: Option<String>,
    pub secret: Option<String>,
    pub token: Option<String>,
    pub region: Option<String>,
    pub endpoint: Option<String>,
    pub url_style: Option<String>,
    pub use_ssl: Option<bool>,
    pub scope: Option<String>,
}

/// Pure, compiled secret registration plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretPlan {
    pub extensions: Vec<String>,
    pub sql: String,
    pub fallback_temp_sql: Option<String>,
}

/// Returns `true` only when `host` is an exact loopback address (`localhost`, `127.0.0.1`, `::1`, `[::1]`).
/// Strips optional port (`host:port`, `host,port`) and SQL Server instance suffix (`host\INSTANCE`)
/// before comparing, so hostnames like `notlocalhost.corp.com` never falsely match.
pub fn is_exact_loopback_host(host: &str) -> bool {
    let trimmed = host.trim();
    if trimmed.is_empty() {
        return false;
    }
    let bare = if let Some(rest) = trimmed.strip_prefix('[') {
        rest.split(']').next().unwrap_or("")
    } else if trimmed.matches(':').count() > 1 {
        // Bare IPv6 literal such as "::1"
        trimmed
    } else {
        let no_inst = trimmed.split('\\').next().unwrap_or(trimmed);
        let no_comma_port = no_inst.split(',').next().unwrap_or(no_inst);
        no_comma_port.split(':').next().unwrap_or(no_comma_port)
    };
    bare.eq_ignore_ascii_case("localhost") || bare == "127.0.0.1" || bare == "::1"
}

/// Normalizes a cloud bucket string to include a scheme prefix (`s3://` if none provided).
pub fn normalize_bucket_uri(bucket: &str) -> Option<String> {
    let trimmed = bucket.trim();
    if trimmed.is_empty() {
        return None;
    }
    let lower = trimmed.to_ascii_lowercase();
    if lower.starts_with("s3://")
        || lower.starts_with("r2://")
        || lower.starts_with("gcs://")
        || lower.starts_with("gs://")
        || lower.starts_with("az://")
        || lower.starts_with("azure://")
        || lower.starts_with("abfss://")
        || lower.starts_with("abfs://")
    {
        Some(trimmed.to_string())
    } else {
        Some(format!("s3://{}", trimmed))
    }
}

/// Formats a normalized `quack:<host>:<port>` connection endpoint (handling IPv6 brackets cleanly).
pub fn format_quack_endpoint(host_raw: &str, port_raw: &str) -> String {
    let trimmed = host_raw.trim();
    let stripped = trimmed
        .strip_prefix("quack://")
        .or_else(|| trimmed.strip_prefix("quack:"))
        .unwrap_or(trimmed);
    let port = if port_raw.trim().is_empty() {
        "9494"
    } else {
        port_raw.trim()
    };
    let endpoint = if stripped.starts_with('[') {
        if let Some(close_idx) = stripped.find(']') {
            let after = &stripped[close_idx + 1..];
            if after.starts_with(':') {
                stripped.to_string()
            } else {
                format!("{}:{}", stripped, port)
            }
        } else {
            stripped.to_string()
        }
    } else if stripped.matches(':').count() > 1 {
        format!("[{}]:{}", stripped, port)
    } else if stripped.contains(':') {
        stripped.to_string()
    } else {
        format!("{}:{}", stripped, port)
    };
    format!("quack:{}", endpoint)
}

pub fn escape_ado_value(val: &str) -> String {
    let trimmed = val.trim();
    if trimmed.contains(';') || trimmed.contains('{') || trimmed.contains('}') {
        format!("{{{}}}", trimmed.replace('}', "}}"))
    } else {
        trimmed.to_string()
    }
}

pub fn escape_libpq_value(val: &str) -> String {
    let trimmed = val.trim();
    if trimmed.chars().any(|c| c.is_whitespace()) || trimmed.contains('\'') || trimmed.contains('\\') {
        format!("'{}'", trimmed.replace('\\', "\\\\").replace('\'', "\\'"))
    } else {
        trimmed.to_string()
    }
}

/// Extracts the `Server=` / `Data Source=` host value from an ADO.NET connection string, if present.
fn extract_ado_server_host(conn_str: &str) -> Option<&str> {
    for part in conn_str.split(';') {
        if let Some((k, v)) = part.split_once('=') {
            let kl = k.trim().to_ascii_lowercase();
            if kl == "server" || kl == "data source" || kl == "addr" || kl == "address" {
                return Some(v.trim().trim_start_matches('{').trim_end_matches('}'));
            }
        }
    }
    None
}

/// Formats an MSSQL ADO.NET connection string (without the `mssql:` prefix) from individual fields.
#[allow(clippy::too_many_arguments)]
pub fn format_mssql_connection_string(
    host_raw: &str,
    port_raw: &str,
    database: &str,
    username: &str,
    password: &str,
    trusted_connection: bool,
    insecure: bool,
    disable_ssl: bool,
) -> String {
    let host_trimmed = host_raw.trim();
    // Preserve native mssql:// URIs untouched (do not wrap in Server=... or append ADO.NET semicolons)
    if host_trimmed.to_ascii_lowercase().starts_with("mssql://") {
        return host_trimmed.to_string();
    }
    let mut conn_str = if host_trimmed.contains('=') {
        host_trimmed.to_string()
    } else if host_trimmed.contains(',') {
        format!("Server={}", escape_ado_value(host_trimmed))
    } else if host_trimmed.contains('\\')
        && (port_raw.trim().is_empty() || port_raw.trim() == "1433")
    {
        // Named SQL Server instance (e.g. myhost\SQLEXPRESS): omit default port 1433 so SQL Server Browser resolves dynamic port
        format!("Server={}", escape_ado_value(host_trimmed))
    } else {
        let port = if port_raw.trim().is_empty() {
            "1433"
        } else {
            port_raw.trim()
        };
        format!("Server={},{}", escape_ado_value(host_trimmed), port)
    };
    if !database.trim().is_empty() && !conn_str.to_lowercase().contains("database=") {
        conn_str.push_str(&format!(";Database={}", escape_ado_value(database)));
    }
    let use_trusted = trusted_connection
        || (cfg!(target_os = "windows")
            && username.trim().is_empty()
            && password.trim().is_empty());

    if use_trusted {
        let lower = conn_str.to_lowercase();
        if !lower.contains("trusted_connection=")
            && !lower.contains("integrated security=")
            && !lower.contains("authenticator=")
        {
            conn_str.push_str(";Trusted_Connection=yes");
        }
    } else {
        let lower = conn_str.to_lowercase();
        if !username.trim().is_empty()
            && !lower.contains("user id=")
            && !lower.contains("uid=")
        {
            conn_str.push_str(&format!(";User Id={}", escape_ado_value(username)));
        }
        if !password.trim().is_empty()
            && !lower.contains("password=")
            && !lower.contains("pwd=")
        {
            conn_str.push_str(&format!(";Password={}", escape_ado_value(password)));
        }
    }

    // L5: Check exact loopback host equality rather than substring `.contains("localhost")`.
    let server_host = extract_ado_server_host(&conn_str).unwrap_or(host_trimmed);
    if (insecure || is_exact_loopback_host(server_host))
        && !conn_str.to_lowercase().contains("trustservercertificate=")
    {
        conn_str.push_str(";TrustServerCertificate=yes");
    }
    if disable_ssl && !conn_str.to_lowercase().contains("encrypt=") {
        conn_str.push_str(";Encrypt=no");
    }
    conn_str
}

/// Formats a PostgreSQL libpq connection string using `escape_libpq_value` (safe for passwords
/// containing `@`, `:`, `#`, `%`, spaces, or quotes), or preserves a caller-supplied URI/DSN.
pub fn format_postgres_conn_str(
    host_or_dsn: &str,
    port_raw: &str,
    database: &str,
    username: &str,
    password: &str,
    disable_ssl: bool,
    insecure: bool,
) -> String {
    let trimmed = host_or_dsn.trim();
    let mut conn_str = if trimmed.contains("://") || trimmed.contains('=') {
        trimmed.to_string()
    } else {
        let port = if port_raw.trim().is_empty() {
            "5432"
        } else {
            port_raw.trim()
        };
        let mut s = format!("host={} port={}", escape_libpq_value(trimmed), port);
        if !database.trim().is_empty() {
            s.push_str(&format!(" dbname={}", escape_libpq_value(database)));
        }
        if !username.trim().is_empty() {
            s.push_str(&format!(" user={}", escape_libpq_value(username)));
        }
        if !password.trim().is_empty() {
            s.push_str(&format!(" password={}", escape_libpq_value(password)));
        }
        s
    };

    if !conn_str.to_lowercase().contains("sslmode=") {
        if disable_ssl {
            let sep = if conn_str.contains("://") {
                if conn_str.contains('?') { "&" } else { "?" }
            } else {
                " "
            };
            conn_str.push_str(&format!("{}sslmode=disable", sep));
        } else if insecure {
            let sep = if conn_str.contains("://") {
                if conn_str.contains('?') { "&" } else { "?" }
            } else {
                " "
            };
            conn_str.push_str(&format!("{}sslmode=prefer", sep));
        }
    }
    conn_str
}

/// Formats a MySQL libpq-style connection string consistently across `workspace.rs` and `attach.rs`.
pub fn format_mysql_conn_str(
    host_or_dsn: &str,
    port_raw: &str,
    database: &str,
    username: &str,
    password: &str,
    disable_ssl: bool,
    insecure: bool,
) -> String {
    let trimmed = host_or_dsn.trim();
    let mut conn_str = if trimmed.contains('=') {
        trimmed.to_string()
    } else {
        let port = if port_raw.trim().is_empty() {
            "3306"
        } else {
            port_raw.trim()
        };
        let mut parts = vec![
            format!("host={}", escape_libpq_value(trimmed)),
            format!("port={}", port),
        ];
        if !database.trim().is_empty() {
            parts.push(format!("database={}", escape_libpq_value(database)));
        }
        if !username.trim().is_empty() {
            parts.push(format!("user={}", escape_libpq_value(username)));
        }
        if !password.trim().is_empty() {
            parts.push(format!("password={}", escape_libpq_value(password)));
        }
        parts.join(" ")
    };
    if (disable_ssl || insecure) && !conn_str.to_lowercase().contains("ssl_mode=") {
        conn_str.push_str(" ssl_mode=disabled");
    }
    conn_str
}

/// Formats a MotherDuck target (`md:<db>` with optional `?motherduck_token=...`).
pub fn format_motherduck_target(path_or_db: &str, token: Option<&str>) -> String {
    let raw = path_or_db.trim();
    let mut target = if raw.starts_with("md:") || raw.starts_with("motherduck:") {
        raw.to_string()
    } else {
        format!("md:{}", raw)
    };
    if let Some(tok) = token.map(|s| s.trim()).filter(|s| !s.is_empty()) {
        if !target.to_lowercase().contains("motherduck_token=") {
            let sep = if target.contains('?') { '&' } else { '?' };
            target.push_str(&format!("{}motherduck_token={}", sep, tok));
        }
    }
    target
}

/// Returns `true` if `path` is a remote HTTP(S) or cloud-storage URL for a frozen DuckLake file
/// (which DuckLake attaches as `ATTACH '<url>' (TYPE ducklake)` rather than `ducklake:<url>`).
pub fn is_remote_ducklake_url(path: &str) -> bool {
    let lower = path.trim().to_ascii_lowercase();
    lower.starts_with("http://")
        || lower.starts_with("https://")
        || lower.starts_with("s3://")
        || lower.starts_with("r2://")
        || lower.starts_with("gcs://")
        || lower.starts_with("gs://")
        || lower.starts_with("az://")
        || lower.starts_with("azure://")
        || lower.starts_with("abfss://")
        || lower.starts_with("abfs://")
}

/// Returns `true` if `s` references HTTP/S3/R2/GCS storage requiring the `httpfs` extension.
pub fn references_httpfs_storage(s: &str) -> bool {
    let l = s.to_ascii_lowercase();
    l.contains("s3://")
        || l.contains("r2://")
        || l.contains("gcs://")
        || l.contains("gs://")
        || l.contains("http://")
        || l.contains("https://")
}

/// Returns `true` if `s` references Azure Blob/ADLS storage requiring the `azure` extension.
pub fn references_azure_storage(s: &str) -> bool {
    let l = s.to_ascii_lowercase();
    l.contains("az://")
        || l.contains("azure://")
        || l.contains("abfss://")
        || l.contains("abfs://")
}

/// Formats a normalized `ducklake:<catalog_dsn>` target string for the selected metadata backend.
/// For remote/frozen DuckLake URLs (`https://...`, `s3://...`, etc.), returns the raw URL without
/// a `ducklake:` prefix so callers attach it with `(TYPE ducklake)`.
pub fn format_ducklake_target(
    backend: &str,
    host: &str,
    port: &str,
    database: &str,
    username: &str,
    password: &str,
    path: &str,
) -> std::result::Result<String, String> {
    let cat = match backend.to_ascii_lowercase().as_str() {
        "quack" => {
            if host.trim().is_empty() {
                return Err("Catalog host cannot be empty for DuckLake Quack backend.".to_string());
            }
            format_quack_endpoint(host, port)
        }
        "postgres" | "postgresql" => {
            if host.trim().is_empty() {
                return Err(
                    "Catalog host cannot be empty for DuckLake PostgreSQL backend.".to_string(),
                );
            }
            let host_raw = host.trim();
            if host_raw.starts_with("postgresql://") || host_raw.starts_with("postgres://") {
                format!("postgres:{}", host_raw)
            } else if host_raw.starts_with("postgres:") {
                host_raw.to_string()
            } else if host_raw.starts_with("postgresql:") {
                format!("postgres:{}", host_raw.trim_start_matches("postgresql:"))
            } else {
                let db = if database.trim().is_empty() {
                    "ducklake"
                } else {
                    database.trim()
                };
                format!(
                    "postgres:{}",
                    format_postgres_conn_str(host_raw, port, db, username, password, false, false)
                )
            }
        }
        "file" | "duckdb" | "local" => {
            let trimmed = path.trim();
            if trimmed.is_empty() {
                return Err(
                    "Metadata file path cannot be empty for DuckLake Local File backend."
                        .to_string(),
                );
            }
            if is_remote_ducklake_url(trimmed) {
                return Ok(trimmed.to_string());
            }
            trimmed.to_string()
        }
        "sqlite" => {
            if path.trim().is_empty() {
                return Err(
                    "SQLite metadata database path cannot be empty for DuckLake SQLite backend."
                        .to_string(),
                );
            }
            let p = path.trim();
            if p.starts_with("sqlite:") {
                p.to_string()
            } else {
                format!("sqlite:{}", p)
            }
        }
        other => other.to_string(),
    };

    if cat.starts_with("ducklake:") || is_remote_ducklake_url(&cat) {
        Ok(cat)
    } else {
        Ok(format!("ducklake:{}", cat))
    }
}

/// Extracts Cloudflare R2 `<account_id>` from an endpoint like `<account_id>.r2.cloudflarestorage.com`.
pub fn extract_r2_account_id(endpoint: &str) -> Option<String> {
    let clean = endpoint
        .trim()
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_end_matches('/');
    let suffix = ".r2.cloudflarestorage.com";
    if let Some(acct) = clean.strip_suffix(suffix) {
        let trimmed = acct.trim();
        if !trimmed.is_empty() && !trimmed.contains('<') && !trimmed.contains('/') {
            return Some(trimmed.to_string());
        }
    }
    None
}

/// Builds a native DuckDB v2.0 `TYPE R2` secret body (`KEY_ID`, `SECRET`, `ACCOUNT_ID`).
pub fn build_r2_secret_body(key_id: &str, secret_key: &str, account_id: &str) -> String {
    let mut clauses = vec!["TYPE R2".to_string()];
    if !key_id.trim().is_empty() {
        clauses.push(format!("KEY_ID {}", quote_literal(key_id.trim())));
    }
    if !secret_key.trim().is_empty() {
        clauses.push(format!("SECRET {}", quote_literal(secret_key.trim())));
    }
    if !account_id.trim().is_empty() {
        clauses.push(format!("ACCOUNT_ID {}", quote_literal(account_id.trim())));
    }
    clauses.join(", ")
}

/// Builds the inner `TYPE S3, KEY_ID ..., SECRET ..., REGION ..., ENDPOINT ..., URL_STYLE 'path', USE_SSL ...`
/// body for `CREATE SECRET (...)` in DuckDB.
pub fn build_s3_secret_body(
    key_id: &str,
    secret_key: &str,
    region: &str,
    endpoint: &str,
    use_ssl_default: bool,
) -> String {
    let mut clauses = vec!["TYPE S3".to_string()];
    if !key_id.trim().is_empty() {
        clauses.push(format!("KEY_ID {}", quote_literal(key_id.trim())));
    }
    if !secret_key.trim().is_empty() {
        clauses.push(format!("SECRET {}", quote_literal(secret_key.trim())));
    }
    if !region.trim().is_empty() {
        clauses.push(format!("REGION {}", quote_literal(region.trim())));
    }
    let mut use_ssl = use_ssl_default;
    let ep_trimmed = endpoint.trim();
    if !ep_trimmed.is_empty() {
        let clean_ep = ep_trimmed
            .trim_start_matches("http://")
            .trim_start_matches("https://")
            .trim_end_matches('/');
        clauses.push(format!("ENDPOINT {}", quote_literal(clean_ep)));
        clauses.push("URL_STYLE 'path'".to_string());
        if ep_trimmed.to_ascii_lowercase().starts_with("http://")
            || is_exact_loopback_host(clean_ep)
        {
            use_ssl = false;
        }
    }
    clauses.push(format!("USE_SSL {}", use_ssl));
    clauses.join(", ")
}

fn push_ext_unique(exts: &mut Vec<String>, ext: &str) {
    if !exts.iter().any(|e| e == ext) {
        exts.push(ext.to_string());
    }
}

fn resolve_local_path(raw: &str, base_dir: Option<&Path>) -> String {
    if raw == ":memory:" || Path::new(raw).is_absolute() {
        raw.to_string()
    } else if let Some(base) = base_dir {
        base.join(raw).to_string_lossy().to_string()
    } else {
        raw.to_string()
    }
}

/// Parses a legacy `ducklake:... (DATA_PATH '...', READ_ONLY)` string safely into structured fields
/// so raw untrusted option clauses are never concatenated verbatim into SQL (H1).
pub fn parse_ducklake_cli_string(raw_db: &str) -> (String, Option<String>, bool) {
    let trimmed = raw_db.trim();
    let Some(idx) = trimmed.find(" (") else {
        return (trimmed.to_string(), None, false);
    };
    let target = trimmed[..idx].trim().to_string();
    let opts_block = trimmed[idx + 2..].trim().trim_end_matches(')').trim();

    let mut data_path = None;
    let mut read_only = false;

    static DP_RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let dp_re = DP_RE.get_or_init(|| {
        regex::Regex::new(r"(?i)\bDATA_PATH\s+'((?:[^']|'')*)'").unwrap()
    });
    if let Some(caps) = dp_re.captures(opts_block) {
        if let Some(m) = caps.get(1) {
            data_path = Some(m.as_str().replace("''", "'"));
        }
    }
    if opts_block.to_ascii_uppercase().contains("READ_ONLY") {
        read_only = true;
    }

    (target, data_path, read_only)
}

/// Pure compiler: turns a normalized `AttachSpec` into an `AttachPlan` (extensions + SQL).
/// Performs ZERO I/O and is exhaustively unit-testable across all 8 providers.
pub fn plan_attach(spec: &AttachSpec) -> Result<AttachPlan> {
    let provider = spec
        .provider
        .ok_or_else(|| anyhow::anyhow!("AttachSpec requires a provider"))?;
    let alias = spec.alias.trim().to_string();
    if alias.is_empty() {
        anyhow::bail!("Attachment alias cannot be empty.");
    }
    let quoted_alias = quote_ident(&alias);
    let mut extensions = Vec::new();
    let mut opts = Vec::new();
    let mut needs_s3_env_fallback = false;

    let target_uri: String = match provider {
        AttachProviderKind::DuckDb => {
            let raw = spec
                .path
                .as_deref()
                .or(spec.database.as_deref())
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .unwrap_or(":memory:");
            resolve_local_path(raw, spec.base_dir.as_deref())
        }
        AttachProviderKind::Sqlite => {
            push_ext_unique(&mut extensions, "sqlite");
            opts.push("TYPE SQLITE".to_string());
            let raw = spec
                .path
                .as_deref()
                .or(spec.database.as_deref())
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .ok_or_else(|| anyhow::anyhow!("SQLite attachment requires a database file path."))?;
            resolve_local_path(raw, spec.base_dir.as_deref())
        }
        AttachProviderKind::Quack => {
            push_ext_unique(&mut extensions, "quack");
            let endpoint = if let Some(ref m) = spec.metadata.as_deref().filter(|s| !s.trim().is_empty()) {
                format_quack_endpoint(m, "")
            } else if let Some(ref p) = spec.path.as_deref().filter(|s| !s.trim().is_empty()) {
                format_quack_endpoint(p, "")
            } else if let Some(ref h) = spec.host.as_deref().filter(|s| !s.trim().is_empty()) {
                format_quack_endpoint(h, spec.port.as_deref().unwrap_or(""))
            } else {
                anyhow::bail!("Quack attachment requires 'metadata', 'path', or 'host'.");
            };
            let disable_ssl = should_disable_quack_ssl(&endpoint, spec.insecure || spec.disable_ssl);
            if disable_ssl {
                opts.push("DISABLE_SSL true".to_string());
            }
            if let Some(tok) = spec.token.as_deref().map(|s| s.trim()).filter(|s| !s.is_empty()) {
                opts.push(format!("TOKEN {}", quote_literal(tok)));
            }
            let effective_cid = spec
                .client_id
                .as_deref()
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .or_else(|| spec.token.as_deref().map(|s| s.trim()).filter(|s| !s.is_empty()));
            if let Some(cid) = effective_cid {
                opts.push(format!("CLIENT_ID {}", quote_literal(cid)));
            }
            endpoint
        }
        AttachProviderKind::DuckLake => {
            push_ext_unique(&mut extensions, "ducklake");
            let data_path = spec.data_path.as_deref().unwrap_or("").trim();
            if references_httpfs_storage(data_path) {
                push_ext_unique(&mut extensions, "httpfs");
                needs_s3_env_fallback = true;
            }
            if references_azure_storage(data_path) {
                push_ext_unique(&mut extensions, "azure");
            }

            let backend = spec.ducklake_backend.unwrap_or_else(|| {
                if let Some(m) = spec.metadata.as_deref().or(spec.path.as_deref()) {
                    let ml = m.to_ascii_lowercase();
                    if ml.contains("quack:") {
                        DuckLakeBackendKind::Quack
                    } else if ml.contains("postgres") {
                        DuckLakeBackendKind::Postgres
                    } else if ml.contains("sqlite") {
                        DuckLakeBackendKind::Sqlite
                    } else {
                        DuckLakeBackendKind::File
                    }
                } else if spec.host.is_some() {
                    if spec.port.as_deref().map(|p| p.trim()) == Some("5432") {
                        DuckLakeBackendKind::Postgres
                    } else {
                        DuckLakeBackendKind::Quack
                    }
                } else {
                    DuckLakeBackendKind::File
                }
            });

            let target = match backend {
                DuckLakeBackendKind::Quack => {
                    push_ext_unique(&mut extensions, "quack");
                    let ep = if let Some(ref m) = spec.metadata.as_deref().filter(|s| !s.trim().is_empty()) {
                        let stripped = m.trim().strip_prefix("ducklake:").unwrap_or(m.trim());
                        format_quack_endpoint(stripped, "")
                    } else if let Some(ref h) = spec.host.as_deref().filter(|s| !s.trim().is_empty()) {
                        format_quack_endpoint(h, spec.port.as_deref().unwrap_or(""))
                    } else {
                        anyhow::bail!("DuckLake Quack backend requires 'metadata' or 'host'.");
                    };
                    let disable_ssl =
                        should_disable_quack_ssl(&ep, spec.insecure || spec.disable_ssl);
                    if disable_ssl {
                        opts.push("META_DISABLE_SSL true".to_string());
                    }
                    if let Some(tok) = spec.token.as_deref().map(|s| s.trim()).filter(|s| !s.is_empty()) {
                        opts.push(format!("META_TOKEN {}", quote_literal(tok)));
                        let cid = spec
                            .client_id
                            .as_deref()
                            .map(|s| s.trim())
                            .filter(|s| s.chars().count() >= 4)
                            .unwrap_or(tok);
                        if cid.chars().count() >= 4 {
                            opts.push(format!("META_CLIENT_ID {}", quote_literal(cid)));
                        }
                    }
                    format!("ducklake:{}", ep)
                }
                DuckLakeBackendKind::Postgres => {
                    push_ext_unique(&mut extensions, "postgres");
                    if let Some(raw) = spec
                        .dsn
                        .as_deref()
                        .or(spec.metadata.as_deref())
                        .map(|s| s.trim())
                        .filter(|s| !s.is_empty())
                    {
                        let clean = raw.strip_prefix("ducklake:").unwrap_or(raw);
                        if clean.starts_with("postgresql://") || clean.starts_with("postgres://") {
                            format!("ducklake:postgres:{}", clean)
                        } else if clean.starts_with("postgres:") {
                            format!("ducklake:{}", clean)
                        } else if clean.starts_with("postgresql:") {
                            format!("ducklake:postgres:{}", clean.trim_start_matches("postgresql:"))
                        } else {
                            format!("ducklake:postgres:{}", clean)
                        }
                    } else {
                        let host = spec.host.as_deref().unwrap_or("localhost");
                        let port = spec.port.as_deref().unwrap_or("5432");
                        let db = spec.database.as_deref().unwrap_or("ducklake");
                        let user = spec.username.as_deref().unwrap_or("");
                        let pass = spec.password.as_deref().unwrap_or("");
                        format_ducklake_target("postgres", host, port, db, user, pass, "")
                            .map_err(|e| anyhow::anyhow!(e))?
                    }
                }
                DuckLakeBackendKind::Sqlite => {
                    push_ext_unique(&mut extensions, "sqlite");
                    let raw_p = spec
                        .metadata
                        .as_deref()
                        .or(spec.path.as_deref())
                        .unwrap_or("metadata.db")
                        .trim();
                    let no_dl = raw_p.strip_prefix("ducklake:").unwrap_or(raw_p);
                    let clean_p = no_dl.strip_prefix("sqlite:").unwrap_or(no_dl);
                    let resolved = resolve_local_path(clean_p, spec.base_dir.as_deref());
                    format!("ducklake:sqlite:{}", resolved)
                }
                DuckLakeBackendKind::File => {
                    let raw_p = spec
                        .metadata
                        .as_deref()
                        .or(spec.path.as_deref())
                        .unwrap_or("metadata.ducklake")
                        .trim();
                    let clean_p = raw_p.strip_prefix("ducklake:").unwrap_or(raw_p);
                    if is_remote_ducklake_url(clean_p) {
                        if references_azure_storage(clean_p) {
                            push_ext_unique(&mut extensions, "azure");
                        } else {
                            push_ext_unique(&mut extensions, "httpfs");
                            needs_s3_env_fallback = true;
                        }
                        clean_p.to_string()
                    } else {
                        let resolved = resolve_local_path(clean_p, spec.base_dir.as_deref());
                        format!("ducklake:{}", resolved)
                    }
                }
            };

            if !target.starts_with("ducklake:") {
                opts.insert(0, "TYPE ducklake".to_string());
            }
            if !data_path.is_empty() {
                opts.push(format!("DATA_PATH {}", quote_literal(data_path)));
            }
            target
        }
        AttachProviderKind::MotherDuck => {
            push_ext_unique(&mut extensions, "motherduck");
            let raw = spec
                .database
                .as_deref()
                .or(spec.path.as_deref())
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .ok_or_else(|| anyhow::anyhow!("MotherDuck database name cannot be empty."))?;
            let tok = spec.token.as_deref().or(spec.password.as_deref());
            format_motherduck_target(raw, tok)
        }
        AttachProviderKind::Postgres => {
            push_ext_unique(&mut extensions, "postgres");
            opts.push("TYPE POSTGRES".to_string());
            let host_or_dsn = spec
                .dsn
                .as_deref()
                .or(spec.host.as_deref())
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .unwrap_or("localhost");
            format_postgres_conn_str(
                host_or_dsn,
                spec.port.as_deref().unwrap_or("5432"),
                spec.database.as_deref().unwrap_or(""),
                spec.username.as_deref().unwrap_or(""),
                spec.password.as_deref().unwrap_or(""),
                spec.disable_ssl,
                spec.insecure,
            )
        }
        AttachProviderKind::Mysql => {
            push_ext_unique(&mut extensions, "mysql");
            opts.push("TYPE MYSQL".to_string());
            let host_or_dsn = spec
                .dsn
                .as_deref()
                .or(spec.host.as_deref())
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .unwrap_or("localhost");
            format_mysql_conn_str(
                host_or_dsn,
                spec.port.as_deref().unwrap_or("3306"),
                spec.database.as_deref().unwrap_or(""),
                spec.username.as_deref().unwrap_or(""),
                spec.password.as_deref().unwrap_or(""),
                spec.disable_ssl,
                spec.insecure,
            )
        }
        AttachProviderKind::Mssql => {
            push_ext_unique(&mut extensions, "mssql");
            opts.push("TYPE MSSQL".to_string());
            if let Some(dsn) = spec.dsn.as_deref().map(|s| s.trim()).filter(|s| !s.is_empty()) {
                dsn.to_string()
            } else {
                let host = spec
                    .host
                    .as_deref()
                    .map(|s| s.trim())
                    .filter(|s| !s.is_empty())
                    .unwrap_or("localhost");
                format_mssql_connection_string(
                    host,
                    spec.port.as_deref().unwrap_or("1433"),
                    spec.database.as_deref().unwrap_or(""),
                    spec.username.as_deref().unwrap_or(""),
                    spec.password.as_deref().unwrap_or(""),
                    spec.trusted_connection,
                    spec.insecure,
                    spec.disable_ssl,
                )
            }
        }
    };

    if spec.read_only {
        opts.push("READ_ONLY".to_string());
    }

    let attach_sql = if opts.is_empty() {
        format!("ATTACH {} AS {};", quote_literal(&target_uri), quoted_alias)
    } else {
        format!(
            "ATTACH {} AS {} ({});",
            quote_literal(&target_uri),
            quoted_alias,
            opts.join(", ")
        )
    };

    Ok(AttachPlan {
        extensions,
        target_uri,
        attach_sql,
        detach_first: spec.detach_first,
        alias,
        needs_s3_env_fallback,
    })
}

/// Executes a compiled `AttachPlan` against `conn`: loads extensions, optionally auto-registers
/// S3 env secret for DuckLake cloud storage, detaches stale alias if requested, and runs `ATTACH`.
pub fn execute_attach_plan(
    conn: &Connection,
    plan: &AttachPlan,
    project_dir: Option<&Path>,
) -> Result<()> {
    for ext in &plan.extensions {
        if ext == "quack" {
            load_quack_extension(conn)
                .with_context(|| "Failed to install/load quack extension")?;
        } else if ext == "azure" {
            let _ = ensure_extension_loaded(conn, "azure");
        } else {
            ensure_extension_loaded(conn, ext)
                .with_context(|| format!("Failed to install/load {} extension", ext))?;
        }
    }
    if plan.needs_s3_env_fallback {
        crate::engine::ensure_s3_secret_from_env(conn, project_dir);
    }
    if plan.detach_first {
        let _ = conn.execute(&format!("DETACH IF EXISTS {};", quote_ident(&plan.alias)), []);
    }
    conn.execute(&plan.attach_sql, [])
        .map_err(|e| anyhow::anyhow!("{}", redact_connection_credentials(&e.to_string())))?;
    Ok(())
}

/// Pure compiler: turns a `SecretSpec` into a `SecretPlan` (required extensions + SQL).
pub fn plan_secret(spec: &SecretSpec) -> Result<SecretPlan> {
    let quoted_name = quote_ident(spec.name.trim());
    let mut extensions = Vec::new();
    let body = match spec.kind {
        SecretKind::S3 | SecretKind::R2 => {
            push_ext_unique(&mut extensions, "httpfs");
            let key_id = spec.key_id.as_deref().unwrap_or("");
            let secret = spec.secret.as_deref().unwrap_or("");
            let endpoint = spec.endpoint.as_deref().unwrap_or("");
            let region = spec.region.as_deref().unwrap_or(if spec.kind == SecretKind::R2 {
                "auto"
            } else {
                "us-east-1"
            });
            let mut b = if spec.kind == SecretKind::R2 {
                if let Some(acct_id) = extract_r2_account_id(endpoint) {
                    build_r2_secret_body(key_id, secret, &acct_id)
                } else {
                    build_s3_secret_body(key_id, secret, region, endpoint, spec.use_ssl.unwrap_or(true))
                }
            } else {
                build_s3_secret_body(key_id, secret, region, endpoint, spec.use_ssl.unwrap_or(true))
            };
            if spec.kind != SecretKind::R2 || !b.starts_with("TYPE R2") {
                if let Some(style) = spec.url_style.as_deref().filter(|s| !s.trim().is_empty()) {
                    let style_clause = format!("URL_STYLE {}", quote_literal(style.trim()));
                    if b.contains("URL_STYLE 'path'") {
                        b = b.replace("URL_STYLE 'path'", &style_clause);
                    } else {
                        b.push_str(&format!(", {}", style_clause));
                    }
                }
            }
            if let Some(scope) = spec.scope.as_deref().filter(|s| !s.trim().is_empty()) {
                b.push_str(&format!(", SCOPE {}", quote_literal(scope.trim())));
            }
            b
        }
        SecretKind::Quack => {
            push_ext_unique(&mut extensions, "quack");
            let tok = spec
                .token
                .as_deref()
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .ok_or_else(|| anyhow::anyhow!("Quack secret '{}' requires a token.", spec.name))?;
            let mut clauses = vec![
                "TYPE QUACK".to_string(),
                format!("TOKEN {}", quote_literal(tok)),
            ];
            if let Some(scope) = spec.scope.as_deref().filter(|s| !s.trim().is_empty()) {
                clauses.push(format!("SCOPE {}", quote_literal(scope.trim())));
            }
            clauses.join(", ")
        }
        SecretKind::Gcs => {
            push_ext_unique(&mut extensions, "httpfs");
            let mut clauses = vec!["TYPE GCS".to_string()];
            if let Some(k) = spec.key_id.as_deref().filter(|s| !s.trim().is_empty()) {
                clauses.push(format!("KEY_ID {}", quote_literal(k.trim())));
            }
            if let Some(s) = spec.secret.as_deref().filter(|s| !s.trim().is_empty()) {
                clauses.push(format!("SECRET {}", quote_literal(s.trim())));
            }
            if let Some(r) = spec.region.as_deref().filter(|s| !s.trim().is_empty()) {
                clauses.push(format!("REGION {}", quote_literal(r.trim())));
            }
            if let Some(ep) = spec.endpoint.as_deref().filter(|s| !s.trim().is_empty()) {
                clauses.push(format!("ENDPOINT {}", quote_literal(ep.trim())));
            }
            if let Some(st) = spec.url_style.as_deref().filter(|s| !s.trim().is_empty()) {
                clauses.push(format!("URL_STYLE {}", quote_literal(st.trim())));
            }
            if let Some(ssl) = spec.use_ssl {
                clauses.push(format!("USE_SSL {}", ssl));
            }
            if let Some(scope) = spec.scope.as_deref().filter(|s| !s.trim().is_empty()) {
                clauses.push(format!("SCOPE {}", quote_literal(scope.trim())));
            }
            clauses.join(", ")
        }
        SecretKind::Azure => {
            push_ext_unique(&mut extensions, "azure");
            let mut clauses = vec!["TYPE AZURE".to_string()];
            if let Some(tok) = spec.token.as_deref().filter(|s| !s.trim().is_empty()) {
                clauses.push("PROVIDER access_token".to_string());
                clauses.push(format!("ACCESS_TOKEN {}", quote_literal(tok.trim())));
            } else if let Some(cs) = spec.secret.as_deref().filter(|s| !s.trim().is_empty()) {
                clauses.push(format!("CONNECTION_STRING {}", quote_literal(cs.trim())));
            } else {
                clauses.push("PROVIDER credential_chain".to_string());
            }
            if let Some(acct) = spec.key_id.as_deref().filter(|s| !s.trim().is_empty()) {
                clauses.push(format!("ACCOUNT_NAME {}", quote_literal(acct.trim())));
            }
            if let Some(ep) = spec.endpoint.as_deref().filter(|s| !s.trim().is_empty()) {
                clauses.push(format!("ENDPOINT {}", quote_literal(ep.trim())));
            }
            if let Some(scope) = spec.scope.as_deref().filter(|s| !s.trim().is_empty()) {
                clauses.push(format!("SCOPE {}", quote_literal(scope.trim())));
            }
            clauses.join(", ")
        }
        SecretKind::Http => {
            push_ext_unique(&mut extensions, "httpfs");
            let mut clauses = vec!["TYPE HTTP".to_string()];
            if let Some(tok) = spec.token.as_deref().filter(|s| !s.trim().is_empty()) {
                clauses.push(format!("BEARER_TOKEN {}", quote_literal(tok.trim())));
            }
            if let Some(scope) = spec.scope.as_deref().filter(|s| !s.trim().is_empty()) {
                clauses.push(format!("SCOPE {}", quote_literal(scope.trim())));
            }
            clauses.join(", ")
        }
    };

    let temp_sql = format!("CREATE OR REPLACE SECRET {} ({});", quoted_name, body);
    let (sql, fallback_temp_sql) = if spec.persistent {
        (
            format!("CREATE OR REPLACE PERSISTENT SECRET {} ({});", quoted_name, body),
            Some(temp_sql),
        )
    } else {
        (temp_sql, None)
    };

    Ok(SecretPlan {
        extensions,
        sql,
        fallback_temp_sql,
    })
}

/// Executes a compiled `SecretPlan` against `conn`.
pub fn execute_secret_plan(conn: &Connection, plan: &SecretPlan) -> Result<()> {
    for ext in &plan.extensions {
        if ext == "quack" {
            let _ = load_quack_extension(conn);
        } else {
            let _ = ensure_extension_loaded(conn, ext);
        }
    }
    let res = conn.execute(&plan.sql, []).or_else(|e| {
        if let Some(ref fb) = plan.fallback_temp_sql {
            conn.execute(fb, [])
        } else {
            Err(e)
        }
    });
    res.map_err(|e| anyhow::anyhow!("{}", redact_connection_credentials(&e.to_string())))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exact_loopback_host_prevents_substring_false_positives() {
        assert!(is_exact_loopback_host("localhost"));
        assert!(is_exact_loopback_host("LOCALHOST:1433"));
        assert!(is_exact_loopback_host("localhost\\SQLEXPRESS"));
        assert!(is_exact_loopback_host("127.0.0.1,1433"));
        assert!(is_exact_loopback_host("::1"));
        assert!(is_exact_loopback_host("[::1]:9494"));
        assert!(!is_exact_loopback_host("notlocalhost.corp.com"));
        assert!(!is_exact_loopback_host("my-localhost.internal"));
        assert!(!is_exact_loopback_host("10.0.0.1"));

        // L5: MSSQL TrustServerCertificate must NOT auto-enable for "notlocalhost.corp.com"
        let cs_remote = format_mssql_connection_string(
            "notlocalhost.corp.com",
            "1433",
            "sales",
            "sa",
            "pw",
            false,
            false,
            false,
        );
        assert!(!cs_remote.contains("TrustServerCertificate=yes"), "{cs_remote}");

        let cs_local = format_mssql_connection_string(
            "localhost",
            "1433",
            "sales",
            "sa",
            "pw",
            false,
            false,
            false,
        );
        assert!(cs_local.contains("TrustServerCertificate=yes"), "{cs_local}");
    }

    #[test]
    fn test_plan_attach_all_providers_and_special_char_escaping() {
        // 1. DuckLake on Quack uses native META_* options (ADR 0026)
        let dl_spec = AttachSpec {
            provider: Some(AttachProviderKind::DuckLake),
            alias: "sales_lake".to_string(),
            ducklake_backend: Some(DuckLakeBackendKind::Quack),
            host: Some("127.0.0.1".to_string()),
            port: Some("9494".to_string()),
            data_path: Some("s3://bucket/sales/".to_string()),
            token: Some("tok_secret_1234".to_string()),
            ..Default::default()
        };
        let dl_plan = plan_attach(&dl_spec).unwrap();
        assert!(dl_plan.extensions.contains(&"ducklake".to_string()));
        assert!(dl_plan.extensions.contains(&"quack".to_string()));
        assert!(dl_plan.extensions.contains(&"httpfs".to_string()));
        assert!(dl_plan.attach_sql.contains("META_DISABLE_SSL true"));
        assert!(dl_plan.attach_sql.contains("META_TOKEN 'tok_secret_1234'"));
        assert!(dl_plan.attach_sql.contains("META_CLIENT_ID 'tok_secret_1234'"));
        assert!(dl_plan.attach_sql.contains("DATA_PATH 's3://bucket/sales/'"));

        // 2. PostgreSQL with special characters (@ : # ' space) uses escaped libpq (L1)
        let pg_spec = AttachSpec {
            provider: Some(AttachProviderKind::Postgres),
            alias: "pg_db".to_string(),
            host: Some("pg.example.com".to_string()),
            port: Some("5432".to_string()),
            database: Some("app".to_string()),
            username: Some("admin".to_string()),
            password: Some("p@ss:w#rd' 1".to_string()),
            disable_ssl: true,
            ..Default::default()
        };
        let pg_plan = plan_attach(&pg_spec).unwrap();
        assert!(pg_plan.attach_sql.contains("TYPE POSTGRES"));
        assert!(pg_plan.attach_sql.contains("sslmode=disable"));
        assert!(!pg_plan.attach_sql.contains("postgres://"));

        // 3. MySQL honors both insecure and disable_ssl consistently
        let my_spec = AttachSpec {
            provider: Some(AttachProviderKind::Mysql),
            alias: "my_db".to_string(),
            host: Some("mysql.example.com".to_string()),
            insecure: true,
            ..Default::default()
        };
        let my_plan = plan_attach(&my_spec).unwrap();
        assert!(my_plan.attach_sql.contains("ssl_mode=disabled"));
        assert!(my_plan.attach_sql.contains("TYPE MYSQL"));

        // 4. Remote frozen DuckLake URL (including abfs://) uses TYPE ducklake
        let frozen_spec = AttachSpec {
            provider: Some(AttachProviderKind::DuckLake),
            alias: "frozen".to_string(),
            ducklake_backend: Some(DuckLakeBackendKind::File),
            path: Some("abfs://cont/lake.ducklake".to_string()),
            read_only: true,
            ..Default::default()
        };
        let frozen_plan = plan_attach(&frozen_spec).unwrap();
        assert!(frozen_plan.extensions.contains(&"azure".to_string()));
        assert!(frozen_plan.attach_sql.contains("TYPE ducklake"));
        assert!(frozen_plan.attach_sql.contains("READ_ONLY"));
    }

    #[test]
    fn test_plan_secret_r2_and_s3_and_azure() {
        let r2_plan = plan_secret(&SecretSpec {
            name: "r2_prod".to_string(),
            kind: SecretKind::R2,
            persistent: false,
            key_id: Some("kid".to_string()),
            secret: Some("sec'1".to_string()),
            token: None,
            region: None,
            endpoint: Some("acct123.r2.cloudflarestorage.com".to_string()),
            url_style: None,
            use_ssl: Some(true),
            scope: Some("r2://lake/".to_string()),
        })
        .unwrap();
        assert!(r2_plan.sql.contains("TYPE R2"));
        assert!(r2_plan.sql.contains("ACCOUNT_ID 'acct123'"));
        assert!(r2_plan.sql.contains("SECRET 'sec''1'"));
        assert!(r2_plan.sql.contains("SCOPE 'r2://lake/'"));

        let az_plan = plan_secret(&SecretSpec {
            name: "az_prod".to_string(),
            kind: SecretKind::Azure,
            persistent: false,
            key_id: Some("myacct".to_string()),
            secret: None,
            token: Some("tok_az".to_string()),
            region: None,
            endpoint: None,
            url_style: None,
            use_ssl: None,
            scope: None,
        })
        .unwrap();
        assert!(az_plan.extensions.contains(&"azure".to_string()));
        assert!(az_plan.sql.contains("PROVIDER access_token"));
        assert!(az_plan.sql.contains("ACCESS_TOKEN 'tok_az'"));
    }
}
