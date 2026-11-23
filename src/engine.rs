use anyhow::{Context, Result};
use duckdb::Connection;
use std::fs;
use std::path::{Path, PathBuf};

use crate::diff;

/// Common database execution trait to allow DuckPack to run against either
/// native Rust `duckdb::Connection` or foreign connections (such as Python `DuckDBPyConnection`).
pub trait DbExecutor {
    fn execute(&mut self, sql: &str) -> Result<()>;
    fn execute_batch(&mut self, sql: &str) -> Result<()>;
    fn query_rows(&mut self, sql: &str) -> Result<(Vec<String>, Vec<Vec<String>>)>;
}

impl DbExecutor for Connection {
    fn execute(&mut self, sql: &str) -> Result<()> {
        <&Connection as DbExecutor>::execute(&mut &*self, sql)
    }

    fn execute_batch(&mut self, sql: &str) -> Result<()> {
        <&Connection as DbExecutor>::execute_batch(&mut &*self, sql)
    }

    fn query_rows(&mut self, sql: &str) -> Result<(Vec<String>, Vec<Vec<String>>)> {
        <&Connection as DbExecutor>::query_rows(&mut &*self, sql)
    }
}

impl DbExecutor for &Connection {
    fn execute(&mut self, sql: &str) -> Result<()> {
        Connection::execute(*self, sql, [])?;
        Ok(())
    }

    fn execute_batch(&mut self, sql: &str) -> Result<()> {
        Connection::execute_batch(*self, sql)?;
        Ok(())
    }

    fn query_rows(&mut self, sql: &str) -> Result<(Vec<String>, Vec<Vec<String>>)> {
        let mut stmt = self.prepare(sql)?;
        let mut rows = stmt.query([])?;
        let mut column_names = Vec::new();
        if let Some(stmt_ref) = rows.as_ref() {
            for col in stmt_ref.column_names() {
                column_names.push(col.to_string());
            }
        }
        let column_count = column_names.len();

        let mut data = Vec::new();
        while let Some(row) = rows.next()? {
            let mut formatted_row = Vec::new();
            for i in 0..column_count {
                let val: Result<duckdb::types::Value, _> = row.get(i);
                match val {
                    Ok(v) => formatted_row.push(format_duckdb_value(&v)),
                    Err(_) => formatted_row.push("NULL".to_string()),
                }
            }
            data.push(formatted_row);
        }

        Ok((column_names, data))
    }
}

fn format_duckdb_value(val: &duckdb::types::Value) -> String {
    use duckdb::types::Value;
    match val {
        Value::Null => "NULL".to_string(),
        Value::Boolean(b) => b.to_string(),
        Value::TinyInt(i) => i.to_string(),
        Value::SmallInt(i) => i.to_string(),
        Value::Int(i) => i.to_string(),
        Value::BigInt(i) => i.to_string(),
        Value::HugeInt(i) => i.to_string(),
        Value::Float(f) => f.to_string(),
        Value::Double(d) => d.to_string(),
        Value::Text(t) => t.clone(),
        Value::Blob(b) => format!("<blob {}B>", b.len()),
        Value::Timestamp(unit, val) => {
            let timestamp_secs = match unit {
                duckdb::types::TimeUnit::Second => *val,
                duckdb::types::TimeUnit::Millisecond => *val / 1_000,
                duckdb::types::TimeUnit::Microsecond => *val / 1_000_000,
                duckdb::types::TimeUnit::Nanosecond => *val / 1_000_000_000,
            };
            let timestamp_nanos = match unit {
                duckdb::types::TimeUnit::Second => 0,
                duckdb::types::TimeUnit::Millisecond => (*val % 1_000) * 1_000_000,
                duckdb::types::TimeUnit::Microsecond => (*val % 1_000_000) * 1_000,
                duckdb::types::TimeUnit::Nanosecond => *val % 1_000_000_000,
            };
            if let Some(dt) =
                chrono::DateTime::from_timestamp(timestamp_secs, timestamp_nanos as u32)
            {
                dt.naive_utc().format("%Y-%m-%d %H:%M:%S").to_string()
            } else {
                format!("{}", val)
            }
        }
        Value::Date32(days) => {
            if let Some(dt) = chrono::DateTime::from_timestamp((*days as i64) * 86400, 0) {
                dt.naive_utc().format("%Y-%m-%d").to_string()
            } else {
                format!("{}", days)
            }
        }
        Value::Time64(unit, val) => format!("{} {:?}", val, unit),
        Value::Interval {
            months,
            days,
            nanos,
        } => format!("{} months {} days {} nanos", months, days, nanos),
        Value::Decimal(d) => d.to_string(),
        other => format!("{:?}", other),
    }
}

/// Determines if the provided database string refers to a remote motherduck instance.
pub fn is_remote(db: &str) -> bool {
    db.starts_with("md:") || db.starts_with("motherduck:")
}

/// Checks if a remote Quack database is attached as `target_db` on the given `DbExecutor`.
pub fn is_quack_target_attached(exec: &mut dyn DbExecutor) -> bool {
    exec.query_rows(
        "SELECT count(*) FROM duckdb_databases() WHERE database_name = 'target_db' AND (lower(type) = 'quack' OR (path LIKE 'quack:%' AND lower(type) != 'ducklake'))",
    )
    .ok()
    .and_then(|(_, rows)| rows.into_iter().next())
    .and_then(|r| r.into_iter().next())
    .and_then(|v| v.parse::<i64>().ok())
    .unwrap_or(0)
        > 0
}

fn extract_quack_host(db: &str) -> &str {
    let stripped = db
        .trim()
        .strip_prefix("quack://")
        .or_else(|| db.trim().strip_prefix("quack:"))
        .unwrap_or(db.trim());
    let without_path = stripped.split('/').next().unwrap_or(stripped);
    if without_path.starts_with('[') {
        if let Some(end) = without_path.find(']') {
            return &without_path[..=end];
        }
        return without_path;
    }
    without_path.split(':').next().unwrap_or(without_path)
}

/// Determines if the provided quack connection URI targets the 0.0.0.0 local unspecified address.
pub fn is_quack_zero_host(db: &str) -> bool {
    let host = extract_quack_host(db);
    host == "0.0.0.0" || host == "[::]"
}

/// Determines if the provided quack connection URI targets a loopback address (127.0.0.1, localhost, [::1]).
pub fn is_quack_loopback(db: &str) -> bool {
    let host = extract_quack_host(db);
    host.eq_ignore_ascii_case("localhost") || host == "127.0.0.1" || host == "[::1]"
}

/// Determines if SSL should be disabled for a Quack connection.
/// Returns true if explicitly flagged, if DUCKDB_QUACK_DISABLE_SSL is set, or if connecting to 0.0.0.0 or loopback.
pub fn should_disable_quack_ssl(db: &str, flag_disable_ssl: bool) -> bool {
    flag_disable_ssl
        || std::env::var("DUCKDB_QUACK_DISABLE_SSL")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true") || v.eq_ignore_ascii_case("yes"))
            .unwrap_or(false)
        || is_quack_zero_host(db)
        || is_quack_loopback(db)
}

/// Resolves the effective Quack client ID for RBAC role correlation:
/// uses `QUACK_CLIENT_ID` from the environment if set, otherwise defaults to the active auth token.
pub fn resolve_quack_client_id(token: Option<&str>) -> Option<String> {
    if let Ok(cid) = std::env::var("QUACK_CLIENT_ID") {
        let trimmed = cid.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string());
        }
    }
    token
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
}

/// Builds the SQL ATTACH statement for a Quack database, optionally embedding an explicit `TOKEN '...'`
/// and `CLIENT_ID '...'` so persistent secrets in `~/.duckdb/stored_secrets` never shadow the active token
/// and the server's `quack_active_connections().client_id_hash` correlates deterministically.
pub fn build_quack_attach_sql_with_token(
    db: &str,
    disable_ssl: bool,
    token: Option<&str>,
) -> String {
    let safe_db = db.replace('\'', "''");
    let mut opts = Vec::new();
    if disable_ssl {
        opts.push("DISABLE_SSL true".to_string());
    }
    if let Some(tok) = token.map(|s| s.trim()).filter(|s| !s.is_empty()) {
        opts.push(format!("TOKEN '{}'", tok.replace('\'', "''")));
    }
    if let Some(cid) = resolve_quack_client_id(token) {
        opts.push(format!("CLIENT_ID '{}'", cid.replace('\'', "''")));
    }
    if opts.is_empty() {
        format!("ATTACH '{}' AS target_db;", safe_db)
    } else {
        format!("ATTACH '{}' AS target_db ({});", safe_db, opts.join(", "))
    }
}

/// Builds the SQL ATTACH statement for a Quack database.
pub fn build_quack_attach_sql(db: &str, disable_ssl: bool) -> String {
    build_quack_attach_sql_with_token(db, disable_ssl, None)
}

/// Returns the display version string of DuckPack (e.g. "0.2.1_dev1").
/// Converts Cargo's SemVer pre-release hyphen separator (`0.2.1-dev1`) to an underscore (`0.2.1_dev1`).
pub fn duckpack_version() -> String {
    env!("CARGO_PKG_VERSION").replace('-', "_")
}

/// Returns the version string of the embedded DuckDB C++ engine (e.g. "v1.1.3").
pub fn embedded_duckdb_version() -> String {
    Connection::open_in_memory()
        .and_then(|c| c.query_row("SELECT version();", [], |r| r.get::<_, String>(0)))
        .unwrap_or_else(|_| "1.1.x".to_string())
}

/// Diagnoses DuckDB storage version mismatch errors when attempting to open a database file.
pub fn diagnose_storage_version_error(err_str: &str, db_path: &str) -> Option<String> {
    let lower = err_str.to_lowercase();
    if lower.contains("storage version")
        || lower.contains("serialization error")
        || lower.contains("different version of duckdb")
        || lower.contains("created by a newer version")
        || lower.contains("version number 69")
        || lower.contains("versions between")
        || (lower.contains("version number") && lower.contains("database file"))
        || (lower.contains("cannot open database") && lower.contains("version"))
    {
        Some(format!(
            "DuckDB Storage Version Mismatch: The database file '{}' appears to have been created or modified by a newer or different version of DuckDB than DuckPack's embedded engine ({}).\n\n\
            Remediation Options:\n\
              1. Upgrade DuckPack: Run `duckpack python install --upgrade` or download the latest CLI release.\n\
              2. Compatibility Setting: In your DuckDB client, configure storage compatibility before writing:\n\
                 SET storage_compatibility_version = 'v1.2.0';\n\
              3. Python One-Liner (Auto-Bridged): Run using your active Python environment's DuckDB:\n\
                 $ python -c \"import duckdb, duckpack; con = duckdb.connect('{}'); duckpack.explore(con)\"\n\
              4. Serve via your local DuckDB CLI:\n\
                 $ duckdb {} -serve\n\
                 $ duckpack explore --db quack:localhost:9494",
            db_path,
            embedded_duckdb_version(),
            db_path,
            db_path
        ))
    } else {
        None
    }
}

/// Configures DuckDB connection settings to allow unsigned extensions, community extensions,
/// and metadata version mismatches (e.g. loading v2.0.0 extensions into dev/hash builds).
pub fn create_duckdb_config() -> Result<duckdb::Config> {
    let config = duckdb::Config::default()
        .allow_unsigned_extensions()?
        .with("allow_extensions_metadata_mismatch", "true")?
        .with("allow_community_extensions", "true")?;
    Ok(config)
}

/// Opens an in-memory DuckDB connection configured with extension support flags.
pub fn open_duckdb_connection() -> Result<Connection> {
    if let Ok(cfg) = create_duckdb_config() {
        if let Ok(conn) = Connection::open_in_memory_with_flags(cfg) {
            return Ok(conn);
        }
    }
    Connection::open_in_memory().context("Failed to open in-memory DuckDB connection")
}

/// Returns the release version tag (e.g. "v2.0.0", "v1.2.0") and optional commit hash of the active DuckDB connection.
pub fn get_duckdb_version_info(conn: &Connection) -> (String, Option<String>) {
    if let Ok((ver, hash)) = conn.query_row("PRAGMA version;", [], |r| {
        let v: String = r.get(0)?;
        let h: String = r.get(1)?;
        Ok((v, h))
    }) {
        let clean_ver = ver.split('-').next().unwrap_or(&ver).to_string();
        let release_tag = if clean_ver.starts_with('v') {
            clean_ver
        } else {
            format!("v{}", clean_ver)
        };
        let commit_hash = if !hash.is_empty() { Some(hash) } else { None };
        (release_tag, commit_hash)
    } else {
        ("v2.0.0".to_string(), None)
    }
}

/// Loads any DuckDB extension, gracefully handling local version path mismatches
/// (e.g. ~/.duckdb/extensions/v2.0.0 vs dev commit hashes) and automatically installing
/// from the official release CDN repository if DuckDB's internal autoload returns HTTP 404.
pub fn ensure_extension_loaded(conn: &Connection, ext_name: &str) -> Result<()> {
    crate::sql_utils::validate_safe_extension_name(ext_name)?;
    let res = ensure_extension_loaded_inner(conn, ext_name);
    if res.is_ok() && ext_name.eq_ignore_ascii_case("mssql") {
        let _ = conn.execute("SET mssql_exec_invalidate_cache = true;", []);
    }
    res
}

