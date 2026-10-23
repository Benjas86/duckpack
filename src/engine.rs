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
        Connection::execute(self, sql, [])?;
        Ok(())
    }

    fn execute_batch(&mut self, sql: &str) -> Result<()> {
        Connection::execute_batch(self, sql)?;
        Ok(())
    }

    fn query_rows(&mut self, sql: &str) -> Result<(Vec<String>, Vec<Vec<String>>)> {
        let mut stmt = self.prepare(sql)?;
        let column_count = stmt.column_count();
        let column_names: Vec<String> = (0..column_count)
            .map(|i| stmt.column_name(i).unwrap_or(&format!("col_{}", i)).to_string())
            .collect();

        let rows = stmt.query_map([], |row| {
            let mut formatted_row = Vec::new();
            for i in 0..column_count {
                let val: Result<duckdb::types::Value, _> = row.get(i);
                match val {
                    Ok(v) => formatted_row.push(format_duckdb_value(&v)),
                    Err(_) => formatted_row.push("NULL".to_string()),
                }
            }
            Ok(formatted_row)
        })?;

        let mut data = Vec::new();
        for r in rows {
            data.push(r?);
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
            if let Some(dt) = chrono::DateTime::from_timestamp(timestamp_secs, timestamp_nanos as u32) {
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
        Value::Interval { months, days, nanos } => format!("{} months {} days {} nanos", months, days, nanos),
        Value::Decimal(d) => d.to_string(),
        other => format!("{:?}", other),
    }
}

/// Determines if the provided database string refers to a remote motherduck instance.
pub fn is_remote(db: &str) -> bool {
    db.starts_with("md:") || db.starts_with("motherduck:")
}

/// Determines if the provided quack connection URI targets the 0.0.0.0 local unspecified address.
pub fn is_quack_zero_host(db: &str) -> bool {
    let stripped = db.strip_prefix("quack://").or_else(|| db.strip_prefix("quack:")).unwrap_or(db);
    stripped.starts_with("0.0.0.0") || stripped.starts_with("[::]")
}

/// Determines if the provided quack connection URI targets a loopback address (127.0.0.1, localhost, [::1]).
pub fn is_quack_loopback(db: &str) -> bool {
    let stripped = db.strip_prefix("quack://").or_else(|| db.strip_prefix("quack:")).unwrap_or(db);
    stripped.starts_with("127.0.0.1") || stripped.starts_with("localhost") || stripped.starts_with("[::1]")
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

/// Connects to the target DuckDB database (local, motherduck remote, or quack remote).
pub fn connect_target_db(
    db: &str,
    quack_token: Option<&str>,
    quack_disable_ssl: bool,
) -> Result<Connection> {
    if db.starts_with("quack:") {
        let conn = Connection::open_in_memory()
            .with_context(|| "Failed to open in-memory DB for remote ATTACH")?;
        conn.execute_batch("LOAD quack;").or_else(|_| conn.execute_batch("INSTALL quack; LOAD quack;"))
            .with_context(|| format!("Failed to install/load quack extension for target '{}'", db))?;

        let token = resolve_quack_token(quack_token, None);

        if let Some(token) = token {
            let safe_token = token.replace("'", "''");
            conn.execute(&format!("CREATE SECRET (TYPE QUACK, TOKEN '{}');", safe_token), [])
                .with_context(|| "Failed to set quack auth secret")?;
        }

        let disable_ssl = should_disable_quack_ssl(db, quack_disable_ssl);
        let attach_sql = build_quack_attach_sql(db, disable_ssl);

        let attach_res = conn.execute(&attach_sql, []);
        if let Err(e) = attach_res {
            let err_msg = e.to_string();
            if err_msg.contains("Could not find a Quack authentication token") || err_msg.contains("Authentication failed") {
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
            return Err(anyhow::anyhow!("Failed to attach remote database '{}': {}", db, err_msg));
        }
        if conn.execute("CONNECT target_db;", []).is_err() {
            conn.execute("USE target_db;", [])
                .with_context(|| "Failed to switch to target database")?;
        }
        Ok(conn)
    } else if is_remote(db) {
        let conn = Connection::open_in_memory()
            .with_context(|| "Failed to open in-memory DB for remote ATTACH")?;
        let safe_db = db.replace("'", "''");
        let attach_res = conn.execute(&format!("ATTACH '{}' AS target_db;", safe_db), []);
        if let Err(e) = attach_res {
            return Err(anyhow::anyhow!("Failed to attach remote database '{}': {}", db, e));
        }
        if conn.execute("CONNECT target_db;", []).is_err() {
            conn.execute("USE target_db;", [])
                .with_context(|| "Failed to switch to target database")?;
        }
        Ok(conn)
    } else {
        let conn = Connection::open_in_memory()
            .with_context(|| "Failed to open in-memory DB for local ATTACH")?;
        let safe_db = db.replace("'", "''");
        let attach_res = conn.execute(&format!("ATTACH '{}' AS target_db;", safe_db), []);
        if let Err(e) = attach_res {
            let err_msg = e.to_string();
            if let Some(diagnostic) = diagnose_storage_version_error(&err_msg, db) {
                return Err(anyhow::anyhow!("{}\n\nOriginal error: {}", diagnostic, err_msg));
            }
            return Err(anyhow::anyhow!("Failed to attach local database '{}': {}", db, err_msg));
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
                let _ = conn.execute(&format!("DROP TABLE IF EXISTS target_db.\"{}\";", safe_bt), []);
                let _ = conn.execute(&format!("CALL target_db.query('DROP TABLE IF EXISTS \"{}\";');", safe_bt.replace('\'', "''")), []);
                let _ = conn.execute(&format!("CALL target_db.query('DROP TABLE IF EXISTS memory.\"{}\";');", safe_bt.replace('\'', "''")), []);
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
        let _ = conn.execute("CALL target_db.query('DROP TABLE IF EXISTS memory.__duckpack_temp_views_registry;');", []);
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
            let _ = conn.execute(&format!("DROP TABLE IF EXISTS target_db.\"{}\";", safe_bt), []);
            let _ = conn.execute(&format!("CALL target_db.query('DROP TABLE IF EXISTS \"{}\";');", safe_bt.replace('\'', "''")), []);
            let _ = conn.execute(&format!("CALL target_db.query('DROP TABLE IF EXISTS memory.\"{}\";');", safe_bt.replace('\'', "''")), []);
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
        let stage_sql = format!("CREATE OR REPLACE TABLE target_db.\"__duckpack_temp_bridge_{}\" AS SELECT * FROM \"{}\";", safe_t, safe_t);
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
            let ins_sql = format!("CALL target_db.query('INSERT INTO memory.__duckpack_temp_views_sync_back VALUES (''{}'', ''{}'');');", safe_vname, safe_vsql);
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
    let is_quack = conn
        .query_row(
            "SELECT count(*) FROM duckdb_databases() WHERE database_name = 'target_db' AND (lower(type) = 'quack' OR path LIKE 'quack:%')",
            [],
            |r| r.get::<_, i64>(0),
        )
        .unwrap_or(0) > 0;

    let orig_db: String = if is_quack {
        conn.query_row("SELECT * FROM target_db.query('SELECT current_database()')", [], |r| r.get(0))
            .unwrap_or_else(|_| "memory".to_string())
    } else {
        conn.query_row("SELECT current_database()", [], |r| r.get(0)).unwrap_or_default()
    };

    let target_cat = catalog.unwrap_or(&orig_db);

    if !is_quack && !target_cat.is_empty() && target_cat != orig_db {
        conn.execute_batch(&format!("USE \"{}\";", target_cat.replace('\"', "\"\"")))?;
    }

    if is_remote {
        if is_quack {
            let _ = conn.execute_batch("CALL target_db.query('BEGIN TRANSACTION;');");
        } else {
            conn.execute_batch("BEGIN TRANSACTION;")?;
        }
    }

    let execute_sql = |sql: &str| -> Result<()> {
        if is_quack {
            let safe_cat = target_cat.replace('\'', "''");
            let safe_sql = sql.replace('\'', "''");
            conn.execute_batch(&format!("CALL target_db.query('USE \"{}\"; {}')", safe_cat, safe_sql))
                .with_context(|| format!("Failed executing DDL on catalog '{}' via Quack", target_cat))?;
        } else {
            conn.execute_batch(sql)?;
        }
        Ok(())
    };

    let result = (|| -> Result<()> {
        let pre_deploy_dir = project_dir.join("scripts").join("pre-deploy");
        if pre_deploy_dir.exists() {
            execute_directory_custom(&pre_deploy_dir, &execute_sql)?;
        }

        let mut views_to_apply = Vec::new();
        let mut triggers_to_apply = Vec::new();
        let mut drops_to_apply = Vec::new();
        let mut drop_schemas = Vec::new();

        // 1. Create schemas first
        for item in &diff.items {
            if !item.selected || item.ignored { continue; }
            if item.item_type == diff::DiffItemType::CreateSchema {
                execute_sql(&item.sql)?;
            }
        }

        let mut structure_to_apply = Vec::new();
        for item in &diff.items {
            if !item.selected || item.ignored { continue; }
            match item.item_type {
                diff::DiffItemType::CreateTable | diff::DiffItemType::AlterTable | diff::DiffItemType::CreateMacro | diff::DiffItemType::RenameTable | diff::DiffItemType::RenameColumn => {
                    structure_to_apply.push(item.sql.clone());
                }
                diff::DiffItemType::DropTable | diff::DiffItemType::DropColumn | diff::DiffItemType::DropView | diff::DiffItemType::DropMacro | diff::DiffItemType::DropTrigger => {
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
                diff::DiffItemType::CreateSchema => {} // handled above
            }
        }

        if !structure_to_apply.is_empty() {
            execute_sqls_with_retry_custom(structure_to_apply, &execute_sql)?;
        }

        // 3. Views
        if !views_to_apply.is_empty() {
            execute_sqls_with_retry_custom(views_to_apply, &execute_sql)?;
        }

        // 4. Triggers
        if !triggers_to_apply.is_empty() {
            execute_sqls_with_retry_custom(triggers_to_apply, &execute_sql)?;
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
            execute_directory_custom(&post_deploy_dir, &execute_sql)?;
        }

        Ok(())
    })();

    if is_remote {
        if result.is_ok() {
            if is_quack {
                let _ = conn.execute_batch("CALL target_db.query('COMMIT;');");
            } else {
                conn.execute_batch("COMMIT;")?;
            }
        } else {
            if is_quack {
                let _ = conn.execute_batch("CALL target_db.query('ROLLBACK;');");
            } else {
                conn.execute_batch("ROLLBACK;")?;
            }
        }
    }

    if !is_quack && !orig_db.is_empty() && target_cat != orig_db {
        let _ = conn.execute_batch(&format!("USE \"{}\";", orig_db.replace('\"', "\"\"")));
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

    if let Some(db_val) = tbl.get("db").or_else(|| tbl.get("default_db")).and_then(|v| v.as_str()) {
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

    if let Some(val) = tbl.get("database").or_else(|| tbl.get("catalog")) {
        if let Some(s) = val.as_str() {
            cfg.catalogs = vec![s.to_string()];
        } else if let Some(arr) = val.as_array() {
            cfg.catalogs = arr.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect();
        }
    }

    if let Some(t_env) = tbl.get("token_env").and_then(|v| v.as_str()) {
        cfg.token_env = Some(t_env.to_string());
    }

    if let Some(ins) = tbl.get("insecure").or_else(|| tbl.get("quack_disable_ssl")).and_then(|v| v.as_bool()) {
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

/// Resolves target configuration from duckpack.toml, supporting [target] defaults and [env.<profile>] overrides.
pub fn resolve_target_config(project_dir: &std::path::Path, env_profile: Option<&str>) -> ResolvedTargetConfig {
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

    let target_defaults = value.get("target").map(parse_target_table).unwrap_or_default();

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
pub fn resolve_project_catalogs(project_dir: &std::path::Path, env_profile: Option<&str>) -> Vec<String> {
    resolve_target_config(project_dir, env_profile).catalogs
}

/// Resolves target catalog from duckpack.toml if configured (returns first configured catalog).
pub fn resolve_project_catalog(project_dir: &std::path::Path, env_profile: Option<&str>) -> Option<String> {
    resolve_project_catalogs(project_dir, env_profile).into_iter().next()
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
            Some(d) if env_profile.is_none() && d != "local.duckdb" && !d.is_empty() => d.to_string(),
            Some(d) if env_profile.is_some() => config.db.clone().unwrap_or_else(|| d.to_string()),
            Some(d) if d == "local.duckdb" => config.db.clone().unwrap_or_else(|| "local.duckdb".to_string()),
            Some(d) if !d.is_empty() => d.to_string(),
            _ => config.db.clone().unwrap_or_else(|| "local.duckdb".to_string()),
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
    let dirs = ["schemas", "tables", "views", "macros", "triggers", "queries", "seeds", "scripts/pre-deploy", "scripts/post-deploy"];
    for dir in dirs.iter() {
        let path = project_dir.join(dir);
        if !path.exists() {
            fs::create_dir_all(&path).with_context(|| format!("Failed to create directory {}", path.display()))?;
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
- `tables/`: Contains `CREATE TABLE` statements (one per file). Subdirectories supported for nested schemas.
- `views/`: Contains `CREATE VIEW` statements (engine auto-forces `OR REPLACE`).
- `macros/`: Contains `CREATE MACRO` statements.
- `triggers/`: Contains `CREATE TRIGGER` statements (table-scoped triggers).
- `queries/`: Save your ad-hoc `.sql` queries and scratchpad files here.
- `seeds/`: Contains static CSVs or seed scripts.
- `scripts/`: Contains `pre-deploy` and `post-deploy` scripts.
- `duckpack.toml`: Project configuration and targets.
- `.env`: Environment variables for template interpolation. Write `${VAR_NAME}` in your SQL, and the engine will dynamically replace it!
- `.duckpackignore`: Specifies tables or views to ignore during diffing (wildcards supported).
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
        fs::write(&env_path, env_content).with_context(|| "Failed to write .env")?;
        println!("Created environment file: {}", env_path.display());
    }

    let ignore_path = project_dir.join(".duckpackignore");
    if !ignore_path.exists() {
        let ignore_content = r#"# Specify tables, views, or triggers to ignore during diffing (wildcards supported).
# Example:
# temp_*
# *_backup
# scratch_*
"#;
        fs::write(&ignore_path, ignore_content).with_context(|| "Failed to write .duckpackignore")?;
        println!("Created ignore file: {}", ignore_path.display());
    }

    let gitignore_path = project_dir.join(".gitignore");
    if !gitignore_path.exists() {
        let gitignore_content = ".env*\n*.duckdb\n*.history.sql\nREADME.md\n*.csv\n*.duckpack\nduckpack.log\nmigration.sql\n";
        fs::write(&gitignore_path, gitignore_content).with_context(|| "Failed to write .gitignore")?;
        println!("Created .gitignore file: {}", gitignore_path.display());
    }

    Ok(())
}

/// Compiles all local `.sql` files into an in-memory DuckDB connection.
/// This acts as the "Desired State" or "Shadow DB" used for validation and diff generation.
pub fn build_shadow_db(project_dir: &Path) -> Result<Connection> {
    if project_dir.is_file() && project_dir.extension().unwrap_or_default() == "duckpack" {
        return Connection::open(project_dir).with_context(|| "Failed to open .duckpack artifact");
    }

    let conn = Connection::open_in_memory().with_context(|| "Failed to create Shadow DB")?;
    
    let schemas_dir = project_dir.join("schemas");
    if schemas_dir.exists() {
        execute_directory(&conn, &schemas_dir)?;
    }

    let macros_dir = project_dir.join("macros");
    if macros_dir.exists() {
        execute_directory(&conn, &macros_dir)?;
    }

    let tables_dir = project_dir.join("tables");
    if tables_dir.exists() {
        execute_directory(&conn, &tables_dir)?;
    }

    let views_dir = project_dir.join("views");
    if views_dir.exists() {
        execute_directory(&conn, &views_dir)?;
    }

    let triggers_dir = project_dir.join("triggers");
    if triggers_dir.exists() {
        execute_directory(&conn, &triggers_dir)?;
    }

    Ok(conn)
}

fn collect_sql_files_recursive(dir: &Path, out: &mut Vec<std::path::PathBuf>) -> Result<()> {
    if !dir.exists() {
        return Ok(());
    }
    let mut entries: Vec<_> = fs::read_dir(dir)?.filter_map(|e| e.ok()).collect();
    entries.sort_by_key(|e| e.path());
    for entry in entries {
        let path = entry.path();
        if path.is_dir() {
            collect_sql_files_recursive(&path, out)?;
        } else if path.is_file() && path.extension().unwrap_or_default() == "sql" {
            out.push(path);
        }
    }
    Ok(())
}

/// A helper utility to iterate over a directory of `.sql` files and execute them using a custom executor function.
/// Recursively scans subdirectories to support nested schemas and modular structures.
pub fn execute_directory_custom<F>(dir: &Path, exec_fn: F) -> Result<()>
where
    F: FnMut(&str) -> Result<()>,
{
    let mut files = Vec::new();
    collect_sql_files_recursive(dir, &mut files)?;
    let mut pending_sqls = Vec::new();
    for path in files {
        pending_sqls.push(diff::read_sql_with_env(&path)?);
    }

    execute_sqls_with_retry_custom(pending_sqls, exec_fn)
        .context(format!("Failed to compile SQL files in {}", dir.display()))
}

/// A helper utility to iterate over a directory of `.sql` files and execute them against a connection.
pub fn execute_directory(conn: &Connection, dir: &Path) -> Result<()> {
    execute_directory_custom(dir, |sql| conn.execute_batch(sql).map_err(|e| anyhow::anyhow!(e)))
}

/// A specialized execution engine for arbitrary SQLs with custom executor function.
pub fn execute_sqls_with_retry_custom<F>(mut pending_sqls: Vec<String>, mut exec_fn: F) -> Result<()>
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
                    last_error = Some(anyhow::anyhow!("Failed: {}", e));
                    remaining.push(sql);
                }
            }
        }
        pending_sqls = remaining;
    }

    if !pending_sqls.is_empty() {
        if let Some(e) = last_error {
            return Err(e).context("Failed to compile SQLs due to unresolvable dependencies or syntax errors");
        }
    }

    Ok(())
}

/// A specialized execution engine for arbitrary SQLs against a connection.
pub fn execute_sqls_with_retry(conn: &Connection, pending_sqls: Vec<String>) -> Result<()> {
    execute_sqls_with_retry_custom(pending_sqls, |sql| conn.execute_batch(sql).map_err(|e| anyhow::anyhow!(e)))
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

/// Auto-discovers the active DuckPack project directory.
/// If `start` contains `duckpack.toml`, `queries/`, `tables/`, or `schemas/`, it is returned.
/// Otherwise, checks candidate subdirectories (e.g. `duckpack/`, `duckdb_project/`, `demo_project/`),
/// and then checks parent directories.
pub fn discover_project_dir(start: &Path) -> PathBuf {
    // 1. If start already has duckpack markers, use it
    if is_project_root(start) {
        return start.to_path_buf();
    }

    // 2. Check candidate subdirectories
    let candidates = ["duckpack", "duckdb_project", "duckdb", "project", "demo_project"];
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_embedded_duckdb_version() {
        let v = embedded_duckdb_version();
        assert!(!v.is_empty());
        assert!(v.starts_with('v') || v.chars().next().map_or(false, |c| c.is_ascii_digit()));
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
        let temp_dir = std::env::temp_dir().join(format!("dp_rec_{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        let nested = temp_dir.join("finance").join("reports");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(nested.join("kpis.sql"), "CREATE TABLE kpis (id INT);").unwrap();

        let conn = Connection::open_in_memory().unwrap();
        execute_directory(&conn, &temp_dir).unwrap();

        let count: i64 = conn.query_row("SELECT count(*) FROM duckdb_tables() WHERE table_name = 'kpis'", [], |r| r.get(0)).unwrap();
        assert_eq!(count, 1);

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_discover_project_dir() {
        let temp_dir = std::env::temp_dir().join(format!("duckpack_test_disc_{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
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
        let temp_dir = std::env::temp_dir().join(format!("duckpack_test_cfg_{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
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
        assert_eq!(cfg.catalogs, vec!["analytics".to_string(), "orders".to_string()]);
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
        let temp_dir = std::env::temp_dir().join(format!("duckpack_test_env_{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir_all(&temp_dir).unwrap();

        // Base .env has common token and host
        std::fs::write(temp_dir.join(".env"), "DUCKPACK_TEST_BASE_TOKEN=base_secret_123\nDUCKPACK_TEST_COMMON_VAR=base_val\n").unwrap();
        // Profile .env.dev overrides COMMON_VAR but leaves base token
        std::fs::write(temp_dir.join(".env.dev"), "DUCKPACK_TEST_COMMON_VAR=dev_val\n").unwrap();

        load_env_files(&temp_dir, Some("dev"));

        assert_eq!(std::env::var("DUCKPACK_TEST_BASE_TOKEN").as_deref(), Ok("base_secret_123"));
        assert_eq!(std::env::var("DUCKPACK_TEST_COMMON_VAR").as_deref(), Ok("dev_val"));

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_target_resolver_hierarchy_and_spec() {
        let temp_dir = std::env::temp_dir().join(format!("duckpack_test_resolver_{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
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
        assert_eq!(spec_prod.catalogs, vec!["prod_analytics".to_string(), "prod_events".to_string()]);

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
        let temp_dir = std::env::temp_dir().join(format!("dp_ignore_{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        let _ = std::fs::create_dir_all(&temp_dir);

        // When no file exists, returns empty
        assert!(load_ignore_rules(&temp_dir).is_empty());

        // Create .duckpackignore with comments, blank lines, and patterns
        let content = "\n# Comments are ignored\nscratch_*\ntemp_*\n\nraw_data\n";
        std::fs::write(temp_dir.join(".duckpackignore"), content).unwrap();

        let rules = load_ignore_rules(&temp_dir);
        assert_eq!(rules, vec!["scratch_*".to_string(), "temp_*".to_string(), "raw_data".to_string()]);

        let _ = std::fs::remove_dir_all(&temp_dir);
    }
}

