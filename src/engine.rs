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
        "SELECT count(*) FROM duckdb_databases() WHERE database_name = 'target_db' AND (lower(type) = 'quack' OR path LIKE 'quack:%')",
    )
    .ok()
    .and_then(|(_, rows)| rows.into_iter().next())
    .and_then(|r| r.into_iter().next())
    .and_then(|v| v.parse::<i64>().ok())
    .unwrap_or(0)
        > 0
}

/// Determines if the provided quack connection URI targets the 0.0.0.0 local unspecified address.
pub fn is_quack_zero_host(db: &str) -> bool {
    let stripped = db
        .strip_prefix("quack://")
        .or_else(|| db.strip_prefix("quack:"))
        .unwrap_or(db);
    stripped.starts_with("0.0.0.0") || stripped.starts_with("[::]")
}

/// Determines if the provided quack connection URI targets a loopback address (127.0.0.1, localhost, [::1]).
pub fn is_quack_loopback(db: &str) -> bool {
    let stripped = db
        .strip_prefix("quack://")
        .or_else(|| db.strip_prefix("quack:"))
        .unwrap_or(db);
    stripped.starts_with("127.0.0.1")
        || stripped.starts_with("localhost")
        || stripped.starts_with("[::1]")
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

/// Builds the SQL ATTACH statement for a Quack database.
pub fn build_quack_attach_sql(db: &str, disable_ssl: bool) -> String {
    let safe_db = db.replace("'", "''");
    if disable_ssl {
        format!("ATTACH '{}' AS target_db (DISABLE_SSL true);", safe_db)
    } else {
        format!("ATTACH '{}' AS target_db;", safe_db)
    }
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
                 $ duckdb {} -c \"INSTALL quack; LOAD quack; CALL quack_serve('quack:localhost:9494');\"\n\
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

        let token = resolve_quack_token(quack_token, None);

        if let Some(token) = token {
            let safe_token = token.replace('\'', "''");
            conn.execute(
                &format!("CREATE SECRET (TYPE QUACK, TOKEN '{}');", safe_token),
                [],
            )
            .with_context(|| "Failed to set quack auth secret")?;
        }

        let disable_ssl = should_disable_quack_ssl(db, quack_disable_ssl);
        let attach_sql = build_quack_attach_sql(db, disable_ssl);

        let attach_res = conn.execute(&attach_sql, []);
        if let Err(e) = attach_res {
            let err_msg = e.to_string();
            if err_msg.contains("Could not find a Quack authentication token")
                || err_msg.contains("Authentication failed")
            {
                return Err(anyhow::anyhow!(
                    "Failed to attach remote database '{}': {}\n\n\
                    Troubleshooting:\n\
                      1. Check that your Quack token is set in .env (QUACK_TOKEN) or duckpack.toml (token_env).\n\
                      2. If using --env <profile>, ensure either .env or .env.<profile> contains the token.\n\
                      3. Alternatively, pass --quack-token <token> directly.",
                    db,
                    err_msg
                ));
            }
            return Err(anyhow::anyhow!(
                "Failed to attach remote database '{}': {}",
                db,
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
        let conn_str = db.strip_prefix("mssql:").unwrap_or(db);
        let mut final_conn_str = conn_str.to_string();
        if cfg!(target_os = "windows")
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
                db,
                e
            ));
        }
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
                db,
                e
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
                let safe_orig = orig.replace('\"', "\"\"");
                let safe_bt = bt.replace('\"', "\"\"");
                let restore_sql = format!(
                    "CREATE OR REPLACE TEMP TABLE \"{}\" AS SELECT * FROM target_db.query('SELECT * FROM memory.\"{}\"');",
                    safe_orig, safe_bt
                );
                let _ = conn.execute(&restore_sql, []);
                let _ = conn.execute(
                    &format!("DROP TABLE IF EXISTS target_db.\"{}\";", safe_bt),
                    [],
                );
                let _ = conn.execute(
                    &format!(
                        "CALL target_db.query('DROP TABLE IF EXISTS \"{}\";');",
                        safe_bt.replace('\'', "''")
                    ),
                    [],
                );
                let _ = conn.execute(
                    &format!(
                        "CALL target_db.query('DROP TABLE IF EXISTS memory.\"{}\";');",
                        safe_bt.replace('\'', "''")
                    ),
                    [],
                );
            }
        }
    }

    // 2. Restore views
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
            let _ = conn.execute(&vsql, []);
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
            let safe_bt = bt.replace('\"', "\"\"");
            let _ = conn.execute(
                &format!("DROP TABLE IF EXISTS target_db.\"{}\";", safe_bt),
                [],
            );
            let _ = conn.execute(
                &format!(
                    "CALL target_db.query('DROP TABLE IF EXISTS \"{}\";');",
                    safe_bt.replace('\'', "''")
                ),
                [],
            );
            let _ = conn.execute(
                &format!(
                    "CALL target_db.query('DROP TABLE IF EXISTS memory.\"{}\";');",
                    safe_bt.replace('\'', "''")
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
        let safe_t = t.replace('\"', "\"\"");
        let stage_sql = format!(
            "CREATE OR REPLACE TABLE target_db.\"__duckpack_temp_bridge_{}\" AS SELECT * FROM \"{}\";",
            safe_t, safe_t
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
            let safe_vname = vname.replace('\'', "''");
            let safe_vsql = vsql.replace('\'', "''");
            let ins_sql = format!(
                "CALL target_db.query('INSERT INTO memory.__duckpack_temp_views_sync_back VALUES (''{}'', ''{}'');');",
                safe_vname, safe_vsql
            );
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
    let is_quack = is_quack_target_attached(exec);

    let orig_db: String = if is_quack {
        exec.query_rows("SELECT * FROM target_db.query('SELECT current_database()')")
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
    // If connected via Quack, 'target_db' is merely DuckPack's client-side attachment alias.
    // The remote Quack database server does NOT have a catalog named 'target_db'.
    // Therefore, any target catalog pointing to 'target_db' must resolve to the remote server's active database (orig_db).
    if is_quack && (target_cat == "target_db" || target_cat.is_empty()) {
        target_cat = &orig_db;
    }

    if !is_quack && !target_cat.is_empty() && target_cat != orig_db {
        exec.execute_batch(&format!("USE \"{}\";", target_cat.replace('\"', "\"\"")))?;
    }

    let has_recreate = diff.items.iter().any(|i| {
        i.selected && !i.ignored && i.item_type == diff::DiffItemType::RecreateTable && force_drop
    });
    let is_remote_effective = (is_remote || is_quack) && !has_recreate;

    if is_remote_effective {
        if is_quack {
            let _ = exec.execute_batch("CALL target_db.query('BEGIN TRANSACTION;');");
        } else {
            exec.execute_batch("BEGIN TRANSACTION;")?;
        }
    }

    let result = (|| -> Result<()> {
        let mut execute_sql = |sql: &str| -> Result<()> {
            if is_quack {
                let safe_cat = target_cat.replace('\'', "''");
                let safe_sql = sql.replace('\'', "''");
                if !safe_cat.is_empty() {
                    exec.execute_batch(&format!(
                        "CALL target_db.query('USE \"{}\"; {}')",
                        safe_cat, safe_sql
                    ))
                    .with_context(|| {
                        format!("Failed executing DDL on catalog '{}' via Quack", target_cat)
                    })?;
                } else {
                    exec.execute_batch(&format!("CALL target_db.query('{}')", safe_sql))
                        .with_context(|| "Failed executing DDL via Quack".to_string())?;
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
                        structure_to_apply.push(item.sql.clone());
                    }
                }
                diff::DiffItemType::DropTable
                | diff::DiffItemType::DropColumn
                | diff::DiffItemType::DropView
                | diff::DiffItemType::DropMacro
                | diff::DiffItemType::DropTrigger
                | diff::DiffItemType::DropSequence => {
                    if force_drop {
                        drops_to_apply.push(item.sql.clone());
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

        // 4. Triggers
        if !triggers_to_apply.is_empty() {
            execute_sqls_with_retry_custom(triggers_to_apply, &mut execute_sql)?;
        }

        // 5. Drops
        for drop_sql in drops_to_apply {
            let _ = execute_sql(&drop_sql); // Ignore errors if already dropped by cascade
        }
        for drop_sql in drop_schemas {
            let _ = execute_sql(&drop_sql);
        }

        let post_deploy_dir = project_dir.join("scripts").join("post-deploy");
        if post_deploy_dir.exists() {
            execute_directory_filtered(&post_deploy_dir, project_dir, &ignore_rules, &mut execute_sql)?;
        }

        Ok(())
    })();

    if is_remote_effective {
        if result.is_ok() {
            if is_quack {
                let _ = exec.execute_batch("CALL target_db.query('COMMIT;');");
            } else {
                exec.execute_batch("COMMIT;")?;
            }
        } else {
            if is_quack {
                let _ = exec.execute_batch("CALL target_db.query('ROLLBACK;');");
            } else {
                exec.execute_batch("ROLLBACK;")?;
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
            let port = tbl.get("port").and_then(|v| v.as_integer()).unwrap_or(1433);
            let mut conn_str = format!("mssql:Server={},{}", server, port);
            if let Some(db_name) = tbl
                .get("database")
                .or_else(|| tbl.get("db"))
                .and_then(|v| v.as_str())
            {
                conn_str.push_str(&format!(";Database={}", db_name));
            }
            let is_trusted = tbl
                .get("trusted_connection")
                .or_else(|| tbl.get("windows_auth"))
                .and_then(|v| v.as_bool())
                .unwrap_or(cfg!(target_os = "windows"));

            if is_trusted {
                conn_str.push_str(";Trusted_Connection=yes");
            } else {
                if let Some(user) = tbl
                    .get("username")
                    .or_else(|| tbl.get("user"))
                    .or_else(|| tbl.get("user_id"))
                    .and_then(|v| v.as_str())
                {
                    conn_str.push_str(&format!(";User Id={}", user));
                }
                if let Some(pass) = tbl.get("password").and_then(|v| v.as_str()) {
                    conn_str.push_str(&format!(";Password={}", pass));
                }
            }
            if tbl
                .get("insecure")
                .or_else(|| tbl.get("trust_server_certificate"))
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
                || server == "localhost"
                || server == "127.0.0.1"
            {
                conn_str.push_str(";TrustServerCertificate=yes");
            }
            if tbl
                .get("disable_ssl")
                .or_else(|| tbl.get("no_encrypt"))
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
            {
                conn_str.push_str(";Encrypt=no");
            }
            cfg.db = Some(conn_str);
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

    if let Some(val) = tbl.get("database").or_else(|| tbl.get("catalog")) {
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

/// Loads environment variables from project directory:
/// 1. Always loads base `.env` if present.
/// 2. If `env_profile` is provided (e.g. "dev"), loads `.env.<profile>` on top to override base variables.
pub fn load_env_files(project_dir: &std::path::Path, env_profile: Option<&str>) {
    let base_env = project_dir.join(".env");
    if base_env.exists() {
        let _ = dotenvy::from_path(&base_env);
    }
    if let Some(profile) = env_profile {
        if !profile.is_empty() {
            let profile_env = project_dir.join(format!(".env.{}", profile));
            if profile_env.exists() {
                let _ = dotenvy::from_path_override(&profile_env);
            }
        }
    }
}

/// Generates a cryptographically random 256-bit (64-hex-character) token from the OS CSPRNG
/// (`/dev/urandom` on Unix, with a high-entropy hashed fallback).
pub fn generate_random_hex_token() -> String {
    let mut bytes = [0u8; 32];
    let mut filled = false;
    if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
        use std::io::Read;
        if f.read_exact(&mut bytes).is_ok() {
            filled = true;
        }
    }
    if !filled {
        use std::collections::hash_map::RandomState;
        use std::hash::{BuildHasher, Hasher};
        for chunk in bytes.chunks_mut(8) {
            let s = RandomState::new();
            let mut h = s.build_hasher();
            h.write_u128(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or(0),
            );
            h.write_u32(std::process::id());
            let val = h.finish().to_le_bytes();
            for (dst, src) in chunk.iter_mut().zip(val.iter()) {
                *dst = *src;
            }
        }
    }
    let mut hex = String::with_capacity(64);
    for b in &bytes {
        use std::fmt::Write;
        let _ = write!(&mut hex, "{:02x}", b);
    }
    hex
}

/// Strips control characters (`\r`, `\n`, `\0`) from a value before writing to `.env` to prevent newline injection.
fn sanitize_env_value(val: &str) -> String {
    val.chars().filter(|&c| c != '\r' && c != '\n' && c != '\0').collect()
}

/// Writes `.env` contents to disk and restricts permissions to `0600` (owner read/write only) on Unix.
pub fn write_restricted_env_file(env_path: &std::path::Path, content: &str) -> Result<()> {
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .mode(0o600)
            .open(env_path)
            .with_context(|| format!("Failed to open {} for writing", env_path.display()))?;
        let _ = f.set_permissions(std::fs::Permissions::from_mode(0o600));
        f.write_all(content.as_bytes())?;
        return Ok(());
    }
    #[cfg(not(unix))]
    {
        std::fs::write(env_path, content)?;
        Ok(())
    }
}

/// Safely writes or updates S3 / cloud credentials into the project's `.env` file (git-ignored).
pub fn save_cloud_secret_to_env(
    project_dir: &std::path::Path,
    key_id: &str,
    secret_key: &str,
    region: &str,
    endpoint: &str,
    bucket: &str,
) -> Result<()> {
    let env_path = project_dir.join(".env");
    let mut lines: Vec<String> = if env_path.exists() {
        std::fs::read_to_string(&env_path)
            .unwrap_or_default()
            .lines()
            .map(|s| s.to_string())
            .collect()
    } else {
        Vec::new()
    };

    let mut keys_to_set = std::collections::BTreeMap::new();
    let clean_key_id = sanitize_env_value(key_id);
    let clean_secret_key = sanitize_env_value(secret_key);
    let clean_region = sanitize_env_value(region);
    let clean_endpoint = sanitize_env_value(endpoint);
    let clean_bucket = sanitize_env_value(bucket);

    if !clean_key_id.is_empty() {
        keys_to_set.insert("AWS_ACCESS_KEY_ID", clean_key_id);
    }
    if !clean_secret_key.is_empty() {
        keys_to_set.insert("AWS_SECRET_ACCESS_KEY", clean_secret_key);
    }
    if !clean_region.is_empty() {
        keys_to_set.insert("AWS_REGION", clean_region.clone());
        keys_to_set.insert("AWS_DEFAULT_REGION", clean_region);
    }
    if !clean_endpoint.is_empty() {
        keys_to_set.insert("AWS_ENDPOINT", clean_endpoint);
    }
    if !clean_bucket.is_empty() {
        keys_to_set.insert("DUCKPACK_S3_BUCKET", clean_bucket);
    }

    let mut updated_keys = std::collections::HashSet::new();
    for line in &mut lines {
        let key_opt = line.split_once('=').map(|(k, _)| k.trim().to_string());
        if let Some(trimmed_k) = key_opt {
            if let Some(new_val) = keys_to_set.get(trimmed_k.as_str()) {
                *line = format!("{}={}", trimmed_k, new_val);
                updated_keys.insert(trimmed_k);
            }
        }
    }

    for (k, v) in keys_to_set {
        if !updated_keys.contains(k) {
            lines.push(format!("{}={}", k, v));
        }
    }

    let output = lines.join("\n") + "\n";
    write_restricted_env_file(&env_path, &output)?;
    Ok(())
}

/// Safely appends or updates a cloud bucket URI into the project's `.env` file under DUCKPACK_S3_BUCKETS.
pub fn append_cloud_bucket_to_env(project_dir: &std::path::Path, bucket: &str) -> Result<()> {
    let env_path = project_dir.join(".env");
    let mut lines: Vec<String> = if env_path.exists() {
        std::fs::read_to_string(&env_path)
            .unwrap_or_default()
            .lines()
            .map(|s| s.to_string())
            .collect()
    } else {
        Vec::new()
    };

    let clean_bucket = sanitize_env_value(bucket.trim());
    let mut found = false;

    for line in &mut lines {
        let key_opt = line
            .split_once('=')
            .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()));
        if let Some((k, v)) = key_opt {
            if k == "DUCKPACK_S3_BUCKET" || k == "DUCKPACK_S3_BUCKETS" {
                let mut buckets: Vec<String> = v
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
                if !buckets.contains(&clean_bucket.to_string()) {
                    buckets.push(clean_bucket.to_string());
                }
                *line = format!("DUCKPACK_S3_BUCKETS={}", buckets.join(","));
                found = true;
            }
        }
    }

    if !found {
        lines.push(format!("DUCKPACK_S3_BUCKET={}", clean_bucket));
    }

    let output = lines.join("\n") + "\n";
    write_restricted_env_file(&env_path, &output)?;
    Ok(())
}

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

/// Scaffolds a new declarative database project by generating the standard directory structure,
/// configuration files (`duckpack.toml`), environment templates, and documentation.
pub fn init_project(project_dir: &Path) -> Result<()> {
    let dirs = [
        "schemas",
        "types",
        "sequences",
        "tables",
        "views",
        "macros",
        "triggers",
        "queries",
        "seeds",
        "scripts/pre-deploy",
        "scripts/post-deploy",
    ];
    for dir in dirs.iter() {
        let path = project_dir.join(dir);
        if !path.exists() {
            fs::create_dir_all(&path)
                .with_context(|| format!("Failed to create directory {}", path.display()))?;
            println!("Created directory: {}", path.display());
        }
    }

    let config_path = project_dir.join("duckpack.toml");
    if !config_path.exists() {
        let default_config = r#"[project]
name = "my_duckdb_project"
version = "0.1.0"
description = "A declarative DuckDB database project"

[target]
default_db = "local.duckdb"

# Remote Quack example:
# host = "analytics.company.internal"
# port = 9494
# database = "analytics"
# token_env = "QUACK_TOKEN"
# insecure = false

# Multi-environment targets:
# [env.staging]
# db = "quack:staging.company.internal:9494"
# database = "analytics_stg"
# token_env = "STAGING_QUACK_TOKEN"

# [env.prod]
# db = "quack:prod.company.internal:9494"
# database = "analytics"
# token_env = "PROD_QUACK_TOKEN"
"#;
        fs::write(&config_path, default_config).with_context(|| "Failed to write duckpack.toml")?;
        println!("Created config file: {}", config_path.display());
    }

    let readme_path = project_dir.join("README.md");
    if !readme_path.exists() {
        let readme_content = r#"# DuckDB Database Project

Welcome to your declarative DuckDB project! This directory contains the desired state of your database schema.

## Project Structure

- `schemas/`: Contains `CREATE SCHEMA` statements. Schemas deleted from this folder will be dropped **CASCADE** on the target DB!
- `types/`: Contains `CREATE TYPE` statements (enums, custom data types).
- `sequences/`: Contains `CREATE SEQUENCE` statements (compiled before tables to support `DEFAULT nextval('...')`).
- `tables/`: Contains `CREATE TABLE` statements (one per file). Subdirectories supported for nested schemas.
- `views/`: Contains `CREATE VIEW` statements (engine auto-forces `OR REPLACE`).
- `macros/`: Contains `CREATE MACRO` statements.
- `triggers/`: Contains `CREATE TRIGGER` statements (table-scoped triggers).
- `queries/`: Save your ad-hoc `.sql` queries and scratchpad files here.
- `seeds/`: Contains static CSVs or seed scripts.
- `scripts/`: Contains `pre-deploy` and `post-deploy` scripts.
- `duckpack.toml`: Project configuration and targets.
- `.env`: Environment variables for template interpolation. Write `${VAR_NAME}` in your SQL, and the engine will dynamically replace it!
- `.duckpackignore`: Specifies tables, views, sequences, triggers, or file paths from any directory (including `scripts/pre-deploy` and `scripts/post-deploy`) to ignore during compilation and deployment (wildcards supported).
"#;
        fs::write(&readme_path, readme_content).with_context(|| "Failed to write README.md")?;
        println!("Created documentation file: {}", readme_path.display());
    }

    let env_path = project_dir.join(".env");
    if !env_path.exists() {
        let env_content = r#"# Environment variables for DuckPack SQL template interpolation and Quack credentials.
# Example SQL template variable: 
# ENV=dev

# Remote Quack server authentication token (avoids exposing credentials in shell history):
# QUACK_TOKEN=your_quack_token_here
"#;
        write_restricted_env_file(&env_path, env_content).with_context(|| "Failed to write .env")?;
        println!("Created environment file: {}", env_path.display());
    }

    let ignore_path = project_dir.join(".duckpackignore");
    if !ignore_path.exists() {
        let ignore_content = r#"# Specify tables, views, sequences, triggers, or file paths from any directory to ignore.
# Wildcards (* and ?) and relative directory paths are supported.
# Examples:
# temp_*
# *_backup
# scratch_*
# scripts/pre-deploy/01_one_time_seed.sql
# scripts/post-deploy/*
# archive/
"#;
        fs::write(&ignore_path, ignore_content)
            .with_context(|| "Failed to write .duckpackignore")?;
        println!("Created ignore file: {}", ignore_path.display());
    }

    let gitignore_path = project_dir.join(".gitignore");
    if !gitignore_path.exists() {
        let gitignore_content = ".env*\n*.duckdb\n*.history.sql\nREADME.md\n*.csv\n*.duckpack\nduckpack.log\nmigration.sql\n";
        fs::write(&gitignore_path, gitignore_content)
            .with_context(|| "Failed to write .gitignore")?;
        println!("Created .gitignore file: {}", gitignore_path.display());
    }

    Ok(())
}

/// Compiles all declarative SQL directories (`schemas`, `types`, `sequences`, `macros`, `tables`, `views`, `triggers`)
/// against any `DbExecutor` in dependency-safe topological order with retry, filtering out files matched by `.duckpackignore`.
pub fn compile_project_declarations(exec: &mut dyn DbExecutor, project_dir: &Path) -> Result<()> {
    let ignore_rules = load_ignore_rules(project_dir);
    for dir_name in ["schemas", "types", "sequences", "macros", "tables", "views", "triggers"] {
        let dir = project_dir.join(dir_name);
        if dir.exists() {
            execute_directory_filtered(&dir, project_dir, &ignore_rules, |sql| exec.execute_batch(sql))?;
        }
    }
    Ok(())
}

/// Compiles all local `.sql` files into an in-memory DuckDB connection.
/// This acts as the "Desired State" or "Shadow DB" used for validation and diff generation.
pub fn build_shadow_db(project_dir: &Path) -> Result<Connection> {
    if project_dir.is_file() && project_dir.extension().unwrap_or_default() == "duckpack" {
        return Connection::open(project_dir).with_context(|| "Failed to open .duckpack artifact");
    }

    let mut conn = open_duckdb_connection().with_context(|| "Failed to create Shadow DB")?;
    compile_project_declarations(&mut conn, project_dir)?;

    Ok(conn)
}

/// Recursively collects `.sql` files, filtering out any files or subdirectories matched by `ignore_rules`.
pub fn collect_sql_files_filtered(
    dir: &Path,
    project_dir: &Path,
    ignore_rules: &[String],
    out: &mut Vec<std::path::PathBuf>,
) -> Result<()> {
    if !dir.exists() {
        return Ok(());
    }
    if is_file_ignored(dir, project_dir, ignore_rules) {
        return Ok(());
    }
    let mut entries: Vec<_> = fs::read_dir(dir)?.filter_map(|e| e.ok()).collect();
    entries.sort_by_key(|e| e.path());
    for entry in entries {
        let path = entry.path();
        if path.is_dir() {
            if !is_file_ignored(&path, project_dir, ignore_rules) {
                collect_sql_files_filtered(&path, project_dir, ignore_rules, out)?;
            }
        } else if path.is_file() && path.extension().unwrap_or_default() == "sql" {
            if !is_file_ignored(&path, project_dir, ignore_rules) {
                out.push(path);
            }
        }
    }
    Ok(())
}

/// Helper to collect all SQL files in `dir` respecting project `.duckpackignore`.
pub fn collect_sql_files_recursive(dir: &Path, out: &mut Vec<std::path::PathBuf>) -> Result<()> {
    let project_dir = discover_project_dir(dir);
    let ignore_rules = load_ignore_rules(&project_dir);
    collect_sql_files_filtered(dir, &project_dir, &ignore_rules, out)
}

/// A helper utility to iterate over a directory of `.sql` files and execute them using a custom executor function.
/// Respects ignore rules from `.duckpackignore`, `.duckdbignore`, or `.duckignore`.
/// Recursively scans subdirectories to support nested schemas and modular structures.
pub fn execute_directory_custom<F>(dir: &Path, exec_fn: F) -> Result<()>
where
    F: FnMut(&str) -> Result<()>,
{
    let project_dir = discover_project_dir(dir);
    let ignore_rules = load_ignore_rules(&project_dir);
    execute_directory_filtered(dir, &project_dir, &ignore_rules, exec_fn)
}

/// A helper utility to iterate over a directory of `.sql` files and execute them using a custom executor function,
/// filtering out any files or subdirectories matched by `ignore_rules`.
/// Recursively scans subdirectories to support nested schemas and modular structures.
pub fn execute_directory_filtered<F>(
    dir: &Path,
    project_dir: &Path,
    ignore_rules: &[String],
    exec_fn: F,
) -> Result<()>
where
    F: FnMut(&str) -> Result<()>,
{
    let mut files = Vec::new();
    collect_sql_files_filtered(dir, project_dir, ignore_rules, &mut files)?;
    let mut pending_sqls = Vec::new();
    for path in files {
        pending_sqls.push(diff::read_sql_with_env(&path)?);
    }

    execute_sqls_with_retry_custom(pending_sqls, exec_fn)
        .context(format!("Failed to compile SQL files in {}", dir.display()))
}

/// A helper utility to iterate over a directory of `.sql` files and execute them against a connection.
pub fn execute_directory(conn: &Connection, dir: &Path) -> Result<()> {
    execute_directory_custom(dir, |sql| {
        conn.execute_batch(sql).map_err(|e| anyhow::anyhow!(e))
    })
}

/// A specialized execution engine for arbitrary SQLs with custom executor function.
pub fn execute_sqls_with_retry_custom<F>(
    mut pending_sqls: Vec<String>,
    mut exec_fn: F,
) -> Result<()>
where
    F: FnMut(&str) -> Result<()>,
{
    let mut last_error: Option<anyhow::Error> = None;
    let mut progress_made = true;

    while !pending_sqls.is_empty() && progress_made {
        progress_made = false;
        let mut remaining = Vec::new();

        for sql in pending_sqls {
            match exec_fn(&sql) {
                Ok(_) => {
                    progress_made = true;
                }
                Err(e) => {
                    last_error = Some(e);
                    remaining.push(sql);
                }
            }
        }
        pending_sqls = remaining;
    }

    if !pending_sqls.is_empty() {
        if let Some(e) = last_error {
            return Err(e).context(
                "Failed to compile SQLs due to unresolvable dependencies or syntax errors",
            );
        }
    }

    Ok(())
}

/// A specialized execution engine for arbitrary SQLs against a connection.
pub fn execute_sqls_with_retry(conn: &Connection, pending_sqls: Vec<String>) -> Result<()> {
    execute_sqls_with_retry_custom(pending_sqls, |sql| {
        conn.execute_batch(sql).map_err(|e| anyhow::anyhow!(e))
    })
}

/// Loads ignore patterns from `.duckpackignore`, `.duckdbignore`, or legacy `.duckignore`.
/// Lines beginning with '#' and empty lines are skipped.
pub fn load_ignore_rules(project_dir: &Path) -> Vec<String> {
    let mut rules = Vec::new();
    let candidates = [".duckpackignore", ".duckdbignore", ".duckignore"];
    for name in candidates {
        let path = project_dir.join(name);
        if path.exists() {
            if let Ok(content) = fs::read_to_string(path) {
                for line in content.lines() {
                    let trimmed = line.trim();
                    if !trimmed.is_empty() && !trimmed.starts_with('#') {
                        if !rules.contains(&trimmed.to_string()) {
                            rules.push(trimmed.to_string());
                        }
                    }
                }
            }
        }
    }
    rules
}

/// Matches a string against a simple glob pattern where `*` matches 0 or more characters
/// and `?` matches any single character.
pub fn glob_match(pattern: &str, text: &str) -> bool {
    if pattern == text || pattern == "*" {
        return true;
    }
    let mut regex_str = String::with_capacity(pattern.len() * 2 + 2);
    regex_str.push('^');
    let mut chars = pattern.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '*' => {
                if chars.peek() == Some(&'*') {
                    chars.next();
                }
                regex_str.push_str(".*");
            }
            '?' => regex_str.push('.'),
            '.' | '(' | ')' | '+' | '|' | '^' | '$' | '@' | '%' | '{' | '}' | '[' | ']' | '\\' => {
                regex_str.push('\\');
                regex_str.push(c);
            }
            other => regex_str.push(other),
        }
    }
    regex_str.push('$');
    regex::Regex::new(&regex_str)
        .map(|re| re.is_match(text))
        .unwrap_or(false)
}

/// Checks whether a file or directory path matches any ignore rule from `.duckpackignore`
/// (or `.duckdbignore` / `.duckignore`). Supports relative paths from any directory (e.g. `scripts/pre-deploy/seed.sql`),
/// path suffixes (`pre-deploy/seed.sql`), filenames (`seed.sql`), file stems (`seed`),
/// directory paths (with trailing slash `scripts/pre-deploy/`), and wildcards (`*`, `?`).
pub fn is_file_ignored(file_path: &Path, project_dir: &Path, rules: &[String]) -> bool {
    if rules.is_empty() {
        return false;
    }

    let norm_path_str = file_path.to_string_lossy().replace('\\', "/");
    let norm_proj_str = project_dir.to_string_lossy().replace('\\', "/");

    // Compute relative path string
    let rel_str = if norm_path_str.starts_with(&norm_proj_str) {
        let stripped = &norm_path_str[norm_proj_str.len()..];
        stripped.trim_start_matches('/').to_string()
    } else {
        match file_path.strip_prefix(project_dir) {
            Ok(p) => p.to_string_lossy().replace('\\', "/"),
            Err(_) => norm_path_str.clone(),
        }
    };

    let file_name = file_path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let file_stem = file_path
        .file_stem()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();

    for raw_rule in rules {
        let pattern = raw_rule.trim();
        if pattern.is_empty() || pattern.starts_with('#') {
            continue;
        }
        let pattern = pattern.replace('\\', "/");
        let pattern = pattern.strip_prefix("./").unwrap_or(&pattern);
        let pattern = pattern.strip_prefix('/').unwrap_or(pattern);

        if pattern.is_empty() {
            continue;
        }

        // 1. Directory pattern (ending in /)
        if pattern.ends_with('/') {
            let dir_prefix = pattern.trim_end_matches('/');
            if rel_str == dir_prefix
                || rel_str.starts_with(pattern)
                || rel_str.starts_with(&format!("{}/", dir_prefix))
                || rel_str.ends_with(&format!("/{}", dir_prefix))
                || rel_str.contains(&format!("/{}/", dir_prefix))
            {
                return true;
            }
        }

        // 2. Pattern containing / (path-specific)
        if pattern.contains('/') {
            if rel_str == pattern {
                return true;
            }
            if glob_match(pattern, &rel_str) {
                return true;
            }
            // Suffix match (e.g. pattern "pre-deploy/01.sql" matching "scripts/pre-deploy/01.sql")
            if rel_str.ends_with(&format!("/{}", pattern)) {
                return true;
            }
            // Suffix glob match (e.g. pattern "pre-deploy/*" matching "scripts/pre-deploy/01.sql")
            if glob_match(&format!("*/{}", pattern), &rel_str) {
                return true;
            }
        } else {
            // 3. Pattern without / (matches filename, file stem, or direct glob)
            if file_name == pattern || file_stem == pattern {
                return true;
            }
            if glob_match(pattern, &file_name) {
                return true;
            }
            if glob_match(pattern, &file_stem) {
                return true;
            }
            if glob_match(pattern, &rel_str) {
                return true;
            }
        }
    }
    false
}

/// Checks whether a database catalog object (table, view, macro, sequence, trigger)
/// matches any ignore pattern from `.duckpackignore`. Supports direct names, unqualified names,
/// wildcards (`*`, `?`), and path-like rules (e.g. `tables/users.sql` or `sequences/my_seq.sql`).
pub fn is_object_ignored(obj_name: &str, rules: &[String]) -> bool {
    if rules.is_empty() {
        return false;
    }
    let unqual_name = obj_name.split('.').last().unwrap_or(obj_name);

    for raw_rule in rules {
        let pattern = raw_rule.trim();
        if pattern.is_empty() || pattern.starts_with('#') {
            continue;
        }
        let pattern = pattern.replace('\\', "/");
        let pattern = pattern.strip_prefix("./").unwrap_or(&pattern);
        let pattern = pattern.strip_prefix('/').unwrap_or(pattern);

        if pattern.is_empty() {
            continue;
        }

        // Direct match or glob on full qualified name or unqualified name
        if obj_name == pattern || unqual_name == pattern {
            return true;
        }
        if glob_match(pattern, obj_name) || glob_match(pattern, unqual_name) {
            return true;
        }

        // If pattern contains a path separator (e.g. "tables/users.sql", "tables/users")
        if pattern.contains('/') {
            let p = Path::new(pattern);
            if let Some(stem) = p.file_stem().and_then(|s| s.to_str()) {
                if !stem.is_empty()
                    && (stem == obj_name
                        || stem == unqual_name
                        || glob_match(stem, obj_name)
                        || glob_match(stem, unqual_name))
                {
                    return true;
                }
            }
        }
    }
    false
}

/// Auto-discovers the active DuckPack project directory.
/// If `start` contains `duckpack.toml`, `queries/`, `tables/`, `schemas/`, or `sequences/`, it is returned.
/// Otherwise, checks candidate subdirectories (e.g. `duckpack/`, `duckdb_project/`, `demo_project/`),
/// and then checks parent directories.
pub fn discover_project_dir(start: &Path) -> PathBuf {
    // 1. If start already has duckpack markers, use it
    if is_project_root(start) {
        return start.to_path_buf();
    }

    // 2. Check candidate subdirectories
    let candidates = [
        "duckpack",
        "duckdb_project",
        "duckdb",
        "project",
        "demo_project",
    ];
    for sub in &candidates {
        let sub_path = start.join(sub);
        if sub_path.is_dir() && is_project_root(&sub_path) {
            return sub_path;
        }
    }

    // Check all immediate subdirectories
    if let Ok(entries) = fs::read_dir(start) {
        for entry in entries.flatten() {
            if let Ok(ft) = entry.file_type() {
                if ft.is_dir() {
                    let p = entry.path();
                    let name = p.file_name().and_then(|n| n.to_str()).unwrap_or_default();
                    if !name.starts_with('.')
                        && !name.starts_with("target")
                        && !name.starts_with("node_modules")
                        && !name.starts_with("venv")
                        && is_project_root(&p)
                    {
                        return p;
                    }
                }
            }
        }
    }

    // 3. Check parent directories up to root
    let mut current = start.parent();
    while let Some(parent) = current {
        if is_project_root(parent) {
            return parent.to_path_buf();
        }
        current = parent.parent();
    }

    // Default fallback
    start.to_path_buf()
}

fn is_project_root(path: &Path) -> bool {
    path.join("duckpack.toml").exists()
        || path.join("queries").is_dir()
        || path.join("tables").is_dir()
        || path.join("schemas").is_dir()
        || path.join("sequences").is_dir()
}

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
}