fn ensure_extension_loaded_inner(conn: &Connection, ext_name: &str) -> Result<()> {
    // 1. Try standard LOAD <ext>;
    if conn.execute_batch(&format!("LOAD {};", ext_name)).is_ok() {
        return Ok(());
    }

    let (release_tag, commit_hash) = get_duckdb_version_info(conn);
    let hash_str = commit_hash.as_deref().unwrap_or("");

    let platform = if cfg!(target_os = "macos") {
        if cfg!(target_arch = "aarch64") {
            "osx_arm64"
        } else {
            "osx_amd64"
        }
    } else if cfg!(target_os = "windows") {
        "windows_amd64"
    } else {
        if cfg!(target_arch = "aarch64") {
            "linux_arm64"
        } else {
            "linux_amd64"
        }
    };

    // 2. Search local extension directory (~/.duckdb/extensions) ONLY in version-compatible folders:
    //    First check hash_str (e.g. d54cbbe3df), then release_tag (e.g. v2.0.0), then "v2.0.0"
    if let Ok(home) = std::env::var("HOME") {
        let base_ext_dir = PathBuf::from(home).join(".duckdb").join("extensions");
        let target_filename = format!("{}.duckdb_extension", ext_name);
        let info_filename = format!("{}.duckdb_extension.info", ext_name);

        let mut candidate_folders = Vec::new();
        if !hash_str.is_empty() && hash_str != "v2.0.0" {
            candidate_folders.push(base_ext_dir.join(hash_str).join(platform));
        }
        candidate_folders.push(base_ext_dir.join(&release_tag).join(platform));
        if release_tag != "v2.0.0" {
            candidate_folders.push(base_ext_dir.join("v2.0.0").join(platform));
        }

        for folder in &candidate_folders {
            let candidate_file = folder.join(&target_filename);
            if candidate_file.exists() {
                // If a corrupted or 0-byte file exists from an interrupted download, purge it
                if let Ok(meta) = candidate_file.metadata() {
                    if meta.len() < 512 {
                        let _ = fs::remove_file(&candidate_file);
                        let _ = fs::remove_file(folder.join(&info_filename));
                        continue;
                    }
                }

                // If present in v2.0.0 or release folder, sync it to the dev hash directory if distinct
                if !hash_str.is_empty() && hash_str != "v2.0.0" {
                    let dest_dir = base_ext_dir.join(hash_str).join(platform);
                    if folder != &dest_dir {
                        let _ = fs::create_dir_all(&dest_dir);
                        let _ = fs::copy(&candidate_file, dest_dir.join(&target_filename));
                        let src_info = folder.join(&info_filename);
                        if src_info.exists() {
                            let _ = fs::copy(&src_info, dest_dir.join(&info_filename));
                        }
                    }
                }

                let load_sql = format!(
                    "LOAD '{}';",
                    candidate_file.to_string_lossy().replace('\'', "''")
                );
                if conn.execute(&load_sql, []).is_ok()
                    || conn.execute_batch(&format!("LOAD {};", ext_name)).is_ok()
                {
                    return Ok(());
                }
            }
        }
    }

    // 3. Fallback to direct URL install from official DuckDB CDN and Community Extensions CDN
    //    Always prioritize "v2.0.0" because DuckDB release extensions are published under v2.0.0
    let mut versions = vec!["v2.0.0".to_string()];
    if release_tag != "v2.0.0" {
        versions.push(release_tag.clone());
    }

    let mut candidate_urls = Vec::new();
    for ver in &versions {
        // Official Core CDN (e.g. quack, httpfs, parquet, sqlite, postgres, mysql)
        candidate_urls.push(format!(
            "https://extensions.duckdb.org/{}/{}/{}.duckdb_extension.gz",
            ver, platform, ext_name
        ));
        // Community Extensions CDN (e.g. mssql, etc.)
        candidate_urls.push(format!(
            "https://community-extensions.duckdb.org/{}/{}/{}.duckdb_extension.gz",
            ver, platform, ext_name
        ));
    }

    for cdn_url in &candidate_urls {
        let install_res = conn
            .execute(&format!("FORCE INSTALL '{}';", cdn_url), [])
            .or_else(|_| conn.execute(&format!("INSTALL '{}';", cdn_url), []));

        if install_res.is_ok() {
            // Verify if the extension file on disk was installed and is not 0 bytes
            let mut valid_file_found = false;
            if let Ok(home) = std::env::var("HOME") {
                let base_ext_dir = PathBuf::from(&home).join(".duckdb").join("extensions");
                let target_filename = format!("{}.duckdb_extension", ext_name);
                let info_filename = format!("{}.duckdb_extension.info", ext_name);

                for ver in &versions {
                    let ext_file = base_ext_dir.join(ver).join(platform).join(&target_filename);
                    if ext_file.exists() {
                        if ext_file.metadata().map(|m| m.len()).unwrap_or(0) >= 512 {
                            valid_file_found = true;

                            // Only replicate if destination directory is distinct from source
                            if !hash_str.is_empty() && hash_str != "v2.0.0" && hash_str != ver.as_str() {
                                let dest_dir = base_ext_dir.join(hash_str).join(platform);
                                let _ = fs::create_dir_all(&dest_dir);
                                let _ = fs::copy(&ext_file, dest_dir.join(&target_filename));
                                let src_info = base_ext_dir.join(ver).join(platform).join(&info_filename);
                                if src_info.exists() {
                                    let _ = fs::copy(&src_info, dest_dir.join(&info_filename));
                                }
                            }
                            break;
                        } else {
                            // File is corrupted or 0-byte (e.g. 404 response): delete it immediately
                            let _ = fs::remove_file(&ext_file);
                            let _ = fs::remove_file(base_ext_dir.join(ver).join(platform).join(&info_filename));
                        }
                    }
                }
            }

            if !valid_file_found {
                continue;
            }

            // Load via standard name, then fallback to direct path
            let load_res = conn
                .execute(&format!("LOAD {};", ext_name), [])
                .or_else(|_| {
                    if let Ok(home) = std::env::var("HOME") {
                        let base_ext_dir = PathBuf::from(&home).join(".duckdb").join("extensions");
                        let target_filename = format!("{}.duckdb_extension", ext_name);

                        let mut paths = Vec::new();
                        if !hash_str.is_empty() && hash_str != "v2.0.0" {
                            paths.push(base_ext_dir.join(hash_str).join(platform).join(&target_filename));
                        }
                        paths.push(base_ext_dir.join("v2.0.0").join(platform).join(&target_filename));

                        for p in &paths {
                            if p.exists() && p.metadata().map(|m| m.len()).unwrap_or(0) >= 512 {
                                let sql = format!("LOAD '{}';", p.to_string_lossy().replace('\'', "''"));
                                if conn.execute(&sql, []).is_ok() {
                                    return Ok(0);
                                }
                            }
                        }
                    }
                    Err(duckdb::Error::QueryReturnedNoRows)
                });

            if load_res.is_ok() {
                return Ok(());
            }
        }
    }

    // 4. Final fallback to standard INSTALL/LOAD
    conn.execute_batch(&format!("INSTALL {}; LOAD {};", ext_name, ext_name))
        .or_else(|_| {
            conn.execute_batch(&format!(
                "INSTALL {} FROM community; LOAD {};",
                ext_name, ext_name
            ))
        })
        .context(format!("Failed to install/load {} extension", ext_name))?;

    Ok(())
}

/// Loads the `quack` extension, gracefully handling local version path mismatches
/// before falling back to network install.
pub fn load_quack_extension(conn: &Connection) -> Result<()> {
    ensure_extension_loaded(conn, "quack")
}

pub use crate::sql_utils::{redact_connection_credentials, should_redact_sql_history};

/// Cleans an explicit Quack token input (trimming surrounding quotes and expanding `$VAR`/`${VAR}` or
/// uppercase env var names) and falls back to `resolve_quack_token(None, None)` when empty.
/// Pure helper — does NOT mutate `conn` secrets or global `quack_default_client_id`.
pub fn resolve_explicit_quack_token(explicit_token: Option<&str>) -> Option<String> {
    let cleaned_explicit = explicit_token
        .map(|s| {
            let t = s.trim();
            let unquoted = if (t.starts_with('\'') && t.ends_with('\'') && t.len() >= 2)
                || (t.starts_with('"') && t.ends_with('"') && t.len() >= 2)
            {
                &t[1..t.len() - 1]
            } else {
                t
            };
            let var_candidate = unquoted
                .strip_prefix("${")
                .and_then(|v| v.strip_suffix('}'))
                .or_else(|| unquoted.strip_prefix('$'))
                .unwrap_or(unquoted)
                .trim();
            if !var_candidate.is_empty()
                && var_candidate
                    .chars()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
                && var_candidate.chars().any(|c| c.is_ascii_uppercase())
            {
                if let Ok(env_val) = std::env::var(var_candidate) {
                    let ev = env_val.trim();
                    if !ev.is_empty() {
                        return ev.to_string();
                    }
                }
            }
            unquoted.trim().to_string()
        })
        .filter(|s| !s.is_empty());

    cleaned_explicit.or_else(|| resolve_quack_token(None, None))
}

/// Applies a Quack authentication token to `conn` using `CREATE OR REPLACE SECRET (TYPE QUACK, TOKEN '...')`.
/// Drops any stale temporary `quack` secrets first so a previously failed or empty token never shadows the new token.
/// If `explicit_token` is `None` or empty, falls back to `resolve_quack_token(None, None)` (`QUACK_TOKEN` / `DUCKDB_QUACK_TOKEN`).
pub fn apply_quack_token_secret(
    conn: &Connection,
    explicit_token: Option<&str>,
) -> Result<Option<String>> {
    let resolved = resolve_explicit_quack_token(explicit_token);
    if let Some(ref token) = resolved {
        // Drop any existing temporary quack secrets so stale tokens cannot shadow the new secret
        let mut stale_names: Vec<String> = Vec::new();
        if let Ok(mut stmt) = conn.prepare(
            "SELECT name FROM duckdb_secrets() WHERE lower(type) = 'quack' AND persistent = false",
        ) {
            if let Ok(mut rows) = stmt.query([]) {
                while let Ok(Some(row)) = rows.next() {
                    if let Ok(name) = row.get::<_, String>(0) {
                        stale_names.push(name);
                    }
                }
            }
        }
        for name in stale_names {
            let safe_name = name.replace('"', "\"\"");
            let _ = conn.execute(
                &format!("DROP TEMPORARY SECRET IF EXISTS \"{}\";", safe_name),
                [],
            );
            let _ = conn.execute(&format!("DROP SECRET IF EXISTS \"{}\";", safe_name), []);
        }

        let safe_token = token.replace('\'', "''");
        let effective_cid = resolve_quack_client_id(Some(token)).unwrap_or_else(|| token.clone());
        let safe_cid = effective_cid.replace('\'', "''");
        // Synchronize quack_default_client_id (both global and session) with the resolved client ID
        // so quack_active_connections().client_id_hash correlates deterministically on the server.
        let _ = conn.execute(
            &format!("SET GLOBAL quack_default_client_id = '{}';", safe_cid),
            [],
        );
        let _ = conn.execute(
            &format!("SET quack_default_client_id = '{}';", safe_cid),
            [],
        );
        conn.execute(
            &format!("CREATE OR REPLACE SECRET (TYPE QUACK, TOKEN '{}');", safe_token),
            [],
        )
        .or_else(|_| {
            conn.execute(
                &format!("CREATE SECRET (TYPE QUACK, TOKEN '{}');", safe_token),
                [],
            )
        })
        .with_context(|| "Failed to set quack auth secret")?;
    }
    Ok(resolved)
}

