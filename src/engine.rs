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

/// Determines if SSL should be disabled for a Quack connection.
/// Returns true if explicitly flagged, if DUCKDB_QUACK_DISABLE_SSL is set, or if connecting to 0.0.0.0.
pub fn should_disable_quack_ssl(db: &str, flag_disable_ssl: bool) -> bool {
    flag_disable_ssl
        || std::env::var("DUCKDB_QUACK_DISABLE_SSL")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true") || v.eq_ignore_ascii_case("yes"))
            .unwrap_or(false)
        || is_quack_zero_host(db)
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
        || (lower.contains("cannot open database") && lower.contains("version"))
    {
        Some(format!(
            "DuckDB Storage Version Mismatch: The database file '{}' appears to have been created or modified by a newer or different version of DuckDB than DuckPack's embedded engine ({}).\n\n\
            Remediation Options:\n\
              1. Upgrade DuckPack: Run `duckpack python install --upgrade` or download the latest CLI release.\n\
              2. Python One-Liner (Auto-Bridged): Run using your active Python environment's DuckDB:\n\
                 $ python -c \"import duckdb, duckpack; con = duckdb.connect('{}'); duckpack.explore(con)\"\n\
              3. Serve via your local DuckDB CLI:\n\
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
        conn.execute_batch("INSTALL quack; LOAD quack;")
            .with_context(|| "Failed to install/load quack extension")?;

        let token = quack_token
            .map(|s| s.to_string())
            .or_else(|| std::env::var("DUCKDB_QUACK_TOKEN").ok());

        if let Some(token) = token {
            let safe_token = token.replace("'", "''");
            conn.execute(&format!("CREATE SECRET (TYPE QUACK, TOKEN '{}');", safe_token), [])
                .with_context(|| "Failed to set quack auth secret")?;
        }

        let disable_ssl = should_disable_quack_ssl(db, quack_disable_ssl);
        let attach_sql = build_quack_attach_sql(db, disable_ssl);

        conn.execute(&attach_sql, [])
            .with_context(|| "Failed to attach remote database")?;
        conn.execute("USE target_db;", [])
            .with_context(|| "Failed to switch to target database")?;
        Ok(conn)
    } else if is_remote(db) {
        let conn = Connection::open_in_memory()
            .with_context(|| "Failed to open in-memory DB for remote ATTACH")?;
        let safe_db = db.replace("'", "''");
        conn.execute(&format!("ATTACH '{}' AS target_db;", safe_db), [])
            .with_context(|| "Failed to attach remote database")?;
        conn.execute("USE target_db;", [])
            .with_context(|| "Failed to switch to target database")?;
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
        conn.execute("USE target_db;", [])
            .with_context(|| "Failed to switch to target database")?;
        Ok(conn)
    }
}

/// Executes the computed schema differences against the target database.
/// If running against a remote database, it encapsulates the entire deployment inside a single transaction
/// so it can rollback gracefully upon failure.
pub fn apply_diff(conn: &Connection, diff: &diff::DiffResult, force_drop: bool, is_remote: bool, project_dir: &PathBuf) -> Result<()> {
    if is_remote {
        conn.execute_batch("BEGIN TRANSACTION;")?;
    }

    let result = (|| -> Result<()> {
        let pre_deploy_dir = project_dir.join("scripts").join("pre-deploy");
        if pre_deploy_dir.exists() {
            execute_directory(conn, &pre_deploy_dir)?;
        }

        let mut views_to_apply = Vec::new();
        let mut drops_to_apply = Vec::new();
        let mut drop_schemas = Vec::new();

        // 1. Create schemas first
        for item in &diff.items {
            if !item.selected || item.ignored { continue; }
            if item.item_type == diff::DiffItemType::CreateSchema {
                conn.execute_batch(&item.sql)?;
            }
        }

        let mut structure_to_apply = Vec::new();
        for item in &diff.items {
            if !item.selected || item.ignored { continue; }
            match item.item_type {
                diff::DiffItemType::CreateTable | diff::DiffItemType::AlterTable | diff::DiffItemType::CreateMacro | diff::DiffItemType::RenameTable | diff::DiffItemType::RenameColumn => {
                    structure_to_apply.push(item.sql.clone());
                }
                diff::DiffItemType::DropTable | diff::DiffItemType::DropColumn | diff::DiffItemType::DropView | diff::DiffItemType::DropMacro => {
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
                diff::DiffItemType::CreateSchema => {} // handled above
            }
        }

        if !structure_to_apply.is_empty() {
            execute_sqls_with_retry(conn, structure_to_apply)?;
        }

        // 3. Views
        if !views_to_apply.is_empty() {
            execute_sqls_with_retry(conn, views_to_apply)?;
        }

        // 4. Drops
        for drop_sql in drops_to_apply {
            let _ = conn.execute_batch(&drop_sql); // Ignore errors if already dropped by cascade
        }
        for drop_sql in drop_schemas {
            let _ = conn.execute_batch(&drop_sql);
        }

        let post_deploy_dir = project_dir.join("scripts").join("post-deploy");
        if post_deploy_dir.exists() {
            execute_directory(conn, &post_deploy_dir)?;
        }

        Ok(())
    })();

    if is_remote {
        if result.is_ok() {
            conn.execute_batch("COMMIT;")?;
        } else {
            conn.execute_batch("ROLLBACK;")?;
        }
    }
    result
}

/// Scaffolds a new declarative database project by generating the standard directory structure,
/// configuration files (`duckpack.toml`), environment templates, and documentation.
pub fn init_project(project_dir: &Path) -> Result<()> {
    let dirs = ["schemas", "tables", "views", "macros", "queries", "seeds", "scripts/pre-deploy", "scripts/post-deploy"];
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
- `tables/`: Contains `CREATE TABLE` statements (one per file).
- `views/`: Contains `CREATE VIEW` statements (engine auto-forces `OR REPLACE`).
- `macros/`: Contains `CREATE MACRO` statements.
- `queries/`: Save your ad-hoc `.sql` queries and scratchpad files here.
- `seeds/`: Contains static CSVs or seed scripts.
- `scripts/`: Contains `pre-deploy` and `post-deploy` scripts.
- `duckpack.toml`: Project configuration and targets.
- `.env`: Environment variables for template interpolation. Write `${VAR_NAME}` in your SQL, and the engine will dynamically replace it!
- `.duckdbignore`: Specifies tables or views to ignore during diffing (wildcards supported).
"#;
        fs::write(&readme_path, readme_content).with_context(|| "Failed to write README.md")?;
        println!("Created documentation file: {}", readme_path.display());
    }

    let env_path = project_dir.join(".env");
    if !env_path.exists() {
        let env_content = r#"# Define environment variables here to interpolate into your SQL files.
# Example: 
# ENV=dev
"#;
        fs::write(&env_path, env_content).with_context(|| "Failed to write .env")?;
        println!("Created environment file: {}", env_path.display());
    }

    let ignore_path = project_dir.join(".duckdbignore");
    if !ignore_path.exists() {
        let ignore_content = r#"# Specify tables or views to ignore during diffing (wildcards supported).
# Example:
# temp_*
# *_backup
"#;
        fs::write(&ignore_path, ignore_content).with_context(|| "Failed to write .duckdbignore")?;
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

    Ok(conn)
}

/// A helper utility to iterate over a directory of `.sql` files and execute them against a connection.
/// This uses a retry loop because tables/views often depend on each other (e.g. foreign keys or nested views).
/// If one fails, it is pushed to the back of the queue. The loop terminates once all succeed, or if no progress is made.
pub fn execute_directory(conn: &Connection, dir: &Path) -> Result<()> {
    let mut pending_sqls = Vec::new();
    let mut entries: Vec<_> = fs::read_dir(dir)?.filter_map(|e| e.ok()).collect();
    entries.sort_by_key(|e| e.path());
    
    for entry in entries {
        let path = entry.path();
        if path.is_file() && path.extension().unwrap_or_default() == "sql" {
            pending_sqls.push(diff::read_sql_with_env(&path)?);
        }
    }
    
    let mut last_error: Option<anyhow::Error> = None;
    let mut progress_made = true;

    while !pending_sqls.is_empty() && progress_made {
        progress_made = false;
        let mut remaining = Vec::new();

        for sql in pending_sqls {
            match conn.execute_batch(&sql) {
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
            return Err(e).context(format!("Failed to compile SQL files in {} due to unresolvable dependencies or syntax errors", dir.display()));
        }
    }

    Ok(())
}

/// A specialized execution engine for arbitrary SQLs.
/// Because objects often depend on each other, this algorithm continuously loops over the list of statements
/// and attempts to execute them. If one fails, it is pushed to the back of the queue.
/// The loop terminates once all succeed, or if it completes a full pass without making any progress (circular dependency).
pub fn execute_sqls_with_retry(conn: &Connection, mut pending_sqls: Vec<String>) -> Result<()> {
    let mut last_error: Option<anyhow::Error> = None;
    let mut progress_made = true;

    while !pending_sqls.is_empty() && progress_made {
        progress_made = false;
        let mut remaining = Vec::new();

        for sql in pending_sqls {
            match conn.execute_batch(&sql) {
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

/// Loads ignore patterns from `.duckdbignore` (or legacy `.duckignore`).
/// Lines beginning with '#' and empty lines are skipped.
pub fn load_ignore_rules(project_dir: &Path) -> Vec<String> {
    let ignore_path = if project_dir.join(".duckdbignore").exists() {
        Some(project_dir.join(".duckdbignore"))
    } else if project_dir.join(".duckignore").exists() {
        Some(project_dir.join(".duckignore"))
    } else {
        None
    };

    let mut rules = Vec::new();
    if let Some(path) = ignore_path {
        if let Ok(content) = fs::read_to_string(path) {
            for line in content.lines() {
                let trimmed = line.trim();
                if !trimmed.is_empty() && !trimmed.starts_with('#') {
                    rules.push(trimmed.to_string());
                }
            }
        }
    }
    rules
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
    }
}
