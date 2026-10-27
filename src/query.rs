use std::path::Path;

/// Unified regex pattern for highlighting standard DuckDB SQL keywords in the IDE editor.
pub use crate::syntax::SQL_KEYWORD_PATTERN;

/// Checks if a SQL query includes a `-- @remote` or `-- [remote]` directive.
pub fn is_remote_directive_query(query: &str) -> bool {
    query.lines().any(|line| {
        let trimmed = line.trim();
        trimmed.eq_ignore_ascii_case("-- @remote")
            || trimmed.eq_ignore_ascii_case("--@remote")
            || trimmed.to_lowercase().starts_with("-- @remote ")
            || trimmed.to_lowercase().starts_with("--@remote ")
            || trimmed.eq_ignore_ascii_case("-- [remote]")
    })
}

/// Strips the `-- @remote` directive and wraps the query for remote execution on target_db.
pub fn wrap_remote_query(query: &str) -> (String, bool) {
    let stripped_lines: Vec<&str> = query
        .lines()
        .filter(|line| {
            let trimmed = line.trim();
            !(trimmed.eq_ignore_ascii_case("-- @remote")
                || trimmed.eq_ignore_ascii_case("--@remote")
                || trimmed.to_lowercase().starts_with("-- @remote ")
                || trimmed.to_lowercase().starts_with("--@remote ")
                || trimmed.eq_ignore_ascii_case("-- [remote]"))
        })
        .collect();
    let stripped = stripped_lines.join("\n");
    let trimmed = stripped.trim();
    let clean_stripped = trimmed.strip_suffix(';').unwrap_or(trimmed);
    let lower = clean_stripped.to_lowercase();
    let is_mutating_cte = lower.starts_with("with")
        && (lower.contains(") insert into")
            || lower.contains(") delete from")
            || lower.contains(") update ")
            || (lower.contains("insert into") && !lower.starts_with("with") == false && !lower.contains("select ")));

    let is_select = (lower.starts_with("select")
        || lower.starts_with("with")
        || lower.starts_with("values")
        || lower.starts_with("from"))
        && !is_mutating_cte;

    let escaped = clean_stripped.replace('\'', "''");
    let wrapped = if is_select {
        format!("SELECT * FROM target_db.query('{}')", escaped)
    } else {
        format!("FROM target_db.query('{}');", escaped)
    };
    (wrapped, is_select)
}

/// Checks if an unadorned statement executed on a remote connection appears to be a secret or session setting.
pub fn is_unadorned_secret_or_setting(query: &str) -> bool {
    let lower = query.trim().to_lowercase();
    let mut clean_lower = lower.as_str();
    loop {
        if clean_lower.starts_with("--") {
            if let Some(idx) = clean_lower.find('\n') {
                clean_lower = clean_lower[idx..].trim_start();
            } else {
                clean_lower = "";
            }
        } else if clean_lower.starts_with("/*") {
            if let Some(idx) = clean_lower.find("*/") {
                clean_lower = clean_lower[idx + 2..].trim_start();
            } else {
                clean_lower = "";
            }
        } else {
            break;
        }
    }
    (clean_lower.starts_with("create secret")
        || clean_lower.starts_with("create persistent secret")
        || clean_lower.starts_with("create temporary secret")
        || clean_lower.starts_with("drop secret")
        || clean_lower.starts_with("set s3_")
        || clean_lower.starts_with("set aws_")
        || clean_lower.starts_with("install ")
        || clean_lower.starts_with("load "))
        && !clean_lower.contains("query(")
        && !clean_lower.contains("in motherduck")
}

/// Generates a diagnostic hint if an error message on a remote database indicates cloud storage, credential, or Quack streaming scan limitations.
pub fn diagnose_remote_credential_error(err_str: &str, is_remote: bool) -> &'static str {
    let lower = err_str.to_lowercase();
    if lower.contains("deprecated lambda arrow") || lower.contains("lambda_syntax") {
        " [Hint: DuckDB 2.0 deprecated '->' lambdas. Use 'lambda x: ...' or SET lambda_syntax='ENABLE_SINGLE_ARROW']"
    } else if !is_remote {
        ""
    } else if lower.contains("multiple streaming scans") || lower.contains("streaming scans + ctas") {
        " [Hint: Quack protocol limitation. With DuckDB 2.0 native CONNECT target_db, queries run server-side natively!]"
    } else if lower.contains("s3")
        || lower.contains("403")
        || lower.contains("access denied")
        || lower.contains("no credentials")
        || lower.contains("credential")
        || lower.contains("missing secret")
        || lower.contains("forbidden")
    {
        " [Hint: Missing S3 secret on remote Quack? Run 'CREATE SECRET ...']"
    } else {
        ""
    }
}


