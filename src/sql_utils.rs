use anyhow::Result;
use regex::Regex;
use std::sync::OnceLock;

/// Quotes a SQL identifier with double quotes, escaping embedded `"` as `""`.
pub fn quote_ident(ident: &str) -> String {
    format!("\"{}\"", ident.replace('"', "\"\""))
}

/// Escapes single quotes inside a SQL string literal (`'` -> `''`) without surrounding quotes.
pub fn escape_literal_inner(val: &str) -> String {
    val.replace('\'', "''")
}

/// Quotes a SQL string literal with single quotes, escaping embedded `'` as `''`.
pub fn quote_literal(val: &str) -> String {
    format!("'{}'", escape_literal_inner(val))
}

/// Escapes single quotes for a string literal nested `depth` levels deep inside SQL string literals.
/// - `depth == 1`: `'` -> `''` (standard SQL literal)
/// - `depth == 2`: `'` -> `''''` (literal inside `<alias>.query('...')`)
/// - `depth == 3`: `'` -> `''''''''` (double-nested Quack `query('... query(''''...'''') ...')`)
pub fn escape_nested_literal(val: &str, depth: usize) -> String {
    let mut out = val.to_string();
    for _ in 0..depth {
        out = out.replace('\'', "''");
    }
    out
}

/// Builds `SELECT * FROM "<alias>".query('<escaped_inner_sql>')` safely with quoted alias and escaped SQL.
pub fn wrap_quack_alias_query(alias: &str, inner_sql: &str) -> String {
    format!(
        "SELECT * FROM {}.query({})",
        quote_ident(alias),
        quote_literal(inner_sql)
    )
}

/// Builds `CALL "<alias>".query('<escaped_inner_sql>');` safely with quoted alias and escaped SQL.
pub fn wrap_quack_call_query(alias: &str, inner_sql: &str) -> String {
    format!(
        "CALL {}.query({});",
        quote_ident(alias),
        quote_literal(inner_sql)
    )
}

/// Validates that a staged temporary view DDL statement (retrieved from a Quack bridge registry)
/// is strictly a single `CREATE [OR REPLACE] TEMP[ORARY] VIEW ...` statement with no chained `;`
/// statements outside string literals, quoted identifiers, or SQL comments (H1).
pub fn validate_safe_temp_view_ddl(sql: &str) -> Result<()> {
    static TEMP_VIEW_HEAD_RE: OnceLock<Regex> = OnceLock::new();
    let head_re = TEMP_VIEW_HEAD_RE.get_or_init(|| {
        Regex::new(r"(?is)^\s*CREATE\s+(?:OR\s+REPLACE\s+)?TEMP(?:ORARY)?\s+VIEW\s+").unwrap()
    });

    let trimmed = sql.trim();
    if trimmed.is_empty() || !head_re.is_match(trimmed) {
        anyhow::bail!("Rejected staged temp view DDL: must start with CREATE [OR REPLACE] TEMP[ORARY] VIEW");
    }

    // Single-pass lexical scan to reject any semicolon followed by another SQL token
    let bytes = trimmed.as_bytes();
    let mut i = 0;
    let mut saw_semicolon = false;
    while i < bytes.len() {
        let b = bytes[i];
        // Line comment -- ... \n
        if b == b'-' && i + 1 < bytes.len() && bytes[i + 1] == b'-' {
            i += 2;
            while i < bytes.len() && bytes[i] != b'\n' && bytes[i] != b'\r' {
                i += 1;
            }
            continue;
        }
        // Nested block comment /* ... */
        if b == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'*' {
            let mut depth = 1usize;
            i += 2;
            while i < bytes.len() && depth > 0 {
                if bytes[i] == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'*' {
                    depth += 1;
                    i += 2;
                } else if bytes[i] == b'*' && i + 1 < bytes.len() && bytes[i + 1] == b'/' {
                    depth -= 1;
                    i += 2;
                } else {
                    i += 1;
                }
            }
            if depth > 0 {
                anyhow::bail!("Rejected staged temp view DDL: unclosed block comment");
            }
            continue;
        }
        if saw_semicolon {
            if b.is_ascii_whitespace() {
                i += 1;
                continue;
            }
            anyhow::bail!("Rejected staged temp view DDL: chained SQL statements are forbidden");
        }
        // Single-quoted literal
        if b == b'\'' {
            i += 1;
            let mut closed = false;
            while i < bytes.len() {
                if bytes[i] == b'\'' {
                    if i + 1 < bytes.len() && bytes[i + 1] == b'\'' {
                        i += 2;
                        continue;
                    }
                    i += 1;
                    closed = true;
                    break;
                }
                i += 1;
            }
            if !closed {
                anyhow::bail!("Rejected staged temp view DDL: unclosed string literal");
            }
            continue;
        }
        // Double-quoted identifier
        if b == b'"' {
            i += 1;
            let mut closed = false;
            while i < bytes.len() {
                if bytes[i] == b'"' {
                    if i + 1 < bytes.len() && bytes[i + 1] == b'"' {
                        i += 2;
                        continue;
                    }
                    i += 1;
                    closed = true;
                    break;
                }
                i += 1;
            }
            if !closed {
                anyhow::bail!("Rejected staged temp view DDL: unclosed quoted identifier");
            }
            continue;
        }
        // Dollar-quoted literal $tag$...$tag$
        if b == b'$' {
            let mut j = i + 1;
            while j < bytes.len() && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'_') {
                j += 1;
            }
            if j < bytes.len() && bytes[j] == b'$' {
                let tag = &bytes[i..=j];
                i = j + 1;
                let mut closed = false;
                while i + tag.len() <= bytes.len() {
                    if &bytes[i..i + tag.len()] == tag {
                        i += tag.len();
                        closed = true;
                        break;
                    }
                    i += 1;
                }
                if !closed {
                    anyhow::bail!("Rejected staged temp view DDL: unclosed dollar-quoted literal");
                }
                continue;
            }
        }
        if b == b';' {
            saw_semicolon = true;
        }
        i += 1;
    }

    Ok(())
}