/// Connects to the target DuckDB database (local, motherduck remote, or quack remote).
pub fn connect_target_db(
    db: &str,
    quack_token: Option<&str>,
    quack_disable_ssl: bool,
) -> Result<Connection> {
    if db.starts_with("quack:") {
        let conn = open_duckdb_connection()
            .with_context(|| "Failed to open in-memory DB for remote ATTACH")?;
        load_quack_extension(&conn).with_context(|| {
            format!("Failed to install/load quack extension for target '{}'", db)
        })?;

        let applied_token = apply_quack_token_secret(&conn, quack_token)?;

        let disable_ssl = should_disable_quack_ssl(db, quack_disable_ssl);
        let attach_sql =
            build_quack_attach_sql_with_token(db, disable_ssl, applied_token.as_deref());

        let attach_res = conn.execute(&attach_sql, []);
        if let Err(e) = attach_res {
            let safe_db = redact_connection_credentials(db);
            let err_msg = redact_connection_credentials(&e.to_string());
            if err_msg.contains("Could not find a Quack authentication token")
                || err_msg.contains("Authentication failed")
            {
                return Err(anyhow::anyhow!(
                    "Failed to attach remote database '{}': {}\n\n\
                    Troubleshooting:\n\
                      1. Check that your Quack token is set in .env (QUACK_TOKEN) or duckpack.toml (token_env).\n\
                      2. If using --env <profile>, ensure either .env or .env.<profile> contains the token.\n\
                      3. Alternatively, pass --quack-token <token> directly.",
                    safe_db,
                    err_msg
                ));
            }
            return Err(anyhow::anyhow!(
                "Failed to attach remote database '{}': {}",
                safe_db,
                err_msg
            ));
        }
        // For Quack connections in DuckDB v2, keep 'memory' as the default session catalog
        // so that local prepared statements register correctly, while target_db is attached
        // and accessible via 'target_db' qualifier or target_db.query().
        // Configure search_path so unqualified queries resolve to target_db.main first,
        // while local temporary tables and session tables created in main/memory resolve seamlessly.
        let _ = conn.execute("SET search_path = 'target_db.main,main';", []);
        Ok(conn)
    } else if db.starts_with("mssql:") {
        let conn = open_duckdb_connection()
            .with_context(|| "Failed to open in-memory DB for MSSQL ATTACH")?;
        ensure_extension_loaded(&conn, "mssql")
            .with_context(|| "Failed to install/load mssql extension")?;
        let is_mssql_uri = db.to_ascii_lowercase().starts_with("mssql://");
        let conn_str = if is_mssql_uri {
            db
        } else {
            db.strip_prefix("mssql:").unwrap_or(db)
        };
        let mut final_conn_str = conn_str.to_string();
        if !is_mssql_uri
            && cfg!(target_os = "windows")
            && !final_conn_str.to_lowercase().contains("trusted_connection=")
            && !final_conn_str.to_lowercase().contains("integrated security=")
            && !final_conn_str.to_lowercase().contains("authenticator=")
            && !final_conn_str.to_lowercase().contains("user id=")
            && !final_conn_str.to_lowercase().contains("uid=")
        {
            final_conn_str.push_str(";Trusted_Connection=yes");
        }
        let safe_conn_str = final_conn_str.replace('\'', "''");
        let attach_res = conn.execute(
            &format!("ATTACH '{}' AS target_db (TYPE MSSQL);", safe_conn_str),
            [],
        );
        if let Err(e) = attach_res {
            return Err(anyhow::anyhow!(
                "Failed to attach MSSQL database '{}': {}",
                redact_connection_credentials(db),
                redact_connection_credentials(&e.to_string())
            ));
        }
        if conn.execute("CONNECT target_db;", []).is_err() {
            let _ = conn.execute("USE target_db;", []);
        }
        Ok(conn)
    } else if db.starts_with("ducklake:") {
        let conn = open_duckdb_connection()
            .with_context(|| "Failed to open in-memory DB for DuckLake ATTACH")?;
        let (target_path, data_path, read_only) =
            crate::connectors::parse_ducklake_cli_string(db);
        let resolved_token = resolve_quack_token(quack_token, None);
        let spec = crate::connectors::AttachSpec {
            provider: Some(crate::connectors::AttachProviderKind::DuckLake),
            alias: "target_db".to_string(),
            metadata: Some(target_path),
            data_path,
            token: resolved_token.clone(),
            client_id: resolve_quack_client_id(resolved_token.as_deref()),
            read_only,
            disable_ssl: quack_disable_ssl,
            insecure: quack_disable_ssl,
            ..Default::default()
        };
        let plan = crate::connectors::plan_attach(&spec).map_err(|e| {
            anyhow::anyhow!(
                "Failed to plan DuckLake lakehouse '{}': {}",
                redact_connection_credentials(db),
                redact_connection_credentials(&e.to_string())
            )
        })?;
        crate::connectors::execute_attach_plan(&conn, &plan, None).map_err(|e| {
            anyhow::anyhow!(
                "Failed to attach DuckLake lakehouse '{}': {}",
                redact_connection_credentials(db),
                redact_connection_credentials(&e.to_string())
            )
        })?;
        if conn.execute("CONNECT target_db;", []).is_err() {
            let _ = conn.execute("USE target_db;", []);
        }
        Ok(conn)
    } else if is_remote(db) {
        let conn = open_duckdb_connection()
            .with_context(|| "Failed to open in-memory DB for remote ATTACH")?;
        let safe_db = db.replace('\'', "''");
        let attach_res = conn.execute(&format!("ATTACH '{}' AS target_db;", safe_db), []);
        if let Err(e) = attach_res {
            return Err(anyhow::anyhow!(
                "Failed to attach remote database '{}': {}",
                redact_connection_credentials(db),
                redact_connection_credentials(&e.to_string())
            ));
        }
        if conn.execute("CONNECT target_db;", []).is_err() {
            conn.execute("USE target_db;", [])
                .with_context(|| "Failed to switch to target database")?;
        }
        Ok(conn)
    } else {
        let conn = open_duckdb_connection()
            .with_context(|| "Failed to open in-memory DB for local ATTACH")?;
        let safe_db = db.replace('\'', "''");
        let attach_res = conn.execute(&format!("ATTACH '{}' AS target_db;", safe_db), []);
        if let Err(e) = attach_res {
            let err_msg = e.to_string();
            if let Some(diagnostic) = diagnose_storage_version_error(&err_msg, db) {
                return Err(anyhow::anyhow!(
                    "{}\n\nOriginal error: {}",
                    diagnostic,
                    err_msg
                ));
            }
            if err_msg.contains("Conflicting lock is held")
                || err_msg.contains("Could not set lock on file")
            {
                return Err(anyhow::anyhow!(
                    "Failed to attach local database '{}': {}\n\n\
                    💡 DuckDB File Lock Detected:\n\
                      Another process (such as a Jupyter Notebook kernel, Python REPL, or IDE)\n\
                      currently holds an open write lock on this database file.\n\n\
                    Troubleshooting:\n\
                      • If running in Python / Jupyter, pass your live connection object:\n\
                          >>> duckpack.explore(con)\n\
                        DuckPack will connect directly to your live session over loopback Quack without file locks.\n\
                      • Or close the connection in your other process:\n\
                          >>> con.close()\n\
                      • If a background kernel or zombie process is holding the lock, restart the kernel or terminate the process.",
                    db,
                    err_msg
                ));
            }
            return Err(anyhow::anyhow!(
                "Failed to attach local database '{}': {}",
                db,
                err_msg
            ));
        }
        if conn.execute("CONNECT target_db;", []).is_err() {
            conn.execute("USE target_db;", [])
                .with_context(|| "Failed to switch to target database")?;
        }
        Ok(conn)
    }
}

/// Restores temporary tables and views staged by Python across the Quack loopback bridge.
pub fn restore_quack_temp_bridge(conn: &Connection) {
    // 1. Restore tables
    let tbl_sql = "SELECT * FROM target_db.query('SELECT table_name FROM duckdb_tables() WHERE table_name LIKE ''__duckpack_temp_bridge_%'' AND internal = false;')";
    if let Ok(mut stmt) = conn.prepare(tbl_sql) {
        let mut bridges = Vec::new();
        if let Ok(mut rows) = stmt.query([]) {
            while let Ok(Some(row)) = rows.next() {
                if let Ok(tname) = row.get::<_, String>(0) {
                    bridges.push(tname);
                }
            }
        }
        for bt in bridges {
            if let Some(orig) = bt.strip_prefix("__duckpack_temp_bridge_") {
                let q_orig = crate::sql_utils::quote_ident(orig);
                let q_bt = crate::sql_utils::quote_ident(&bt);
                let restore_sql = format!(
                    "CREATE OR REPLACE TEMP TABLE {} AS {};",
                    q_orig,
                    crate::sql_utils::wrap_quack_alias_query(
                        "target_db",
                        &format!("SELECT * FROM memory.{}", q_bt)
                    )
                );
                let _ = conn.execute(&restore_sql, []);
                let _ = conn.execute(&format!("DROP TABLE IF EXISTS target_db.{};", q_bt), []);
                let _ = conn.execute(
                    &crate::sql_utils::wrap_quack_call_query(
                        "target_db",
                        &format!("DROP TABLE IF EXISTS {};", q_bt),
                    ),
                    [],
                );
                let _ = conn.execute(
                    &crate::sql_utils::wrap_quack_call_query(
                        "target_db",
                        &format!("DROP TABLE IF EXISTS memory.{};", q_bt),
                    ),
                    [],
                );
            }
        }
    }

    // 2. Restore views (H1: strictly validate staged view DDL is a single CREATE TEMP VIEW)
    let view_sql = "SELECT * FROM target_db.query('SELECT view_name, view_sql FROM memory.__duckpack_temp_views_registry;')";
    if let Ok(mut stmt) = conn.prepare(view_sql) {
        let mut views = Vec::new();
        if let Ok(mut rows) = stmt.query([]) {
            while let Ok(Some(row)) = rows.next() {
                if let (Ok(vname), Ok(vsql)) = (row.get::<_, String>(0), row.get::<_, String>(1)) {
                    views.push((vname, vsql));
                }
            }
        }
        for (_vname, vsql) in views {
            if crate::sql_utils::validate_safe_temp_view_ddl(&vsql).is_ok() {
                let _ = conn.execute(&vsql, []);
            }
        }
        let _ = conn.execute(
            "CALL target_db.query('DROP TABLE IF EXISTS memory.__duckpack_temp_views_registry;');",
            [],
        );
    }
}

/// Cleans up any orphaned __duckpack_temp_bridge_% or __duckpack_temp_views_% tables left on target_db.
pub fn cleanup_orphaned_quack_bridges(conn: &Connection) {
    let tbl_sql = "SELECT * FROM target_db.query('SELECT table_name FROM duckdb_tables() WHERE table_name LIKE ''__duckpack_%'' AND internal = false;')";
    if let Ok(mut stmt) = conn.prepare(tbl_sql) {
        let mut bridges = Vec::new();
        if let Ok(mut rows) = stmt.query([]) {
            while let Ok(Some(row)) = rows.next() {
                if let Ok(tname) = row.get::<_, String>(0) {
                    bridges.push(tname);
                }
            }
        }
        for bt in bridges {
            let q_bt = crate::sql_utils::quote_ident(&bt);
            let _ = conn.execute(&format!("DROP TABLE IF EXISTS target_db.{};", q_bt), []);
            let _ = conn.execute(
                &crate::sql_utils::wrap_quack_call_query(
                    "target_db",
                    &format!("DROP TABLE IF EXISTS {};", q_bt),
                ),
                [],
            );
            let _ = conn.execute(
                &crate::sql_utils::wrap_quack_call_query(
                    "target_db",
                    &format!("DROP TABLE IF EXISTS memory.{};", q_bt),
                ),
                [],
            );
        }
    }
}

/// Syncs back client temporary tables and views to target_db before closing Quack connection.
pub fn sync_back_quack_temp_bridge(conn: &Connection) {
    let mut client_temp_tables = Vec::new();
    if let Ok(mut stmt) = conn.prepare("SELECT table_name FROM duckdb_tables() WHERE (temporary = true OR database_name = 'temp') AND internal = false;") {
        if let Ok(mut rows) = stmt.query([]) {
            while let Ok(Some(row)) = rows.next() {
                if let Ok(tname) = row.get::<_, String>(0) {
                    client_temp_tables.push(tname);
                }
            }
        }
    }

    for t in &client_temp_tables {
        let stage_sql = format!(
            "CREATE OR REPLACE TABLE target_db.{} AS SELECT * FROM {};",
            crate::sql_utils::quote_ident(&format!("__duckpack_temp_bridge_{}", t)),
            crate::sql_utils::quote_ident(t)
        );
        let _ = conn.execute(&stage_sql, []);
    }

    let mut client_temp_views = Vec::new();
    if let Ok(mut stmt) = conn.prepare("SELECT view_name, sql FROM duckdb_views() WHERE (temporary = true OR database_name = 'temp') AND internal = false;") {
        if let Ok(mut rows) = stmt.query([]) {
            while let Ok(Some(row)) = rows.next() {
                if let (Ok(vname), Ok(vsql)) = (row.get::<_, String>(0), row.get::<_, String>(1)) {
                    client_temp_views.push((vname, vsql));
                }
            }
        }
    }

    if !client_temp_views.is_empty() {
        let _ = conn.execute("CALL target_db.query('CREATE OR REPLACE TABLE memory.__duckpack_temp_views_sync_back (view_name VARCHAR, view_sql VARCHAR);');", []);
        for (vname, vsql) in &client_temp_views {
            let inner_ins = format!(
                "INSERT INTO memory.__duckpack_temp_views_sync_back VALUES ({}, {});",
                crate::sql_utils::quote_literal(vname),
                crate::sql_utils::quote_literal(vsql)
            );
            let ins_sql = crate::sql_utils::wrap_quack_call_query("target_db", &inner_ins);
            let _ = conn.execute(&ins_sql, []);
        }
    }
}

/// Executes the computed schema differences against the target database.
/// If running against a remote database, it encapsulates the entire deployment inside a single transaction
/// so it can rollback gracefully upon failure.
pub fn apply_diff(
    conn: &Connection,
    diff: &diff::DiffResult,
    force_drop: bool,
    is_remote: bool,
    project_dir: &PathBuf,
    catalog: Option<&str>,
) -> Result<()> {
    let mut exec = conn;
    apply_diff_with_executor(&mut exec, diff, force_drop, is_remote, project_dir, catalog)
}

/// Formats a structured DuckLake snapshot commit message summarizing the applied declarative schema changes.
pub fn format_ducklake_commit_message(diff: &diff::DiffResult, force_drop: bool) -> String {
    let active_items: Vec<&diff::DiffItem> = diff
        .items
        .iter()
        .filter(|i| i.selected && !i.ignored && (force_drop || !i.item_type.is_destructive()))
        .collect();
    let count = active_items.len();
    let summary: Vec<String> = active_items
        .iter()
        .take(5)
        .map(|i| format!("{:?} {}", i.item_type, i.obj_name))
        .collect();
    let suffix = if count > 5 {
        format!(", +{} more", count - 5)
    } else {
        String::new()
    };
    if summary.is_empty() {
        format!("duckpack apply: [v{}] 0 schema changes", duckpack_version())
    } else {
        format!(
            "duckpack apply: [v{}] {} schema change(s) ({}{})",
            duckpack_version(),
            count,
            summary.join(", "),
            suffix
        )
    }
}

/// Applies a computed `DiffResult` against any `DbExecutor` adapter (`duckdb::Connection` or `PyDbExecutor`).
/// Orchestrates `pre-deploy` hooks, topological retry across schema phases, `post-deploy` hooks, and transactional rollback.
pub fn apply_diff_with_executor(
    exec: &mut dyn DbExecutor,
    diff: &diff::DiffResult,
    force_drop: bool,
    is_remote: bool,
    project_dir: &Path,
    catalog: Option<&str>,
) -> Result<()> {
    apply_diff_with_lake_options(exec, diff, force_drop, false, is_remote, project_dir, catalog)
}