/// Extracts the extension name if an error was caused by DuckDB extension autoloading or download failure.
pub fn extract_autoload_extension_name(err_str: &str) -> Option<String> {
    if !err_str.contains("Extension Autoloading Error")
        && !err_str.contains("Failed to download extension")
        && !err_str.contains("is an existing extension") {
        return None;
    }
    // Pattern 1: required extension 'name
    if let Some(pos) = err_str.find("required extension '") {
        let rem = &err_str[pos + "required extension '".len()..];
        let name: String = rem.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
        if !name.is_empty() {
            return Some(name);
        }
    }
    // Pattern 2: Failed to download extension "name" or 'name'
    if let Some(pos) = err_str.find("Failed to download extension ") {
        let rem = &err_str[pos + "Failed to download extension ".len()..];
        let trimmed = rem.trim_start_matches(|c| c == '\'' || c == '"');
        let name: String = trimmed.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
        if !name.is_empty() {
            return Some(name);
        }
    }
    // Pattern 3: Extension "name" is an existing extension
    if let Some(pos) = err_str.find("Extension \"") {
        let rem = &err_str[pos + "Extension \"".len()..];
        let name: String = rem.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
        if !name.is_empty() {
            return Some(name);
        }
    }
    None
}

/// Recursively discovers all `.sql` query files under a root queries directory.
pub fn collect_queries_recursive(dir: &Path, base: &Path, queries: &mut Vec<String>) {
    if let Ok(entries) = std::fs::read_dir(dir) {
        let mut sorted_entries: Vec<_> = entries.flatten().collect();
        sorted_entries.sort_by_key(|e| e.path());
        for entry in sorted_entries {
            let path = entry.path();
            let file_name = match path.file_name().and_then(|n| n.to_str()) {
                Some(name) => name,
                None => continue,
            };

            // Ignore hidden files and directories (like .history.sql or .git)
            if file_name.starts_with('.') {
                continue;
            }

            if path.is_dir() {
                collect_queries_recursive(&path, base, queries);
            } else if path.is_file() {
                if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                    if ext.eq_ignore_ascii_case("sql") {
                        if let Ok(rel) = path.strip_prefix(base) {
                            queries.push(rel.to_string_lossy().to_string());
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_remote_directive_query() {
        assert!(is_remote_directive_query("-- @remote\nCREATE SECRET my_s3..."));
        assert!(is_remote_directive_query("--@remote\nSELECT 1;"));
        assert!(is_remote_directive_query("-- @remote (s3)\nSELECT 1;"));
        assert!(is_remote_directive_query("/* comment */\n-- @remote\nSET s3_region='us-east-1';"));
        assert!(!is_remote_directive_query("SELECT 1;"));
        assert!(!is_remote_directive_query("-- regular comment\nCREATE SECRET foo;"));
    }

    #[test]
    fn test_wrap_remote_query() {
        let (ddl, is_select) = wrap_remote_query("-- @remote\nCREATE SECRET my_s3 (TYPE S3, KEY_ID 'foo', SECRET 'bar');");
        assert!(!is_select);
        assert_eq!(ddl, "FROM target_db.query('CREATE SECRET my_s3 (TYPE S3, KEY_ID ''foo'', SECRET ''bar'')');");

        let (query, is_select) = wrap_remote_query("-- @remote\nSELECT * FROM 's3://bucket/test.parquet';");
        assert!(is_select);
        assert_eq!(query, "SELECT * FROM target_db.query('SELECT * FROM ''s3://bucket/test.parquet''')");
    }

    #[test]
    fn test_is_unadorned_secret_or_setting() {
        assert!(is_unadorned_secret_or_setting("CREATE SECRET s3 (TYPE S3);"));
        assert!(is_unadorned_secret_or_setting("CREATE PERSISTENT SECRET s3 (TYPE S3);"));
        assert!(is_unadorned_secret_or_setting("DROP SECRET s3;"));
        assert!(is_unadorned_secret_or_setting("SET s3_region = 'us-east-1';"));
        assert!(is_unadorned_secret_or_setting("-- comment\nCREATE SECRET s3;"));
        assert!(!is_unadorned_secret_or_setting("SELECT * FROM target_db.query('CREATE SECRET s3');"));
        assert!(!is_unadorned_secret_or_setting("CREATE TABLE foo AS SELECT 1;"));
        assert!(!is_unadorned_secret_or_setting("SELECT * FROM s3_table;"));
    }

    #[test]
    fn test_diagnose_remote_credential_error() {
        assert!(diagnose_remote_credential_error("IO Error: HTTP 403 (Access Denied)", true).contains("Missing S3 secret"));
        assert!(diagnose_remote_credential_error("No credentials found for S3", true).contains("Missing S3 secret"));
        assert!(diagnose_remote_credential_error("Not implemented Error: multiple streaming scans or streaming scans + CTAS", true).contains("Quack protocol limitation"));
        assert_eq!(diagnose_remote_credential_error("syntax error at or near 'SELECT'", true), "");
        assert_eq!(diagnose_remote_credential_error("IO Error: HTTP 403", false), "");
    }

    #[test]
    fn test_collect_queries_recursive() {
        let temp_dir = std::env::temp_dir().join(format!("dp_queries_test_{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        let queries_dir = temp_dir.join("queries");
        let mssql_sub = queries_dir.join("mssql");
        std::fs::create_dir_all(&mssql_sub).unwrap();

        std::fs::write(queries_dir.join("test_root.sql"), "SELECT 1;").unwrap();
        std::fs::write(queries_dir.join("UPPERCASE.SQL"), "SELECT 2;").unwrap();
        std::fs::write(queries_dir.join(".history.sql"), "SELECT 3;").unwrap();
        std::fs::write(queries_dir.join("notes.txt"), "some notes").unwrap();
        std::fs::write(mssql_sub.join("orders.sql"), "SELECT * FROM mssql.orders;").unwrap();

        let mut queries = Vec::new();
        collect_queries_recursive(&queries_dir, &queries_dir, &mut queries);
        queries.sort();

        assert_eq!(queries.len(), 3);
        assert!(queries.contains(&"UPPERCASE.SQL".to_string()));
        assert!(queries.contains(&"test_root.sql".to_string()));
        assert!(queries.contains(&"mssql/orders.sql".to_string()));
        assert!(!queries.contains(&".history.sql".to_string()));
        assert!(!queries.contains(&"notes.txt".to_string()));

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_sql_keyword_pattern() {
        let re = regex::Regex::new(SQL_KEYWORD_PATTERN).unwrap();
        assert!(re.is_match("TRIGGER"));
        assert!(re.is_match("trigger"));
        assert!(re.is_match("CONNECT"));
        assert!(re.is_match("connect"));
        assert!(re.is_match("DISCONNECT"));
        assert!(re.is_match("VARIANT"));
        assert!(re.is_match("RETURNING"));
        assert!(re.is_match("APPROX"));
        assert!(re.is_match("MATERIALIZED"));
        assert!(re.is_match("SELECT"));
        assert!(!re.is_match("random_identifier"));
    }

    #[test]
    fn test_extract_autoload_extension_name() {
        let err1 = "Prepare Error: Extension Autoloading Error: An error occurred while trying to automatically install the required extension 'parquet: Failed to download extension \"parquet\" at URL \"https://extensions.duckdb.org/d54cbbe3df/linux_amd64/parquet.duckdb_extension.gz\" (HTTP 404)\nExtension \"parquet\" is an existing extension.";
        assert_eq!(extract_autoload_extension_name(err1), Some("parquet".to_string()));

        let err2 = "Failed to download extension \"excel\" at URL ...";
        assert_eq!(extract_autoload_extension_name(err2), Some("excel".to_string()));

        let err3 = "Failed to download extension 'mssql' at URL ...";
        assert_eq!(extract_autoload_extension_name(err3), Some("mssql".to_string()));

        let err4 = "Table with name 'users' does not exist!";
        assert_eq!(extract_autoload_extension_name(err4), None);
    }
}