/// Quotes a potentially dot-separated nested schema path (e.g. `analytics.silver` -> `"analytics"."silver"`).
pub fn quote_schema_path(schema_path: &str) -> String {
    schema_path
        .split('.')
        .filter(|s| !s.is_empty())
        .map(|part| {
            let trimmed = part.trim();
            let unquoted = if trimmed.starts_with('"') && trimmed.ends_with('"') && trimmed.len() >= 2
            {
                trimmed[1..trimmed.len() - 1].replace("\"\"", "\"")
            } else {
                trimmed.to_string()
            };
            quote_ident(&unquoted)
        })
        .collect::<Vec<_>>()
        .join(".")
}

/// Validates that a DuckDB extension name contains only ASCII alphanumeric characters and underscores.
/// Prevents SQL or CDN URL injection when interpolating into `LOAD <ext>;` or extension URLs.
pub fn validate_safe_extension_name(ext_name: &str) -> Result<()> {
    let trimmed = ext_name.trim();
    if trimmed.is_empty()
        || !trimmed
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_')
    {
        anyhow::bail!(
            "Invalid DuckDB extension name '{}': only ASCII letters, digits, and '_' are permitted.",
            ext_name
        );
    }
    Ok(())
}

/// Redacts sensitive credentials (`Password=...`, `Pwd=...`, `password=...`, `scheme://user:pass@host`,
/// `?motherduck_token=...`/`&token=...`, `TOKEN '...'`, `META_TOKEN '...'`, `SECRET '...'`, `:=`,
/// and `MAP {'TOKEN': '...'}`) from connection strings, SQL statements, or error messages.
pub fn redact_connection_credentials(input: &str) -> String {
    static ADO_PASS_RE: OnceLock<Regex> = OnceLock::new();
    static LIBPQ_PASS_RE: OnceLock<Regex> = OnceLock::new();
    static URI_PASS_RE: OnceLock<Regex> = OnceLock::new();
    static QUERY_PARAM_RE: OnceLock<Regex> = OnceLock::new();
    static SQL_SECRET_RE: OnceLock<Regex> = OnceLock::new();
    static MAP_SECRET_RE: OnceLock<Regex> = OnceLock::new();

    let ado_re = ADO_PASS_RE.get_or_init(|| {
        Regex::new(r"(?i)\b(password|pwd)\s*=\s*(\{[^}]*\}|[^;'\s)]+)").unwrap()
    });
    let libpq_re = LIBPQ_PASS_RE.get_or_init(|| {
        Regex::new(r"(?i)\b(password)\s*=\s*('(?:\\'|[^'])*'|[^\s';)]+)").unwrap()
    });
    let uri_re = URI_PASS_RE
        .get_or_init(|| Regex::new(r"([a-zA-Z0-9+.-]+://[^:/@\s]+):([^@\s]+)@").unwrap());
    let query_param_re = QUERY_PARAM_RE.get_or_init(|| {
        Regex::new(r"(?i)([?&](?:motherduck_token|token|access_token|bearer_token|sas|sig)=)[^&\s'\x22)]+")
            .unwrap()
    });
    let sql_sec_re = SQL_SECRET_RE.get_or_init(|| {
        Regex::new(
            r"(?i)\b(SECRET|TOKEN|META_TOKEN|CLIENT_ID|META_CLIENT_ID|ACCESS_TOKEN|BEARER_TOKEN|CONNECTION_STRING|KEY_ID|SESSION_TOKEN|motherduck_token|quack_default_client_id)\s*(:=|=|\s+)\s*'([^']|'')*'",
        )
        .unwrap()
    });
    let map_sec_re = MAP_SECRET_RE.get_or_init(|| {
        Regex::new(
            r"(?i)('(?:token|secret|password|key_id|access_token|bearer_token|client_id|connection_string)')\s*:\s*'([^']|'')*'",
        )
        .unwrap()
    });

    let s = ado_re.replace_all(input, "$1=***");
    let s = libpq_re.replace_all(&s, "$1=***");
    let s = uri_re.replace_all(&s, "$1:***@");
    let s = query_param_re.replace_all(&s, "${1}***");
    let s = sql_sec_re.replace_all(&s, "$1$2'***'");
    map_sec_re.replace_all(&s, "$1: '***'").into_owned()
}