/// Applies a computed `DiffResult` with explicit control over `--allow-lake-rewrite` (`allow_lake_rewrite`).
pub fn apply_diff_with_lake_options(
    exec: &mut dyn DbExecutor,
    diff: &diff::DiffResult,
    force_drop: bool,
    allow_lake_rewrite: bool,
    is_remote: bool,
    project_dir: &Path,
    catalog: Option<&str>,
) -> Result<()> {
    let req_cat_safe = catalog.unwrap_or("").replace('\'', "''");
    let quack_alias = diff::detect_quack_alias(exec, &req_cat_safe);
    let is_quack = quack_alias.is_some();
    let q_alias = quack_alias.as_deref().unwrap_or("target_db");
    let quoted_q_alias = format!("\"{}\"", q_alias.replace('"', "\"\""));

    let orig_db: String = if is_quack {
        exec.query_rows(&format!(
            "SELECT * FROM {}.query('SELECT current_database()')",
            quoted_q_alias
        ))
        .ok()
        .and_then(|(_, rows)| rows.into_iter().next())
        .and_then(|r| r.into_iter().next())
        .unwrap_or_else(|| "memory".to_string())
    } else {
        exec.query_rows("SELECT current_database()")
            .ok()
            .and_then(|(_, rows)| rows.into_iter().next())
            .and_then(|r| r.into_iter().next())
            .unwrap_or_default()
    };

    let mut target_cat = catalog.unwrap_or(&orig_db);
    // If connected via Quack, the client-side attachment alias (e.g. 'target_db' or 'quack_remote')
    // does not exist as a catalog name on the remote Quack server itself.
    // Resolve it to the remote server's active database (orig_db).
    if is_quack && (target_cat == q_alias || target_cat == "target_db" || target_cat.is_empty()) {
        target_cat = &orig_db;
    }

    let lake_info = diff::get_ducklake_catalog_info_exec(exec, Some(target_cat));
    let any_quack_alias = diff::find_any_quack_alias(exec, Some(target_cat));
    let env_allow_rewrite = std::env::var("DUCKPACK_ALLOW_LAKE_REWRITE")
        .map(|v| {
            let t = v.trim();
            t == "1" || t.eq_ignore_ascii_case("true") || t.eq_ignore_ascii_case("yes")
        })
        .unwrap_or(false);
    let effective_allow_lake_rewrite = allow_lake_rewrite || env_allow_rewrite;

    if let Some(ref lake) = lake_info {
        if let Some(recreate_item) = diff.items.iter().find(|i| {
            i.selected && !i.ignored && i.item_type == diff::DiffItemType::RecreateTable
        }) {
            if !effective_allow_lake_rewrite {
                let path_detail = lake
                    .path
                    .as_deref()
                    .filter(|p| !p.is_empty())
                    .map(|p| format!(" in {}", p))
                    .unwrap_or_default();
                anyhow::bail!(
                    "Cannot execute physical copy-and-swap on DuckLake table '{}' in catalog '{}'.\n\
                     Modifying this constraint would require rewriting underlying Parquet files{}.\n\
                     Pass '--allow-lake-rewrite' (or set DUCKPACK_ALLOW_LAKE_REWRITE=true) to force full table recreation.",
                    recreate_item.obj_name,
                    lake.name,
                    path_detail
                );
            }
        }
    }

    if !is_quack && !target_cat.is_empty() && target_cat != orig_db {
        exec.execute_batch(&format!("USE \"{}\";", target_cat.replace('\"', "\"\"")))?;
    }

    let has_recreate = diff.items.iter().any(|i| {
        i.selected && !i.ignored && i.item_type == diff::DiffItemType::RecreateTable && force_drop
    });
    let is_ducklake = lake_info.is_some();
    let is_remote_effective = (is_remote || is_quack || is_ducklake) && !has_recreate;

    if is_remote_effective {
        if is_quack {
            let _ = exec.execute_batch(&format!(
                "CALL {}.query('BEGIN TRANSACTION;');",
                quoted_q_alias
            ));
        } else {
            let _ = exec.execute_batch("BEGIN TRANSACTION;");
        }
    }

    let result = (|| -> Result<()> {
        let mut execute_sql = |sql: &str| -> Result<()> {
            if is_quack {
                let inner = if !target_cat.is_empty() {
                    format!("USE {}; {}", crate::sql_utils::quote_ident(target_cat), sql)
                } else {
                    sql.to_string()
                };
                exec.execute_batch(&crate::sql_utils::wrap_quack_call_query(q_alias, &inner))
                    .with_context(|| {
                        if !target_cat.is_empty() {
                            format!(
                                "Failed executing DDL on catalog '{}' via Quack ('{}')",
                                target_cat, q_alias
                            )
                        } else {
                            "Failed executing DDL via Quack".to_string()
                        }
                    })?;
            } else if sql.contains("_duckpack_auth") {
                if let Some(ref qa) = any_quack_alias {
                    exec.execute_batch(&crate::sql_utils::wrap_quack_call_query(qa, sql))
                        .with_context(|| {
                            format!("Failed deploying _duckpack_auth to Quack catalog '{}'", qa)
                        })?;
                } else if is_ducklake && !orig_db.is_empty() {
                    // DuckLake storage catalogs do not implement CreateFunction (CREATE MACRO);
                    // execute _duckpack_auth in the primary DuckDB catalog and switch back.
                    let _ = exec.execute_batch(&format!(
                        "USE {};",
                        crate::sql_utils::quote_ident(&orig_db)
                    ));
                    let res = exec.execute_batch(sql);
                    let _ = exec.execute_batch(&format!(
                        "USE {};",
                        crate::sql_utils::quote_ident(target_cat)
                    ));
                    res?;
                } else {
                    exec.execute_batch(sql)?;
                }
            } else {
                exec.execute_batch(sql)?;
            }
            Ok(())
        };

        let ignore_rules = load_ignore_rules(project_dir);

        let pre_deploy_dir = project_dir.join("scripts").join("pre-deploy");
        if pre_deploy_dir.exists() {
            execute_directory_filtered(&pre_deploy_dir, project_dir, &ignore_rules, &mut execute_sql)?;
        }

        let mut views_to_apply = Vec::new();
        let mut triggers_to_apply = Vec::new();
        let mut drops_to_apply = Vec::new();
        let mut drop_sequences = Vec::new();
        let mut drop_schemas = Vec::new();

        // 1. Create schemas first
        let mut schemas_to_apply = Vec::new();
        for item in &diff.items {
            if !item.selected || item.ignored {
                continue;
            }
            if item.item_type == diff::DiffItemType::CreateSchema {
                schemas_to_apply.push(item.sql.clone());
            }
        }
        if !schemas_to_apply.is_empty() {
            execute_sqls_with_retry_custom(schemas_to_apply, &mut execute_sql)?;
        }

        // 2. Create sequences before tables/macros
        let mut sequences_to_apply = Vec::new();
        for item in &diff.items {
            if !item.selected || item.ignored {
                continue;
            }
            if item.item_type == diff::DiffItemType::CreateSequence {
                sequences_to_apply.push(item.sql.clone());
            }
        }
        if !sequences_to_apply.is_empty() {
            execute_sqls_with_retry_custom(sequences_to_apply, &mut execute_sql)?;
        }

        let mut structure_to_apply = Vec::new();
        let mut drop_triggers = Vec::new();
        for item in &diff.items {
            if !item.selected || item.ignored {
                continue;
            }
            match item.item_type {
                diff::DiffItemType::CreateTable
                | diff::DiffItemType::AlterTable
                | diff::DiffItemType::CreateMacro
                | diff::DiffItemType::RenameTable
                | diff::DiffItemType::RenameColumn
                | diff::DiffItemType::AddConstraint => {
                    structure_to_apply.push(item.sql.clone());
                }
                diff::DiffItemType::RecreateTable => {
                    if force_drop {
                        if let Some(ref lake) = lake_info {
                            let commit_msg = format_ducklake_commit_message(diff, force_drop);
                            let safe_msg = commit_msg.replace('\'', "''");
                            let quoted_cat = format!("\"{}\"", lake.name.replace('"', "\"\""));
                            let tag_sql = format!(
                                "CALL {}.set_commit_message('duckpack', '{}');\nCOMMIT;",
                                quoted_cat, safe_msg
                            );
                            let tagged_recreate = if let Some(idx) = item.sql.rfind("COMMIT;") {
                                format!(
                                    "{}{}{}",
                                    &item.sql[..idx],
                                    tag_sql,
                                    &item.sql[idx + "COMMIT;".len()..]
                                )
                            } else {
                                format!(
                                    "BEGIN TRANSACTION;\n{}\n{}",
                                    item.sql.trim(),
                                    tag_sql
                                )
                            };
                            structure_to_apply.push(tagged_recreate);
                        } else {
                            structure_to_apply.push(item.sql.clone());
                        }
                    }
                }
                diff::DiffItemType::DropTrigger => {
                    if force_drop {
                        drop_triggers.push(item.sql.clone());
                    }
                }
                diff::DiffItemType::DropTable
                | diff::DiffItemType::DropColumn
                | diff::DiffItemType::DropView
                | diff::DiffItemType::DropMacro => {
                    if force_drop {
                        drops_to_apply.push(item.sql.clone());
                    }
                }
                diff::DiffItemType::DropSequence => {
                    if force_drop {
                        drop_sequences.push(item.sql.clone());
                    }
                }
                diff::DiffItemType::DropSchema => {
                    if force_drop {
                        drop_schemas.push(item.sql.clone());
                    }
                }
                diff::DiffItemType::CreateView => {
                    views_to_apply.push(item.sql.clone());
                }
                diff::DiffItemType::CreateTrigger => {
                    triggers_to_apply.push(item.sql.clone());
                }
                diff::DiffItemType::CreateSchema | diff::DiffItemType::CreateSequence => {} // handled above
            }
        }

        if !structure_to_apply.is_empty() {
            execute_sqls_with_retry_custom(structure_to_apply, &mut execute_sql)?;
        }

        // 3. Views
        if !views_to_apply.is_empty() {
            execute_sqls_with_retry_custom(views_to_apply, &mut execute_sql)?;
        }

        // 4. Triggers (drop obsolete triggers before creating/replacing so modified triggers never collide)
        for drop_sql in drop_triggers {
            let _ = execute_sql(&drop_sql);
        }
        if !triggers_to_apply.is_empty() {
            execute_sqls_with_retry_custom(triggers_to_apply, &mut execute_sql)?;
        }

        // 5. Drops (tables/columns/views/macros first, then sequences, then schemas)
        for drop_sql in drops_to_apply {
            let _ = execute_sql(&drop_sql); // Ignore errors if already dropped by cascade
        }
        for drop_sql in drop_sequences {
            let _ = execute_sql(&drop_sql);
        }
        for drop_sql in drop_schemas {
            let _ = execute_sql(&drop_sql);
        }

        let post_deploy_dir = project_dir.join("scripts").join("post-deploy");
        if post_deploy_dir.exists() {
            execute_directory_filtered(&post_deploy_dir, project_dir, &ignore_rules, &mut execute_sql)?;
        }

        if let Some(ref lake) = lake_info {
            if is_remote_effective {
                let commit_msg = format_ducklake_commit_message(diff, force_drop);
                let safe_msg = commit_msg.replace('\'', "''");
                let quoted_cat = format!("\"{}\"", lake.name.replace('"', "\"\""));
                let tag_sql = format!(
                    "CALL {}.set_commit_message('duckpack', '{}');",
                    quoted_cat, safe_msg
                );
                let _ = execute_sql(&tag_sql);
            }
        }

        Ok(())
    })();

    let applied_quack_auth = diff.items.iter().any(|i| {
        i.selected && !i.ignored && i.obj_name == "_duckpack_auth.quack_rbac_policy"
    });

    if is_remote_effective {
        if result.is_ok() {
            if is_quack {
                exec.execute_batch(&format!(
                    "CALL {}.query('COMMIT;');",
                    quoted_q_alias
                ))
                .with_context(|| {
                    format!(
                        "Failed to commit transaction on Quack catalog '{}'",
                        q_alias
                    )
                })?;
            } else {
                exec.execute_batch("COMMIT;")?;
            }
        } else {
            if is_quack {
                let _ = exec.execute_batch(&format!(
                    "CALL {}.query('ROLLBACK;');",
                    quoted_q_alias
                ));
            } else {
                exec.execute_batch("ROLLBACK;")?;
            }
        }
    }

    // Activate Quack global RBAC hooks ONLY AFTER COMMIT has succeeded so that all _duckpack_auth
    // tables and _duckpack_quack_* macros are committed and visible to fresh server-side connections.
    if result.is_ok() && applied_quack_auth {
        let hook_quack_alias = quack_alias.as_deref().or(any_quack_alias.as_deref());
        if let Some(qa) = hook_quack_alias {
            if let Some(qa_cfg) = crate::quack_auth::load_quack_auth_config(project_dir) {
                let quoted_qa = format!("\"{}\"", qa.replace('"', "\"\""));
                let (auth_fn, authz_fn) = if qa_cfg.enabled {
                    ("_duckpack_quack_authenticate", "_duckpack_quack_authorize")
                } else {
                    ("quack_check_token", "quack_nop_authorization")
                };
                exec.execute_batch(&format!(
                    "CALL {}.query('SET GLOBAL quack_authentication_function = ''{}'';');",
                    quoted_qa, auth_fn
                ))
                .with_context(|| {
                    format!(
                        "Failed to activate quack_authentication_function='{}' on '{}'",
                        auth_fn, qa
                    )
                })?;
                exec.execute_batch(&format!(
                    "CALL {}.query('SET GLOBAL quack_authorization_function = ''{}'';');",
                    quoted_qa, authz_fn
                ))
                .with_context(|| {
                    format!(
                        "Failed to activate quack_authorization_function='{}' on '{}'",
                        authz_fn, qa
                    )
                })?;
            }
        }
    }

    if !is_quack && !orig_db.is_empty() && target_cat != orig_db {
        let _ = exec.execute_batch(&format!("USE \"{}\";", orig_db.replace('\"', "\"\"")));
    }

    result
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ResolvedTargetConfig {
    pub db: Option<String>,
    pub catalogs: Vec<String>,
    pub token_env: Option<String>,
    pub insecure: Option<bool>,
}

pub use crate::connectors::{
    build_r2_secret_body, build_s3_secret_body, escape_ado_value, escape_libpq_value,
    extract_r2_account_id, format_ducklake_target, format_mssql_connection_string,
    format_quack_endpoint, is_remote_ducklake_url,
};

/// Checks if an S3 secret is already registered in `conn`; if not, inspects process env vars and `.env`
/// in `project_dir` (defaulting to current working directory) for `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`,
/// `AWS_REGION`, and `AWS_ENDPOINT` and auto-registers a session S3 secret.
pub fn ensure_s3_secret_from_env(conn: &Connection, project_dir: Option<&Path>) {
    let existing_s3_secrets: i64 = conn
        .query_row(
            "SELECT count(*) FROM duckdb_secrets() WHERE lower(type) = 's3'",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);
    if existing_s3_secrets > 0 {
        return;
    }

    let dir = project_dir.unwrap_or_else(|| Path::new("."));
    let get_var = |key: &str| -> String {
        crate::env_store::resolve_env_var(Some(dir), None, key).unwrap_or_default()
    };

    let key_id = get_var("AWS_ACCESS_KEY_ID");
    let secret_key = get_var("AWS_SECRET_ACCESS_KEY");
    let mut region = get_var("AWS_REGION");
    if region.is_empty() {
        region = get_var("AWS_DEFAULT_REGION");
    }
    let endpoint = get_var("AWS_ENDPOINT");

    if key_id.is_empty() && secret_key.is_empty() && endpoint.is_empty() {
        return;
    }
    if region.is_empty() {
        region = "us-east-1".to_string();
    }

    let secret_body = build_s3_secret_body(&key_id, &secret_key, &region, &endpoint, true);
    let sql = format!("CREATE OR REPLACE SECRET s3_env_auto ({});", secret_body);
    let _ = conn.execute(&sql, []);
}

/// Installs and loads the required DuckLake extensions (`ducklake`, plus `postgres`/`sqlite`/`quack`
/// and `httpfs`/`azure` when cloud storage paths are used) and optionally binds Quack / S3 secrets on `conn`.
pub fn prepare_ducklake_connection(
    conn: &Connection,
    target_dsn: &str,
    data_path_or_opts: &str,
    quack_token: Option<&str>,
    project_dir: Option<&Path>,
) -> Result<()> {
    prepare_ducklake_connection_with_options(
        conn,
        target_dsn,
        data_path_or_opts,
        quack_token,
        project_dir,
        true,
    )
}

/// Loads required DuckLake extensions with control over whether to mutate global Quack secrets.
pub fn prepare_ducklake_connection_with_options(
    conn: &Connection,
    target_dsn: &str,
    data_path_or_opts: &str,
    quack_token: Option<&str>,
    project_dir: Option<&Path>,
    apply_global_quack_secret: bool,
) -> Result<()> {
    ensure_extension_loaded(conn, "ducklake")
        .with_context(|| "Failed to install/load ducklake extension")?;
    let stripped_dsn = target_dsn.trim().strip_prefix("ducklake:").unwrap_or(target_dsn.trim());
    if stripped_dsn.starts_with("postgres:") || stripped_dsn.starts_with("postgresql:") {
        ensure_extension_loaded(conn, "postgres")
            .with_context(|| "Failed to install/load postgres extension for DuckLake catalog")?;
    }
    if stripped_dsn.starts_with("sqlite:") {
        ensure_extension_loaded(conn, "sqlite")
            .with_context(|| "Failed to install/load sqlite extension for DuckLake catalog")?;
    }
    if stripped_dsn.starts_with("quack:") || stripped_dsn.starts_with("quack://") {
        load_quack_extension(conn)
            .with_context(|| "Failed to install/load quack extension for DuckLake catalog")?;
        if apply_global_quack_secret {
            let _ = apply_quack_token_secret(conn, quack_token);
        }
    }
    if target_dsn.contains("s3://")
        || target_dsn.contains("r2://")
        || target_dsn.contains("gcs://")
        || target_dsn.contains("gs://")
        || target_dsn.contains("http://")
        || target_dsn.contains("https://")
        || data_path_or_opts.contains("s3://")
        || data_path_or_opts.contains("r2://")
        || data_path_or_opts.contains("gcs://")
        || data_path_or_opts.contains("gs://")
        || data_path_or_opts.contains("http://")
        || data_path_or_opts.contains("https://")
    {
        ensure_extension_loaded(conn, "httpfs")
            .with_context(|| "Failed to install/load httpfs extension for DuckLake storage")?;
        ensure_s3_secret_from_env(conn, project_dir);
    }
    if target_dsn.contains("az://")
        || target_dsn.contains("azure://")
        || target_dsn.contains("abfss://")
        || target_dsn.contains("abfs://")
        || data_path_or_opts.contains("az://")
        || data_path_or_opts.contains("azure://")
        || data_path_or_opts.contains("abfss://")
        || data_path_or_opts.contains("abfs://")
    {
        let _ = ensure_extension_loaded(conn, "azure");
    }
    Ok(())
}

fn parse_target_table(tbl: &toml::Value) -> ResolvedTargetConfig {
    let mut cfg = ResolvedTargetConfig::default();

    let provider = tbl
        .get("provider")
        .or_else(|| tbl.get("type"))
        .and_then(|v| v.as_str());

    if let Some(prov) = provider {
        if prov.eq_ignore_ascii_case("mssql") {
            let server = tbl
                .get("server")
                .or_else(|| tbl.get("host"))
                .and_then(|v| v.as_str())
                .unwrap_or("localhost");
            let port = tbl
                .get("port")
                .and_then(|v| v.as_integer())
                .unwrap_or(1433)
                .to_string();
            let db_name = tbl
                .get("database")
                .or_else(|| tbl.get("db"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let is_trusted = tbl
                .get("trusted_connection")
                .or_else(|| tbl.get("windows_auth"))
                .and_then(|v| v.as_bool())
                .unwrap_or(cfg!(target_os = "windows"));
            let user = tbl
                .get("username")
                .or_else(|| tbl.get("user"))
                .or_else(|| tbl.get("user_id"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let pass = tbl.get("password").and_then(|v| v.as_str()).unwrap_or("");
            let insecure = tbl
                .get("insecure")
                .or_else(|| tbl.get("trust_server_certificate"))
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let disable_ssl = tbl
                .get("disable_ssl")
                .or_else(|| tbl.get("no_encrypt"))
                .and_then(|v| v.as_bool())
                .unwrap_or(false);

            let conn_str = format_mssql_connection_string(
                server,
                &port,
                db_name,
                user,
                pass,
                is_trusted,
                insecure,
                disable_ssl,
            );
            cfg.db = Some(format!("mssql:{}", conn_str));
        } else if prov.eq_ignore_ascii_case("ducklake") {
            let backend = tbl
                .get("backend")
                .and_then(|v| v.as_str())
                .map(|s| s.to_ascii_lowercase());
            let catalog_explicit = tbl.get("catalog").and_then(|v| v.as_str());
            let data_path = tbl
                .get("data_path")
                .or_else(|| tbl.get("storage"))
                .or_else(|| {
                    if matches!(backend.as_deref(), Some("file") | Some("duckdb") | Some("sqlite")) {
                        None
                    } else {
                        tbl.get("path")
                    }
                })
                .and_then(|v| v.as_str());

            let mut catalog_target = if let Some(cat) = catalog_explicit {
                if cat.starts_with("ducklake:") {
                    cat.to_string()
                } else {
                    format!("ducklake:{}", cat)
                }
            } else if backend.as_deref() == Some("quack") {
                let server = tbl
                    .get("server")
                    .or_else(|| tbl.get("host"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("localhost");
                let port = tbl
                    .get("port")
                    .and_then(|v| v.as_integer())
                    .unwrap_or(9494)
                    .to_string();
                format_ducklake_target("quack", server, &port, "", "", "", "")
                    .unwrap_or_else(|_| "ducklake:quack:localhost:9494".to_string())
            } else if backend.as_deref() == Some("sqlite") {
                let file_path = tbl
                    .get("database")
                    .or_else(|| tbl.get("file"))
                    .or_else(|| tbl.get("path"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("metadata.db");
                format_ducklake_target("sqlite", "", "", "", "", "", file_path)
                    .unwrap_or_else(|_| "ducklake:sqlite:metadata.db".to_string())
            } else if backend.as_deref() == Some("file") || backend.as_deref() == Some("duckdb") {
                let file_path = tbl
                    .get("database")
                    .or_else(|| tbl.get("file"))
                    .or_else(|| tbl.get("path"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("metadata.ducklake");
                format_ducklake_target("file", "", "", "", "", "", file_path)
                    .unwrap_or_else(|_| "ducklake:metadata.ducklake".to_string())
            } else if let Some(server) = tbl
                .get("server")
                .or_else(|| tbl.get("host"))
                .and_then(|v| v.as_str())
            {
                let is_quack = server.starts_with("quack:")
                    || server.starts_with("quack://")
                    || tbl.get("port").and_then(|v| v.as_integer()) == Some(9494);
                if is_quack {
                    let port = tbl
                        .get("port")
                        .and_then(|v| v.as_integer())
                        .unwrap_or(9494)
                        .to_string();
                    format_ducklake_target("quack", server, &port, "", "", "", "")
                        .unwrap_or_else(|_| "ducklake:quack:localhost:9494".to_string())
                } else {
                    let port = tbl
                        .get("port")
                        .and_then(|v| v.as_integer())
                        .unwrap_or(5432)
                        .to_string();
                    let dbname = tbl
                        .get("dbname")
                        .or_else(|| tbl.get("database"))
                        .or_else(|| tbl.get("db"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("ducklake");
                    let u = tbl
                        .get("username")
                        .or_else(|| tbl.get("user"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    let p = tbl.get("password").and_then(|v| v.as_str()).unwrap_or("");
                    format_ducklake_target("postgres", server, &port, dbname, u, p, "")
                        .unwrap_or_else(|_| "ducklake:postgres:host=localhost port=5432 dbname=ducklake".to_string())
                }
            } else if let Some(db_val) = tbl
                .get("database")
                .or_else(|| tbl.get("db"))
                .and_then(|v| v.as_str())
            {
                format!("ducklake:{}", db_val.trim_start_matches("ducklake:"))
            } else {
                "ducklake:ducklake".to_string()
            };

            if let Some(dp) = data_path {
                catalog_target.push_str(&format!(
                    " (DATA_PATH {})",
                    crate::sql_utils::quote_literal(dp)
                ));
            }
            cfg.db = Some(catalog_target);
        }
    }

    if cfg.db.is_none() {
        if let Some(db_val) = tbl
            .get("db")
            .or_else(|| tbl.get("default_db"))
            .and_then(|v| v.as_str())
        {
            cfg.db = Some(db_val.to_string());
        } else if let Some(host_val) = tbl.get("host").and_then(|v| v.as_str()) {
            let clean_host = host_val
                .strip_prefix("quack://")
                .or_else(|| host_val.strip_prefix("quack:"))
                .unwrap_or(host_val);
            if let Some(port_val) = tbl.get("port").and_then(|v| v.as_integer()) {
                cfg.db = Some(format!("quack:{}:{}", clean_host, port_val));
            } else {
                cfg.db = Some(format!("quack:{}", clean_host));
            }
        }
    }

    let is_provider_with_dsn_database = provider
        .map(|p| p.eq_ignore_ascii_case("ducklake") || p.eq_ignore_ascii_case("mssql"))
        .unwrap_or(false);

    let catalog_val = if is_provider_with_dsn_database {
        tbl.get("catalogs")
    } else {
        tbl.get("catalogs")
            .or_else(|| tbl.get("database"))
            .or_else(|| tbl.get("catalog"))
    };

    if let Some(val) = catalog_val {
        if let Some(s) = val.as_str() {
            cfg.catalogs = vec![s.to_string()];
        } else if let Some(arr) = val.as_array() {
            cfg.catalogs = arr
                .iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect();
        }
    }

    if let Some(t_env) = tbl.get("token_env").and_then(|v| v.as_str()) {
        cfg.token_env = Some(t_env.to_string());
    }

    if let Some(ins) = tbl
        .get("insecure")
        .or_else(|| tbl.get("quack_disable_ssl"))
        .and_then(|v| v.as_bool())
    {
        cfg.insecure = Some(ins);
    }

    cfg
}

pub use crate::env_store::{
    append_cloud_bucket_to_env, generate_random_hex_token, load_env_files, sanitize_env_value,
    save_cloud_secret_to_env, validate_safe_env_profile, write_restricted_env_file,
};

/// Resolves target configuration from duckpack.toml, supporting [target] defaults and [env.<profile>] overrides.
pub fn resolve_target_config(
    project_dir: &std::path::Path,
    env_profile: Option<&str>,
) -> ResolvedTargetConfig {
    load_env_files(project_dir, env_profile);

    let toml_path = project_dir.join("duckpack.toml");
    if !toml_path.exists() {
        return ResolvedTargetConfig::default();
    }

    let content = match std::fs::read_to_string(&toml_path) {
        Ok(c) => c,
        Err(_) => return ResolvedTargetConfig::default(),
    };

    let value: toml::Value = match toml::from_str(&content) {
        Ok(v) => v,
        Err(_) => return ResolvedTargetConfig::default(),
    };

    let target_defaults = value
        .get("target")
        .map(parse_target_table)
        .unwrap_or_default();

    if let Some(env_str) = env_profile {
        let env_tbl = value
            .get("env")
            .and_then(|e| e.get(env_str))
            .or_else(|| value.get("target").and_then(|t| t.get(env_str)))
            .or_else(|| value.get("environments").and_then(|e| e.get(env_str)));

        if let Some(env_tbl) = env_tbl {
            let env_cfg = parse_target_table(env_tbl);
            return ResolvedTargetConfig {
                db: env_cfg.db.or(target_defaults.db),
                catalogs: if !env_cfg.catalogs.is_empty() {
                    env_cfg.catalogs
                } else {
                    target_defaults.catalogs
                },
                token_env: env_cfg.token_env.or(target_defaults.token_env),
                insecure: env_cfg.insecure.or(target_defaults.insecure),
            };
        }
    }

    target_defaults
}

/// Resolves the Quack authentication token following the multi-tier precedence:
/// 1. Explicit CLI token if provided
/// 2. Value of environment variable specified in `token_env` (if defined in active config)
/// 3. Value of standard `QUACK_TOKEN` from process environment / .env
/// 4. Value of standard `DUCKDB_QUACK_TOKEN` from process environment / .env
pub fn resolve_quack_token(cli_token: Option<&str>, token_env: Option<&str>) -> Option<String> {
    if let Some(t) = cli_token {
        if !t.is_empty() {
            return Some(t.to_string());
        }
    }
    if let Some(var_name) = token_env {
        if let Ok(val) = std::env::var(var_name) {
            if !val.is_empty() {
                return Some(val);
            }
        }
    }
    if let Ok(val) = std::env::var("QUACK_TOKEN") {
        if !val.is_empty() {
            return Some(val);
        }
    }
    if let Ok(val) = std::env::var("DUCKDB_QUACK_TOKEN") {
        if !val.is_empty() {
            return Some(val);
        }
    }
    None
}

/// Resolves target catalogs from duckpack.toml if configured. Supports a single string or an array of catalog names.
pub fn resolve_project_catalogs(
    project_dir: &std::path::Path,
    env_profile: Option<&str>,
) -> Vec<String> {
    resolve_target_config(project_dir, env_profile).catalogs
}

/// Resolves target catalog from duckpack.toml if configured (returns first configured catalog).
pub fn resolve_project_catalog(
    project_dir: &std::path::Path,
    env_profile: Option<&str>,
) -> Option<String> {
    resolve_project_catalogs(project_dir, env_profile)
        .into_iter()
        .next()
}

/// Fully resolved, validated target specification ready to connect.
#[derive(Clone, Debug, PartialEq)]
pub struct TargetSpec {
    pub db: String,
    pub token: Option<String>,
    pub disable_ssl: bool,
    pub catalogs: Vec<String>,
    pub is_quack: bool,
    pub is_motherduck: bool,
}

impl TargetSpec {
    pub fn is_remote(&self) -> bool {
        self.is_quack || self.is_motherduck
    }

    pub fn connect(&self) -> Result<Connection> {
        connect_target_db(&self.db, self.token.as_deref(), self.disable_ssl)
    }
}

/// Unified target resolver combining CLI flags, project configuration, environment profile,
/// and environment variables into a validated TargetSpec.
pub struct TargetResolver;

impl TargetResolver {
    pub fn resolve(
        project_dir: &std::path::Path,
        env_profile: Option<&str>,
        explicit_db: Option<&str>,
        explicit_token: Option<&str>,
        explicit_insecure: Option<bool>,
        explicit_catalogs: Option<Vec<String>>,
    ) -> TargetSpec {
        load_env_files(project_dir, env_profile);
        let config = resolve_target_config(project_dir, env_profile);

        // 1. Resolve database path/URI:
        let db = match explicit_db {
            Some(d) if env_profile.is_none() && d != "local.duckdb" && !d.is_empty() => {
                d.to_string()
            }
            Some(d) if env_profile.is_some() => config.db.clone().unwrap_or_else(|| d.to_string()),
            Some(d) if d == "local.duckdb" => config
                .db
                .clone()
                .unwrap_or_else(|| "local.duckdb".to_string()),
            Some(d) if !d.is_empty() => d.to_string(),
            _ => config
                .db
                .clone()
                .unwrap_or_else(|| "local.duckdb".to_string()),
        };

        // 2. Resolve token
        let token = resolve_quack_token(explicit_token, config.token_env.as_deref());

        // 3. Resolve SSL / insecure
        let disable_ssl = explicit_insecure.unwrap_or(false) || config.insecure.unwrap_or(false);

        // 4. Resolve catalogs
        let catalogs = match explicit_catalogs {
            Some(cats) if !cats.is_empty() => cats,
            _ => config.catalogs,
        };

        let is_quack = db.starts_with("quack:");
        let is_motherduck = is_remote(&db);

        TargetSpec {
            db,
            token,
            disable_ssl,
            catalogs,
            is_quack,
            is_motherduck,
        }
    }
}

pub use crate::project::{
    build_shadow_db, collect_sql_files_filtered, collect_sql_files_recursive,
    compile_project_declarations, discover_project_dir, execute_directory,
    execute_directory_custom, execute_directory_filtered, execute_sqls_with_retry,
    execute_sqls_with_retry_custom, glob_match, init_project, is_file_ignored, is_object_ignored,
    load_ignore_rules,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_load_quack_extension() {
        let test_home = std::env::current_dir().unwrap().join("target").join("test_home");
        let _ = std::fs::remove_dir_all(&test_home);
        std::fs::create_dir_all(&test_home).unwrap();
        unsafe { std::env::set_var("HOME", &test_home); }

        let conn = open_duckdb_connection().unwrap();
        let res = load_quack_extension(&conn);
        if res.is_ok() {
            let sec_res = conn.execute("CREATE SECRET (TYPE QUACK, TOKEN 'foo');", []);
            assert!(sec_res.is_ok());
        }
    }

    #[test]
    fn test_embedded_duckdb_version() {
        let v = embedded_duckdb_version();
        assert!(!v.is_empty());
        assert!(v.starts_with('v') || v.chars().next().map_or(false, |c| c.is_ascii_digit()));

        let conn = open_duckdb_connection().unwrap();
        let (ver, hash) = get_duckdb_version_info(&conn);
        assert!(ver.starts_with('v'));
        assert!(hash.is_some());
    }

    #[test]
    fn test_ensure_extension_loaded_mssql() {
        let conn = open_duckdb_connection().unwrap();
        let res = ensure_extension_loaded(&conn, "mssql");
        if let Err(err) = &res {
            let s = format!("{:?}", err);
            if s.contains("Read-only file system")
                || s.contains("Failed to download")
                || s.contains("Couldn't resolve host")
            {
                return;
            }
        }
        assert!(
            res.is_ok(),
            "ensure_extension_loaded mssql failed: {:?}",
            res
        );
    }

    #[test]
    fn test_diagnose_storage_version_error() {
        let err1 = "Serialization Error: Cannot open database \"prod.duckdb\" with storage version 4 because it was created by a newer version of DuckDB (storage version 5)";
        let diag1 = diagnose_storage_version_error(err1, "prod.duckdb");
        assert!(diag1.is_some());
        let msg = diag1.unwrap();
        assert!(msg.contains("DuckDB Storage Version Mismatch"));
        assert!(msg.contains("prod.duckdb"));
        assert!(msg.contains("Remediation Options"));

        let err2 = "IO Error: File not found";
        let diag2 = diagnose_storage_version_error(err2, "prod.duckdb");
        assert!(diag2.is_none());

        let err3 = "IO Error: Trying to read a database file with version number 69, but we can only read versions between 64 and 68.";
        let diag3 = diagnose_storage_version_error(err3, "prod.duckdb");
        assert!(diag3.is_some());
        let msg3 = diag3.unwrap();
        assert!(msg3.contains("DuckDB Storage Version Mismatch"));
        assert!(msg3.contains("storage_compatibility_version"));
    }

    #[test]
    fn test_execute_directory_recursive() {
        let temp_dir = std::env::temp_dir().join(format!(
            "dp_rec_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let nested = temp_dir.join("finance").join("reports");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(nested.join("kpis.sql"), "CREATE TABLE kpis (id INT);").unwrap();

        let conn = Connection::open_in_memory().unwrap();
        execute_directory(&conn, &temp_dir).unwrap();

        let count: i64 = conn
            .query_row(
                "SELECT count(*) FROM duckdb_tables() WHERE table_name = 'kpis'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_discover_project_dir() {
        let temp_dir = std::env::temp_dir().join(format!(
            "duckpack_test_disc_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let sub_proj = temp_dir.join("duckpack");
        std::fs::create_dir_all(sub_proj.join("queries")).unwrap();
        std::fs::write(sub_proj.join("duckpack.toml"), "[project]\nname=\"test\"").unwrap();

        // From root temp_dir, it should discover sub_proj
        let discovered = discover_project_dir(&temp_dir);
        assert_eq!(discovered, sub_proj);

        // From inside sub_proj, it should discover sub_proj directly
        let direct = discover_project_dir(&sub_proj);
        assert_eq!(direct, sub_proj);

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_resolve_target_config_structured_host_port() {
        let temp_dir = std::env::temp_dir().join(format!(
            "duckpack_test_cfg_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&temp_dir).unwrap();
        let toml_content = r#"
[target]
host = "analytics.company.com"
port = 9494
database = ["analytics", "orders"]
insecure = true
token_env = "MY_CUSTOM_TOKEN"

[env.prod]
db = "quack:prod.internal:443"
database = "prod_cat"
insecure = false
token_env = "PROD_TOKEN"
"#;
        std::fs::write(temp_dir.join("duckpack.toml"), toml_content).unwrap();

        // Target defaults
        let cfg = resolve_target_config(&temp_dir, None);
        assert_eq!(cfg.db.as_deref(), Some("quack:analytics.company.com:9494"));
        assert_eq!(
            cfg.catalogs,
            vec!["analytics".to_string(), "orders".to_string()]
        );
        assert_eq!(cfg.insecure, Some(true));
        assert_eq!(cfg.token_env.as_deref(), Some("MY_CUSTOM_TOKEN"));

        // Prod environment overrides
        let prod_cfg = resolve_target_config(&temp_dir, Some("prod"));
        assert_eq!(prod_cfg.db.as_deref(), Some("quack:prod.internal:443"));
        assert_eq!(prod_cfg.catalogs, vec!["prod_cat".to_string()]);
        assert_eq!(prod_cfg.insecure, Some(false));
        assert_eq!(prod_cfg.token_env.as_deref(), Some("PROD_TOKEN"));

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_resolve_target_config_mssql_windows_auth() {
        let temp_dir = std::env::temp_dir().join(format!(
            "duckpack_mssql_cfg_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&temp_dir).unwrap();
        let toml_content = r#"
[target]
provider = "mssql"
server = "sqlserver.corp.local"
database = "FinanceDB"
trusted_connection = true

[env.sql_auth]
provider = "mssql"
server = "sqlserver.corp.local"
database = "FinanceDB"
trusted_connection = false
username = "finance_app"
password = "AppPassword123"
"#;
        std::fs::write(temp_dir.join("duckpack.toml"), toml_content).unwrap();

        let cfg = resolve_target_config(&temp_dir, None);
        assert!(cfg.db.is_some());
        let db_str = cfg.db.unwrap();
        assert!(db_str.starts_with("mssql:Server=sqlserver.corp.local,1433"));
        assert!(db_str.contains(";Database=FinanceDB"));
        assert!(db_str.contains(";Trusted_Connection=yes"));
        assert!(!db_str.contains("User Id="));
        assert!(!db_str.contains("Password="));

        let sql_cfg = resolve_target_config(&temp_dir, Some("sql_auth"));
        assert!(sql_cfg.db.is_some());
        let sql_db = sql_cfg.db.unwrap();
        assert!(sql_db.starts_with("mssql:Server=sqlserver.corp.local,1433"));
        assert!(sql_db.contains(";Database=FinanceDB"));
        assert!(!sql_db.contains("Trusted_Connection=yes"));
        assert!(sql_db.contains(";User Id=finance_app"));
        assert!(sql_db.contains(";Password=AppPassword123"));

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_resolve_target_config_ducklake() {
        let temp_dir = std::env::temp_dir().join(format!(
            "duckpack_ducklake_cfg_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&temp_dir).unwrap();
        let toml_content = r#"
[target]
provider = "ducklake"
server = "postgres.lake.local"
port = 5432
database = "lake_metadata"
username = "lake_user"
password = "LakePassword"
data_path = "s3://my-lake-bucket/data/"

[env.local_lake]
provider = "ducklake"
catalog = "metadata.ducklake"
data_path = "./parquet_data/"

[env.quack_lake]
provider = "ducklake"
backend = "quack"
server = "quack.lake.local"
port = 9494
data_path = "s3://quack-lake/data/"

[env.sqlite_lake]
provider = "ducklake"
backend = "sqlite"
database = "custom_meta.db"
data_path = "/lake/data/"
"#;
        std::fs::write(temp_dir.join("duckpack.toml"), toml_content).unwrap();

        let cfg = resolve_target_config(&temp_dir, None);
        assert!(cfg.db.is_some());
        let db_str = cfg.db.unwrap();
        assert!(db_str.starts_with("ducklake:postgres:host=postgres.lake.local port=5432 dbname=lake_metadata"));
        assert!(db_str.contains("user=lake_user"));
        assert!(db_str.contains("password=LakePassword"));
        assert!(db_str.contains("(DATA_PATH 's3://my-lake-bucket/data/')"));

        let local_cfg = resolve_target_config(&temp_dir, Some("local_lake"));
        assert!(local_cfg.db.is_some());
        let local_db = local_cfg.db.unwrap();
        assert_eq!(local_db, "ducklake:metadata.ducklake (DATA_PATH './parquet_data/')");

        let quack_cfg = resolve_target_config(&temp_dir, Some("quack_lake"));
        assert!(quack_cfg.db.is_some());
        let quack_db = quack_cfg.db.unwrap();
        assert_eq!(quack_db, "ducklake:quack:quack.lake.local:9494 (DATA_PATH 's3://quack-lake/data/')");

        let sqlite_cfg = resolve_target_config(&temp_dir, Some("sqlite_lake"));
        assert!(sqlite_cfg.db.is_some());
        let sqlite_db = sqlite_cfg.db.unwrap();
        assert_eq!(sqlite_db, "ducklake:sqlite:custom_meta.db (DATA_PATH '/lake/data/')");

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_resolve_quack_token_hierarchy() {
        // CLI token takes precedence
        assert_eq!(
            resolve_quack_token(Some("cli_override"), Some("CUSTOM_VAR")),
            Some("cli_override".to_string())
        );

        unsafe {
            // Custom token_env next
            std::env::set_var("CUSTOM_QUACK_VAR", "secret_from_custom");
            assert_eq!(
                resolve_quack_token(None, Some("CUSTOM_QUACK_VAR")),
                Some("secret_from_custom".to_string())
            );

            // QUACK_TOKEN next
            std::env::remove_var("CUSTOM_QUACK_VAR");
            std::env::set_var("QUACK_TOKEN", "secret_from_quack_token");
            assert_eq!(
                resolve_quack_token(None, None),
                Some("secret_from_quack_token".to_string())
            );

            // DUCKDB_QUACK_TOKEN fallback
            std::env::remove_var("QUACK_TOKEN");
            std::env::set_var("DUCKDB_QUACK_TOKEN", "secret_from_duckdb_token");
            assert_eq!(
                resolve_quack_token(None, None),
                Some("secret_from_duckdb_token".to_string())
            );

            std::env::remove_var("DUCKDB_QUACK_TOKEN");
            assert_eq!(resolve_quack_token(None, None), None);
        }
    }

    #[test]
    fn test_load_env_files_base_and_profile_overrides() {
        let temp_dir = std::env::temp_dir().join(format!(
            "duckpack_test_env_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&temp_dir).unwrap();

        // Base .env has common token and host
        std::fs::write(
            temp_dir.join(".env"),
            "DUCKPACK_TEST_BASE_TOKEN=base_secret_123\nDUCKPACK_TEST_COMMON_VAR=base_val\n",
        )
        .unwrap();
        // Profile .env.dev overrides COMMON_VAR but leaves base token
        std::fs::write(
            temp_dir.join(".env.dev"),
            "DUCKPACK_TEST_COMMON_VAR=dev_val\n",
        )
        .unwrap();

        load_env_files(&temp_dir, Some("dev"));

        assert_eq!(
            std::env::var("DUCKPACK_TEST_BASE_TOKEN").as_deref(),
            Ok("base_secret_123")
        );
        assert_eq!(
            std::env::var("DUCKPACK_TEST_COMMON_VAR").as_deref(),
            Ok("dev_val")
        );

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_target_resolver_hierarchy_and_spec() {
        let temp_dir = std::env::temp_dir().join(format!(
            "duckpack_test_resolver_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&temp_dir).unwrap();

        // 1. Default with no config
        let spec_default = TargetResolver::resolve(&temp_dir, None, None, None, None, None);
        assert_eq!(spec_default.db, "local.duckdb");
        assert_eq!(spec_default.token, None);
        assert!(!spec_default.disable_ssl);
        assert!(!spec_default.is_remote());
        assert!(!spec_default.is_quack);
        assert!(!spec_default.is_motherduck);
        assert!(spec_default.catalogs.is_empty());

        // 2. duckpack.toml with target defaults and env.prod
        let toml_content = r#"
[target]
db = "warehouse.duckdb"
catalog = "main_warehouse"

[env.prod]
db = "quack:prod.internal:9494"
database = ["prod_analytics", "prod_events"]
insecure = true
token_env = "TEST_RESOLVER_PROD_TOKEN"
"#;
        std::fs::write(temp_dir.join("duckpack.toml"), toml_content).unwrap();

        unsafe {
            std::env::set_var("TEST_RESOLVER_PROD_TOKEN", "super_secret_prod_token");
        }

        // Resolving prod environment profile
        let spec_prod = TargetResolver::resolve(&temp_dir, Some("prod"), None, None, None, None);
        assert_eq!(spec_prod.db, "quack:prod.internal:9494");
        assert_eq!(spec_prod.token.as_deref(), Some("super_secret_prod_token"));
        assert!(spec_prod.disable_ssl);
        assert!(spec_prod.is_remote());
        assert!(spec_prod.is_quack);
        assert!(!spec_prod.is_motherduck);
        assert_eq!(
            spec_prod.catalogs,
            vec!["prod_analytics".to_string(), "prod_events".to_string()]
        );

        // 3. Explicit overrides take precedence
        let spec_override = TargetResolver::resolve(
            &temp_dir,
            Some("prod"),
            Some("override.duckdb"),
            Some("explicit_cli_token"),
            Some(false),
            Some(vec!["custom_cat".to_string()]),
        );
        assert_eq!(spec_override.token.as_deref(), Some("explicit_cli_token"));
        assert_eq!(spec_override.catalogs, vec!["custom_cat".to_string()]);

        unsafe {
            std::env::remove_var("TEST_RESOLVER_PROD_TOKEN");
        }
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_load_ignore_rules() {
        let temp_dir = std::env::temp_dir().join(format!(
            "dp_ignore_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::create_dir_all(&temp_dir);

        // When no file exists, returns empty
        assert!(load_ignore_rules(&temp_dir).is_empty());

        // Create .duckpackignore with comments, blank lines, and patterns
        let content = "\n# Comments are ignored\nscratch_*\ntemp_*\n\nraw_data\n";
        std::fs::write(temp_dir.join(".duckpackignore"), content).unwrap();

        let rules = load_ignore_rules(&temp_dir);
        assert_eq!(
            rules,
            vec![
                "scratch_*".to_string(),
                "temp_*".to_string(),
                "raw_data".to_string()
            ]
        );

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_glob_match() {
        assert!(glob_match("temp_*", "temp_users"));
        assert!(!glob_match("temp_*", "users_temp"));
        assert!(glob_match("*_backup", "users_backup"));
        assert!(glob_match("scripts/pre-deploy/*", "scripts/pre-deploy/01_once.sql"));
        assert!(!glob_match("scripts/pre-deploy/*", "scripts/post-deploy/01_once.sql"));
        assert!(glob_match("01_once.sql", "01_once.sql"));
        assert!(!glob_match("01_once.sql", "02_once.sql"));
        assert!(glob_match("*", "anything"));
        assert!(glob_match("*.sql", "test.sql"));
        assert!(!glob_match("*.sql", "test.csv"));
    }

    #[test]
    fn test_is_file_ignored_various_patterns() {
        let temp_dir = std::path::PathBuf::from("/mock/project");

        // 1. Direct filename
        let rules = vec!["01_seed_once.sql".to_string()];
        assert!(is_file_ignored(&temp_dir.join("scripts/pre-deploy/01_seed_once.sql"), &temp_dir, &rules));
        assert!(is_file_ignored(&temp_dir.join("scripts/post-deploy/01_seed_once.sql"), &temp_dir, &rules));
        assert!(!is_file_ignored(&temp_dir.join("scripts/pre-deploy/02_normal.sql"), &temp_dir, &rules));

        // 2. Relative path from project root
        let rules = vec!["scripts/pre-deploy/01_seed_once.sql".to_string()];
        assert!(is_file_ignored(&temp_dir.join("scripts/pre-deploy/01_seed_once.sql"), &temp_dir, &rules));
        assert!(!is_file_ignored(&temp_dir.join("scripts/post-deploy/01_seed_once.sql"), &temp_dir, &rules));

        // 3. Subpath / suffix match
        let rules = vec!["pre-deploy/01_seed_once.sql".to_string()];
        assert!(is_file_ignored(&temp_dir.join("scripts/pre-deploy/01_seed_once.sql"), &temp_dir, &rules));
        assert!(!is_file_ignored(&temp_dir.join("scripts/post-deploy/01_seed_once.sql"), &temp_dir, &rules));

        // 4. Directory pattern with trailing slash
        let rules = vec!["scripts/post-deploy/".to_string()];
        assert!(is_file_ignored(&temp_dir.join("scripts/post-deploy/01_cleanup.sql"), &temp_dir, &rules));
        assert!(is_file_ignored(&temp_dir.join("scripts/post-deploy/nested/deep.sql"), &temp_dir, &rules));
        assert!(!is_file_ignored(&temp_dir.join("scripts/pre-deploy/01_seed.sql"), &temp_dir, &rules));

        // 5. Wildcard path patterns
        let rules = vec!["scripts/pre-deploy/*".to_string(), "tables/staging_*".to_string()];
        assert!(is_file_ignored(&temp_dir.join("scripts/pre-deploy/any.sql"), &temp_dir, &rules));
        assert!(!is_file_ignored(&temp_dir.join("scripts/post-deploy/any.sql"), &temp_dir, &rules));
        assert!(is_file_ignored(&temp_dir.join("tables/staging_users.sql"), &temp_dir, &rules));
        assert!(!is_file_ignored(&temp_dir.join("tables/users.sql"), &temp_dir, &rules));

        // 6. Leading slash and ./ stripping
        let rules = vec!["/scripts/pre-deploy/run.sql".to_string(), "./tables/old.sql".to_string()];
        assert!(is_file_ignored(&temp_dir.join("scripts/pre-deploy/run.sql"), &temp_dir, &rules));
        assert!(is_file_ignored(&temp_dir.join("tables/old.sql"), &temp_dir, &rules));
    }

    #[test]
    fn test_is_object_ignored() {
        let rules = vec![
            "temp_*".to_string(),
            "*_backup".to_string(),
            "tables/legacy.sql".to_string(),
            "core.secret_table".to_string(),
            "sequences/custom_seq.sql".to_string(),
        ];

        // Wildcard prefix / suffix
        assert!(is_object_ignored("temp_users", &rules));
        assert!(is_object_ignored("orders_backup", &rules));
        assert!(!is_object_ignored("normal_users", &rules));

        // File-path rule matching object stem
        assert!(is_object_ignored("legacy", &rules));
        assert!(is_object_ignored("custom_seq", &rules));

        // Qualified and unqualified matching
        assert!(is_object_ignored("core.secret_table", &rules));
        let rules_unqual = vec!["secret_table".to_string()];
        assert!(is_object_ignored("secret_table", &rules_unqual));
        assert!(is_object_ignored("core.secret_table", &rules_unqual));
    }

    #[test]
    fn test_pre_and_post_deploy_ignore_execution() {
        let temp_dir = std::env::temp_dir().join(format!(
            "dp_deploy_ignore_test_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let pre_dir = temp_dir.join("scripts").join("pre-deploy");
        let post_dir = temp_dir.join("scripts").join("post-deploy");
        std::fs::create_dir_all(&pre_dir).unwrap();
        std::fs::create_dir_all(&post_dir).unwrap();

        // Pre-deploy scripts: one to run once then ignore, one always active
        std::fs::write(
            pre_dir.join("01_seed_once.sql"),
            "CREATE TABLE should_not_exist (id INT);",
        )
        .unwrap();
        std::fs::write(
            pre_dir.join("02_always_active.sql"),
            "CREATE TABLE active_pre (id INT);",
        )
        .unwrap();

        // Post-deploy scripts
        std::fs::write(
            post_dir.join("99_post_ignored.sql"),
            "CREATE TABLE post_should_not_exist (id INT);",
        )
        .unwrap();
        std::fs::write(
            post_dir.join("99_post_active.sql"),
            "CREATE TABLE active_post (id INT);",
        )
        .unwrap();

        // .duckpackignore ignores the one-time script and the post-deploy script
        let ignore_content = "scripts/pre-deploy/01_seed_once.sql\n99_post_ignored.sql\n";
        std::fs::write(temp_dir.join(".duckpackignore"), ignore_content).unwrap();

        let mut target_conn = Connection::open_in_memory().unwrap();
        let diff = crate::diff::DiffResult { items: Vec::new() };

        // Apply diff with executor
        let res = apply_diff_with_executor(
            &mut target_conn,
            &diff,
            false,
            false,
            &temp_dir,
            None,
        );
        assert!(res.is_ok());

        // Verify ignored scripts were NOT executed
        let pre_ignored_exists = target_conn
            .execute_batch("SELECT * FROM should_not_exist")
            .is_ok();
        assert!(!pre_ignored_exists, "01_seed_once.sql was supposed to be ignored!");

        let post_ignored_exists = target_conn
            .execute_batch("SELECT * FROM post_should_not_exist")
            .is_ok();
        assert!(!post_ignored_exists, "99_post_ignored.sql was supposed to be ignored!");

        // Verify active scripts WERE executed
        assert!(target_conn.execute_batch("SELECT * FROM active_pre").is_ok());
        assert!(target_conn.execute_batch("SELECT * FROM active_post").is_ok());

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_save_cloud_secret_to_env() {
        let temp_dir = std::env::temp_dir().join(format!(
            "dp_env_sec_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::create_dir_all(&temp_dir);

        // Prepopulate with unrelated env variable
        std::fs::write(temp_dir.join(".env"), "FOO=bar\nAWS_REGION=us-west-2\n").unwrap();

        let res = save_cloud_secret_to_env(
            &temp_dir,
            "test_key_id",
            "test_secret_val",
            "us-east-1",
            "http://localhost:9000",
            "s3://my-test-lake",
        );
        assert!(res.is_ok());

        let env_content = std::fs::read_to_string(temp_dir.join(".env")).unwrap();
        assert!(env_content.contains("FOO=bar"));
        assert!(env_content.contains("AWS_ACCESS_KEY_ID=test_key_id"));
        assert!(env_content.contains("AWS_SECRET_ACCESS_KEY=test_secret_val"));
        assert!(env_content.contains("AWS_REGION=us-east-1"));
        assert!(env_content.contains("AWS_ENDPOINT=http://localhost:9000"));
        assert!(env_content.contains("DUCKPACK_S3_BUCKET=s3://my-test-lake"));

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_append_cloud_bucket_to_env() {
        let temp_dir = std::env::temp_dir().join(format!(
            "dp_env_bkt_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::create_dir_all(&temp_dir);

        // First append creates DUCKPACK_S3_BUCKET
        let _ = append_cloud_bucket_to_env(&temp_dir, "s3://lake-one");
        let content1 = std::fs::read_to_string(temp_dir.join(".env")).unwrap();
        assert!(content1.contains("DUCKPACK_S3_BUCKET=s3://lake-one"));

        // Second append turns it into comma-separated DUCKPACK_S3_BUCKETS
        let _ = append_cloud_bucket_to_env(&temp_dir, "s3://lake-two");
        let content2 = std::fs::read_to_string(temp_dir.join(".env")).unwrap();
        assert!(content2.contains("DUCKPACK_S3_BUCKETS=s3://lake-one,s3://lake-two"));

        // Third append of duplicate should not duplicate
        let _ = append_cloud_bucket_to_env(&temp_dir, "s3://lake-two");
        let content3 = std::fs::read_to_string(temp_dir.join(".env")).unwrap();
        assert!(content3.contains("DUCKPACK_S3_BUCKETS=s3://lake-one,s3://lake-two"));

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_security_random_token_and_env_permissions() {
        let t1 = generate_random_hex_token();
        let t2 = generate_random_hex_token();
        assert_eq!(t1.len(), 64);
        assert_eq!(t2.len(), 64);
        assert_ne!(t1, t2);
        assert!(t1.chars().all(|c| c.is_ascii_hexdigit()));

        let temp_dir = std::env::temp_dir().join(format!(
            "dp_env_sec_{}",
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
        ));
        std::fs::create_dir_all(&temp_dir).unwrap();

        save_cloud_secret_to_env(
            &temp_dir,
            "my_key\r\nINJECTED_VAR=evil",
            "my_secret\nSECOND_INJECT=1",
            "us-east-1",
            "",
            "",
        )
        .unwrap();

        let env_path = temp_dir.join(".env");
        let content = std::fs::read_to_string(&env_path).unwrap();
        assert!(!content.contains("\nINJECTED_VAR="));
        assert!(!content.contains("\nSECOND_INJECT="));

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&env_path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "Expected .env mode 0600, got {:o}", mode);
        }

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_ducklake_and_mssql_catalogs_not_polluted_by_dsn_database() {
        let temp_dir = std::env::temp_dir().join(format!(
            "dp_dl_cat_clean_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&temp_dir).unwrap();

        let toml_content = r#"
[env.dl_file]
provider = "ducklake"
backend = "file"
database = "my_meta.ducklake"
data_path = "./lake_data/"

[env.dl_pg]
provider = "ducklake"
host = "localhost"
database = "lake_metadata"
catalogs = ["custom_cat"]

[env.mssql_srv]
provider = "mssql"
host = "localhost"
database = "FinanceDB"
"#;
        std::fs::write(temp_dir.join("duckpack.toml"), toml_content).unwrap();

        let dl_file_cfg = resolve_target_config(&temp_dir, Some("dl_file"));
        assert_eq!(
            dl_file_cfg.db.as_deref(),
            Some("ducklake:my_meta.ducklake (DATA_PATH './lake_data/')")
        );
        assert!(
            dl_file_cfg.catalogs.is_empty(),
            "DuckLake file database key must not pollute cfg.catalogs, got: {:?}",
            dl_file_cfg.catalogs
        );

        let dl_pg_cfg = resolve_target_config(&temp_dir, Some("dl_pg"));
        assert_eq!(dl_pg_cfg.catalogs, vec!["custom_cat".to_string()]);

        let mssql_cfg = resolve_target_config(&temp_dir, Some("mssql_srv"));
        assert!(
            mssql_cfg.catalogs.is_empty(),
            "MSSQL database key must not pollute cfg.catalogs, got: {:?}",
            mssql_cfg.catalogs
        );

        let body = build_s3_secret_body(
            "minioadmin",
            "miniosecret",
            "us-east-1",
            "http://localhost:9000/",
            true,
        );
        assert!(body.contains("TYPE S3"));
        assert!(body.contains("KEY_ID 'minioadmin'"));
        assert!(body.contains("SECRET 'miniosecret'"));
        assert!(body.contains("ENDPOINT 'localhost:9000'"));
        assert!(body.contains("URL_STYLE 'path'"));
        assert!(body.contains("USE_SSL false"));

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_security_credential_redaction_and_dsn_escaping() {
        let mssql_dsn = format_mssql_connection_string(
            "sql.internal",
            "1433",
            "analytics",
            "sa",
            "pa;ss}word",
            false,
            true,
            false,
        );
        assert!(
            mssql_dsn.contains("Password={pa;ss}}word}"),
            "Expected ADO.NET brace escaping for semicolon/brace in password, got: {}",
            mssql_dsn
        );
        let redacted_mssql = redact_connection_credentials(&mssql_dsn);
        assert!(!redacted_mssql.contains("pa;ss"));
        assert!(redacted_mssql.contains("Password=***"));

        let pg_dsn = format_ducklake_target(
            "postgres",
            "pg.internal",
            "5432",
            "lake_meta",
            "pguser",
            "my secret'pass",
            "",
        )
        .unwrap();
        assert!(
            pg_dsn.contains("password='my secret\\'pass'"),
            "Expected libpq single-quote escaping for password with spaces/quotes, got: {}",
            pg_dsn
        );
        let redacted_pg = redact_connection_credentials(&pg_dsn);
        assert!(!redacted_pg.contains("secret"));
        assert!(redacted_pg.contains("password=***"));

        let uri = "postgres://admin:super_secret_123@db.example.com:5432/prod";
        let redacted_uri = redact_connection_credentials(uri);
        assert_eq!(redacted_uri, "postgres://admin:***@db.example.com:5432/prod");

        let sql = "CREATE SECRET (TYPE S3, KEY_ID 'AKIA123', SECRET 'top_secret_val', TOKEN 'tok_abc');";
        assert!(should_redact_sql_history(sql));
        let redacted_sql = redact_connection_credentials(sql);
        assert!(!redacted_sql.contains("top_secret_val"));
        assert!(!redacted_sql.contains("tok_abc"));
    }

    #[test]
    fn test_ducklake_phase3_rewrite_guard_and_snapshot_tagging() {
        struct MockDuckLakeExec {
            executed: Vec<String>,
        }
        impl DbExecutor for MockDuckLakeExec {
            fn execute(&mut self, sql: &str) -> Result<()> {
                self.executed.push(sql.to_string());
                Ok(())
            }
            fn execute_batch(&mut self, sql: &str) -> Result<()> {
                self.executed.push(sql.to_string());
                Ok(())
            }
            fn query_rows(&mut self, sql: &str) -> Result<(Vec<String>, Vec<Vec<String>>)> {
                if sql.contains("SELECT current_database()") {
                    Ok((vec!["current_database()".to_string()], vec![vec!["lake_prod".to_string()]]))
                } else if sql.contains("FROM duckdb_databases()") {
                    Ok((
                        vec![
                            "database_name".to_string(),
                            "type".to_string(),
                            "path".to_string(),
                            "readonly".to_string(),
                        ],
                        vec![vec![
                            "lake_prod".to_string(),
                            "ducklake".to_string(),
                            "s3://my-bucket/warehouse/".to_string(),
                            "false".to_string(),
                        ]],
                    ))
                } else {
                    Ok((vec![], vec![]))
                }
            }
        }

        let mut exec = MockDuckLakeExec { executed: Vec::new() };

        // 1. Verify validate_target_catalog_exec permits ducklake for apply
        let validated = diff::validate_target_catalog_exec(&mut exec, Some("lake_prod"), true).unwrap();
        assert_eq!(validated, "lake_prod");

        // 2. Verify RecreateTable is blocked by default when targeting a DuckLake catalog
        let temp_dir = std::env::temp_dir().join(format!(
            "dp_lake_phase3_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::create_dir_all(&temp_dir);

        let recreate_diff = diff::DiffResult {
            items: vec![diff::DiffItem {
                item_type: diff::DiffItemType::RecreateTable,
                obj_name: "orders".to_string(),
                sql: "CREATE TABLE orders__duckpack_swap AS SELECT * FROM orders;".to_string(),
                old_def: Some("CREATE TABLE orders (id INT);".to_string()),
                new_def: Some("CREATE TABLE orders (id INT PRIMARY KEY);".to_string()),
                selected: true,
                ignored: false,
                locked: false,
            }],
        };

        unsafe {
            std::env::remove_var("DUCKPACK_ALLOW_LAKE_REWRITE");
        }
        let err = apply_diff_with_lake_options(
            &mut exec,
            &recreate_diff,
            true,
            false,
            false,
            &temp_dir,
            Some("lake_prod"),
        )
        .unwrap_err()
        .to_string();
        assert!(
            err.contains("Cannot execute physical copy-and-swap on DuckLake table 'orders'"),
            "Expected DuckLake copy-and-swap guard error, got: {}",
            err
        );
        assert!(err.contains("--allow-lake-rewrite"));
        assert!(exec.executed.is_empty(), "No SQL should execute when guard blocks");

        // 3. Verify allow_lake_rewrite = true permits the migration and tags the DuckLake snapshot commit
        apply_diff_with_lake_options(
            &mut exec,
            &recreate_diff,
            true,
            true,
            false,
            &temp_dir,
            Some("lake_prod"),
        )
        .unwrap();
        assert!(
            exec.executed
                .iter()
                .any(|s| s.contains("CALL \"lake_prod\".set_commit_message('duckpack', 'duckpack apply: [v")),
            "Expected structured DuckLake snapshot commit message call, got: {:?}",
            exec.executed
        );

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_quack_loopback_ipv6_and_r2_helpers() {
        assert!(is_quack_loopback("quack:localhost:9494"));
        assert!(is_quack_loopback("quack:127.0.0.1:9494"));
        assert!(is_quack_loopback("quack:[::1]:9494"));
        assert!(!is_quack_loopback("quack:localhost.example.com:9494"));
        assert!(!is_quack_loopback("quack:my-127.0.0.1.nip.io:9494"));

        assert!(is_quack_zero_host("quack:0.0.0.0:9494"));
        assert!(!is_quack_zero_host("quack:10.0.0.0:9494"));

        assert_eq!(format_quack_endpoint("::1", "9494"), "quack:[::1]:9494");
        assert_eq!(format_quack_endpoint("[::1]", "9494"), "quack:[::1]:9494");
        assert_eq!(format_quack_endpoint("db.internal", "443"), "quack:db.internal:443");

        assert_eq!(
            format_ducklake_target("postgres", "postgresql://u:p@h:5432/meta", "", "", "", "", "")
                .unwrap(),
            "ducklake:postgres:postgresql://u:p@h:5432/meta"
        );
        assert_eq!(
            extract_r2_account_id("https://acct123.r2.cloudflarestorage.com"),
            Some("acct123".to_string())
        );
        let r2_body = build_r2_secret_body("k", "s", "acct123");
        assert!(r2_body.contains("TYPE R2"));
        assert!(r2_body.contains("ACCOUNT_ID 'acct123'"));

        // Frozen remote DuckLake URLs omit the ducklake: prefix so callers pass (TYPE ducklake)
        assert_eq!(
            format_ducklake_target(
                "file",
                "",
                "",
                "",
                "",
                "",
                "https://blobs.duckdb.org/datalake/tpch-sf1.ducklake"
            )
            .unwrap(),
            "https://blobs.duckdb.org/datalake/tpch-sf1.ducklake"
        );
        assert!(is_remote_ducklake_url("s3://bucket/frozen.ducklake"));
        assert!(!is_remote_ducklake_url("./local.ducklake"));

        // MSSQL named instance preserves dynamic port resolution (omits default ,1433)
        let named_inst = format_mssql_connection_string(
            "myhost\\SQLEXPRESS",
            "1433",
            "mydb",
            "app",
            "secret",
            false,
            false,
            false,
        );
        assert_eq!(
            named_inst,
            "Server=myhost\\SQLEXPRESS;Database=mydb;User Id=app;Password=secret"
        );

        // MSSQL pre-ported host does not duplicate port
        let pre_ported = format_mssql_connection_string(
            "sql.internal,14330",
            "1433",
            "mydb",
            "app",
            "secret",
            false,
            false,
            false,
        );
        assert!(pre_ported.starts_with("Server=sql.internal,14330;Database=mydb"));

        // MSSQL native URI passes through untouched
        let mssql_uri = format_mssql_connection_string(
            "mssql://app:secret@sql.internal:1433/mydb?encrypt=true",
            "1433",
            "mydb",
            "app",
            "secret",
            false,
            false,
            false,
        );
        assert_eq!(
            mssql_uri,
            "mssql://app:secret@sql.internal:1433/mydb?encrypt=true"
        );
    }
}