/// Returns `true` if a SQL statement contains secret definitions or inline credentials
/// that should be redacted before writing to `.history.sql`.
pub fn should_redact_sql_history(sql: &str) -> bool {
    static CREATE_SECRET_RE: OnceLock<Regex> = OnceLock::new();
    static URI_CRED_RE: OnceLock<Regex> = OnceLock::new();
    static KV_SECRET_RE: OnceLock<Regex> = OnceLock::new();

    let create_secret_re = CREATE_SECRET_RE.get_or_init(|| {
        Regex::new(r"(?i)\bcreate\s+(or\s+replace\s+)?(persistent\s+|temporary\s+|temp\s+)?secret\b")
            .unwrap()
    });
    let uri_cred_re = URI_CRED_RE
        .get_or_init(|| Regex::new(r"[a-zA-Z0-9+.-]+://[^:/@\s]+:[^@\s]+@").unwrap());
    let kv_secret_re = KV_SECRET_RE.get_or_init(|| {
        Regex::new(
            r"(?i)\b(secret|token|meta_token|meta_client_id|access_token|bearer_token|connection_string|key_id|session_token|client_id)\s*(:=|=|\s+)\s*'",
        )
        .unwrap()
    });

    let lower = sql.to_lowercase();
    create_secret_re.is_match(sql)
        || uri_cred_re.is_match(sql)
        || kv_secret_re.is_match(sql)
        || lower.contains("motherduck_token")
        || lower.contains("quack_default_client_id")
        || lower.contains("password=")
        || lower.contains("pwd=")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_quoting_and_nested_escaping() {
        assert_eq!(quote_ident("my\"db"), "\"my\"\"db\"");
        assert_eq!(quote_literal("it's"), "'it''s'");
        assert_eq!(escape_nested_literal("a'b", 1), "a''b");
        assert_eq!(escape_nested_literal("a'b", 2), "a''''b");
        assert_eq!(escape_nested_literal("a'b", 3), "a''''''''b");
        assert_eq!(quote_schema_path("analytics.silver"), "\"analytics\".\"silver\"");
        assert_eq!(
            wrap_quack_alias_query("q\"1", "SELECT 'hi'"),
            "SELECT * FROM \"q\"\"1\".query('SELECT ''hi''')"
        );
    }

    #[test]
    fn test_extension_name_validation() {
        assert!(validate_safe_extension_name("ducklake").is_ok());
        assert!(validate_safe_extension_name("httpfs").is_ok());
        assert!(validate_safe_extension_name("my_ext2").is_ok());
        assert!(validate_safe_extension_name("").is_err());
        assert!(validate_safe_extension_name("httpfs; DROP TABLE x").is_err());
        assert!(validate_safe_extension_name("../evil").is_err());
    }

    #[test]
    fn test_expanded_credential_redaction_and_history_detection() {
        // MotherDuck query-param token
        let md = "ATTACH 'md:my_db?motherduck_token=eyJhbGciOiJIUzI1NiJ9.secret' AS md1;";
        let red_md = redact_connection_credentials(md);
        assert!(!red_md.contains("eyJhbGciOiJIUzI1NiJ9.secret"), "{red_md}");
        assert!(red_md.contains("motherduck_token=***"), "{red_md}");
        assert!(should_redact_sql_history(md));

        // DuckLake META_TOKEN / META_CLIENT_ID & DuckDB := named syntax
        let dl = "ATTACH 'ducklake:quack:host:9494' AS lake (META_TOKEN 'tok_12345', META_CLIENT_ID 'cid_999', token := 'named_tok');";
        let red_dl = redact_connection_credentials(dl);
        assert!(!red_dl.contains("tok_12345"), "{red_dl}");
        assert!(!red_dl.contains("cid_999"), "{red_dl}");
        assert!(!red_dl.contains("named_tok"), "{red_dl}");
        assert!(should_redact_sql_history(dl));

        // CREATE TEMP SECRET & MAP {'TOKEN': '...'}
        let temp_sec = "CREATE OR REPLACE TEMP SECRET s1 (TYPE HTTP, BEARER_TOKEN 'b_xyz');";
        assert!(should_redact_sql_history(temp_sec));
        assert!(!redact_connection_credentials(temp_sec).contains("b_xyz"));

        let map_sql = "ATTACH 'x' (METADATA_PARAMETERS MAP {'TOKEN': 'map_tok_99'});";
        assert!(!redact_connection_credentials(map_sql).contains("map_tok_99"));

        // URI userinfo
        let uri_sql = "ATTACH 'postgres://admin:s3cr3t@db.example.com:5432/app' AS pg;";
        assert!(should_redact_sql_history(uri_sql));
        assert!(!redact_connection_credentials(uri_sql).contains("s3cr3t"));
    }

    #[test]
    fn test_validate_safe_temp_view_ddl_blocks_statement_chaining() {
        assert!(validate_safe_temp_view_ddl("CREATE TEMP VIEW v1 AS SELECT 1 AS a;").is_ok());
        assert!(validate_safe_temp_view_ddl(
            "CREATE OR REPLACE TEMPORARY VIEW \"v;1\" AS SELECT 'a; DROP TABLE x' AS txt;"
        )
        .is_ok());
        assert!(validate_safe_temp_view_ddl("CREATE TEMP VIEW v2 AS SELECT $tag$a;b$tag$ -- comment\n;").is_ok());

        // Reject chained statements and non-temp-view DDL
        assert!(validate_safe_temp_view_ddl("CREATE TEMP VIEW v1 AS SELECT 1; DROP TABLE prod;").is_err());
        assert!(validate_safe_temp_view_ddl("CREATE VIEW v1 AS SELECT 1;").is_err());
        assert!(validate_safe_temp_view_ddl("DROP TABLE users;").is_err());
        assert!(validate_safe_temp_view_ddl("CREATE TEMP VIEW v1 AS SELECT 'unclosed").is_err());
    }
}
