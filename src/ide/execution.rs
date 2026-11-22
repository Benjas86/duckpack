use duckdb::Connection;
use ratatui::style::{Color, Modifier, Style};
use std::path::Path;
use std::time::{Duration, Instant};

use crate::catalog::{refresh_attached_catalog, ExplorerMetadata};
use crate::ide::editor::{
    build_paginated_sql, format_duckdb_value, handle_sql_error_position, EditorTab,
};
use crate::query::{
    diagnose_remote_credential_error, extract_autoload_extension_name, is_remote_directive_query,
    is_unadorned_secret_or_setting, should_auto_route_to_quack, wrap_remote_query_for_catalog,
    SQL_KEYWORD_PATTERN,
};
use crate::sql_utils::{
    quote_ident, quote_literal, redact_connection_credentials, should_redact_sql_history,
};

/// Result returned by `execute_ide_query`, decoupling headless SQL execution from the Ratatui loop.
pub(crate) struct IdeQueryOutcome {
    pub(crate) status_msg: String,
    pub(crate) trigger_explorer_refresh: bool,
}

/// Builds a safely-escaped `SELECT * FROM read_<fmt>('...');` query for a cloud object path (H1).
pub fn build_cloud_file_select_sql(path: &str) -> String {
    let ext = path.rsplit('.').next().unwrap_or("").to_lowercase();
    let read_fn = match ext.as_str() {
        "parquet" => "read_parquet",
        "csv" => "read_csv",
        "json" | "jsonl" | "ndjson" => "read_json",
        _ => "read_parquet",
    };
    format!("SELECT * FROM {}({});", read_fn, quote_literal(path))
}

/// Single, unified `.history.sql` writer for ALL IDE query execution paths (DDL, SELECT, remote fallback).
/// Always redacts inline secrets/credentials and enforces `0600` file permissions on Unix (H3).
pub fn append_redacted_sql_history(queries_dir: &Path, query: &str, elapsed: Duration) {
    let history_path = queries_dir.join(".history.sql");
    let timestamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
    let sanitized_query = if should_redact_sql_history(query) {
        redact_connection_credentials(query.trim())
    } else {
        query.trim().to_string()
    };
    let history_entry = format!(
        "-- Executed at {} (Took {:.2?})\n{};\n\n",
        timestamp, elapsed, sanitized_query
    );

    use std::io::Write;
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(&history_path)
        {
            let _ = f.set_permissions(std::fs::Permissions::from_mode(0o600));
            let _ = f.write_all(history_entry.as_bytes());
        }
    }
    #[cfg(not(unix))]
    {
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&history_path)
        {
            let _ = f.write_all(history_entry.as_bytes());
        }
    }
}

/// Extracts the active query string from `active_tab` (handling pagination offsets and text selections).
fn extract_query_for_execution(
    active_tab: &mut EditorTab,
    trigger_pagination: Option<i32>,
) -> String {
    if let Some(dir) = trigger_pagination {
        if let Some(bq) = &active_tab.base_query {
            if dir == 0 {
                active_tab.pagination_offset = 0;
            } else {
                let new_offset = (active_tab.pagination_offset as i64
                    + (dir as i64 * active_tab.pagination_limit as i64))
                    .max(0) as usize;
                active_tab.pagination_offset = new_offset;
            }
            bq.clone()
        } else {
            String::new()
        }
    } else {
        let mut query = if let Some(((start_row, start_col), (end_row, end_col))) =
            active_tab.textarea.selection_range()
        {
            let lines = active_tab.textarea.lines();
            let mut selected_text = Vec::new();
            for row in start_row..=end_row {
                if row >= lines.len() {
                    continue;
                }
                let chars: Vec<char> = lines[row].chars().collect();
                let s = if row == start_row { start_col } else { 0 };
                let e = if row == end_row { end_col } else { chars.len() };
                let s = s.min(chars.len());
                let e = e.min(chars.len());
                if s <= e {
                    selected_text.push(chars[s..e].iter().collect::<String>());
                }
            }
            selected_text.join("\n")
        } else {
            active_tab.textarea.lines().join("\n")
        };
        if query.is_empty() {
            query = active_tab.textarea.lines().join("\n");
        }
        active_tab.pagination_offset = 0;
        active_tab.base_query = Some(query.clone());
        query
    }
}

fn strip_leading_sql_comments_lower(query: &str) -> String {
    let mut clean_lower = query.trim().to_lowercase();
    loop {
        if clean_lower.starts_with("--") {
            if let Some(idx) = clean_lower.find('\n') {
                clean_lower = clean_lower[idx..].trim_start().to_string();
            } else {
                clean_lower = String::new();
            }
        } else if clean_lower.starts_with("/*") {
            if let Some(idx) = clean_lower.find("*/") {
                clean_lower = clean_lower[idx + 2..].trim_start().to_string();
            } else {
                clean_lower = String::new();
            }
        } else {
            break;
        }
    }
    clean_lower
}

fn is_catalog_mutation_query(query: &str) -> bool {
    let lower = query.to_lowercase();
    lower.contains("attach ")
        || lower.contains("detach ")
        || lower.contains("create ")
        || lower.contains("drop ")
        || lower.contains("alter ")
        || lower.contains("use ")
}

/// Headless SQL query runner for the Terminal IDE (`IdeQueryRunner` seam).
/// Handles per-tab database switching, pagination, Quack remote routing & fallback,
/// extension autoload retries, redacted error diagnostics, and `0600` redacted `.history.sql`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn execute_ide_query(
    conn: &Connection,
    active_tab: &mut EditorTab,
    meta: &mut ExplorerMetadata,
    queries_dir: &Path,
    db_path: &str,
    is_quack: bool,
    is_remote_db: bool,
    quack_disable_ssl: bool,
    trigger_pagination: Option<i32>,
) -> Option<IdeQueryOutcome> {
    if !active_tab.target_database.is_empty() {
        let _ = conn.execute(
            &format!("USE {};", quote_ident(&active_tab.target_database)),
            [],
        );
        meta.active_database = active_tab.target_database.clone();
        for db in &mut meta.databases {
            db.is_active = db.name == active_tab.target_database;
        }
    }

    let query_to_execute = extract_query_for_execution(active_tab, trigger_pagination);
    if query_to_execute.trim().is_empty() {
        return if trigger_pagination.is_none() {
            Some(IdeQueryOutcome {
                status_msg: "Error: Query is empty".to_string(),
                trigger_explorer_refresh: false,
            })
        } else {
            None
        };
    }

    active_tab.query_results.clear();
    active_tab.column_names.clear();
    active_tab.horizontal_scroll = 0;

    active_tab
        .textarea
        .set_search_pattern(SQL_KEYWORD_PATTERN)
        .unwrap_or(());
    active_tab.textarea.set_search_style(
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
    );

    let is_explicit_remote = is_remote_directive_query(&query_to_execute);
    let active_tab_is_quack = meta
        .databases
        .iter()
        .find(|d| d.name == active_tab.target_database)
        .map(|d| {
            d.db_type.eq_ignore_ascii_case("quack")
                || d.path
                    .as_deref()
                    .map(|p| p.starts_with("quack:"))
                    .unwrap_or(false)
        })
        .unwrap_or(false)
        || (is_quack && active_tab.target_database == "target_db");

    let remote_quack_alias = if active_tab_is_quack {
        Some(active_tab.target_database.clone())
    } else {
        let mut exec_ref: &Connection = conn;
        crate::diff::find_any_quack_alias(&mut exec_ref, Some(&active_tab.target_database))
    };

    let should_route_via_quack = is_explicit_remote
        || (active_tab_is_quack && should_auto_route_to_quack(&query_to_execute));

    let (exec_query, is_paginatable, is_mutation) = if should_route_via_quack {
        let alias = remote_quack_alias.as_deref().unwrap_or("target_db");
        let (eq, is_pag) = wrap_remote_query_for_catalog(&query_to_execute, alias);
        (eq, is_pag, false)
    } else {
        let clean_lower = strip_leading_sql_comments_lower(&query_to_execute);
        let is_pag = clean_lower.starts_with("select")
            || clean_lower.starts_with("with")
            || clean_lower.starts_with("values")
            || clean_lower.starts_with("from");
        let is_mut = clean_lower.starts_with("insert")
            || clean_lower.starts_with("update")
            || clean_lower.starts_with("delete")
            || clean_lower.starts_with("create")
            || clean_lower.starts_with("drop")
            || clean_lower.starts_with("alter")
            || clean_lower.starts_with("truncate")
            || clean_lower.starts_with("attach")
            || clean_lower.starts_with("detach")
            || clean_lower.starts_with("use")
            || clean_lower.starts_with("set")
            || clean_lower.starts_with("reset")
            || clean_lower.starts_with("copy")
            || clean_lower.starts_with("checkpoint")
            || clean_lower.starts_with("vacuum")
            || clean_lower.starts_with("install")
            || clean_lower.starts_with("load");
        (query_to_execute.clone(), is_pag, is_mut)
    };

    let mut trigger_explorer_refresh = false;
    let status_msg: String;

    if is_mutation {
        active_tab.is_paginated = false;
        let start_time = Instant::now();
        let mut exec_batch_res = conn.execute_batch(&exec_query);
        if let Err(ref e) = exec_batch_res {
            let err_str = e.to_string();
            let lower_exec = exec_query.trim().to_lowercase();
            if let Some(ext) = extract_autoload_extension_name(&err_str) {
                if crate::engine::ensure_extension_loaded(conn, &ext).is_ok() {
                    exec_batch_res = Ok(());
                }
            } else if lower_exec.starts_with("install ") {
                let after = lower_exec["install ".len()..].trim();
                let ext = after
                    .trim_matches(|c| c == '\'' || c == '"' || c == ';')
                    .split_whitespace()
                    .next()
                    .unwrap_or("");
                if !ext.is_empty()
                    && !ext.starts_with("http")
                    && crate::engine::ensure_extension_loaded(conn, ext).is_ok()
                {
                    exec_batch_res = Ok(());
                }
            }
        }
        match exec_batch_res {
            Ok(()) => {
                active_tab.query_results.clear();
                active_tab.column_names.clear();
                let elapsed = start_time.elapsed();
                trigger_explorer_refresh = true;
                status_msg = format!("Statement executed successfully in {:.2?}.", elapsed);
                append_redacted_sql_history(queries_dir, &query_to_execute, elapsed);
            }
            Err(e) => {
                let err_str = redact_connection_credentials(&e.to_string());
                handle_sql_error_position(&mut active_tab.textarea, &err_str);
                let diag = diagnose_remote_credential_error(&err_str, is_remote_db);
                status_msg = format!("Execution Error: {}{}", err_str, diag);
            }
        }
    } else {
        let final_query = if is_paginatable {
            let (pag_sql, is_paginated) = build_paginated_sql(
                &exec_query,
                active_tab.pagination_limit,
                active_tab.pagination_offset,
            );
            active_tab.is_paginated = is_paginated;
            pag_sql
        } else {
            active_tab.is_paginated = false;
            exec_query.clone()
        };

        let mut prepare_res = conn.prepare(&final_query);
        if let Err(ref e) = prepare_res {
            let err_str = e.to_string();
            if let Some(ext) = extract_autoload_extension_name(&err_str) {
                if crate::engine::ensure_extension_loaded(conn, &ext).is_ok() {
                    prepare_res = conn.prepare(&final_query);
                }
            }
        }

        match prepare_res {
            Ok(mut stmt) => {
                let start_time = Instant::now();
                match stmt.query([]) {
                    Ok(mut rows) => {
                        let count = collect_rows_into_tab(&mut rows, active_tab);
                        let elapsed = start_time.elapsed();
                        let is_cat_mut = is_catalog_mutation_query(&query_to_execute);

                        if is_explicit_remote {
                            if is_cat_mut {
                                refresh_attached_catalog(conn, db_path, quack_disable_ssl);
                                trigger_explorer_refresh = true;
                            }
                            status_msg = if count > 0 {
                                format!(
                                    "Remote statement executed on server. Returned {} rows in {:.2?}.",
                                    count, elapsed
                                )
                            } else {
                                format!("Remote statement executed on server in {:.2?}.", elapsed)
                            };
                        } else if is_remote_db && is_unadorned_secret_or_setting(&query_to_execute) {
                            status_msg = "⚠️ Applied LOCALLY on DuckPack! Prefix with '-- @remote' to run session secrets/settings on the remote Quack server.".to_string();
                        } else {
                            if is_cat_mut {
                                refresh_attached_catalog(conn, db_path, quack_disable_ssl);
                                trigger_explorer_refresh = true;
                            }
                            status_msg = format!(
                                "Query executed successfully. Returned {} rows in {:.2?}.",
                                count, elapsed
                            );
                        }
                        append_redacted_sql_history(queries_dir, &query_to_execute, elapsed);
                    }
                    Err(e) => {
                        let err_str = redact_connection_credentials(&e.to_string());
                        handle_sql_error_position(&mut active_tab.textarea, &err_str);
                        let diag = diagnose_remote_credential_error(&err_str, is_remote_db);
                        status_msg = format!("Execution Error: {}{}", err_str, diag);
                    }
                }
            }
            Err(e) => {
                let mut executed_remote_fallback = false;
                let mut fallback_status = String::new();
                let local_err_str = e.to_string();
                let is_syntax_error = local_err_str.contains("syntax error at or near");
                let lower_q = query_to_execute.to_lowercase();
                let is_temp_mutation = lower_q.contains("temp table")
                    || lower_q.contains("temporary table")
                    || lower_q.contains("temp view")
                    || lower_q.contains("temporary view");

                if is_quack && !is_explicit_remote && !is_syntax_error && !is_temp_mutation {
                    let clean_query = query_to_execute
                        .trim()
                        .strip_suffix(';')
                        .unwrap_or(query_to_execute.trim());
                    let remote_sql = if is_paginatable {
                        let inner = format!(
                            "SELECT * FROM target_db.query({})",
                            quote_literal(clean_query)
                        );
                        if active_tab.is_paginated && active_tab.pagination_limit > 0 {
                            format!(
                                "{} LIMIT {} OFFSET {}",
                                inner, active_tab.pagination_limit, active_tab.pagination_offset
                            )
                        } else {
                            format!("{};", inner)
                        }
                    } else {
                        format!("FROM target_db.query({});", quote_literal(clean_query))
                    };

                    match conn.prepare(&remote_sql) {
                        Ok(mut remote_stmt) => {
                            let start_time = Instant::now();
                            match remote_stmt.query([]) {
                                Ok(mut rows) => {
                                    let count = collect_rows_into_tab(&mut rows, active_tab);
                                    let elapsed = start_time.elapsed();
                                    if is_catalog_mutation_query(&query_to_execute) {
                                        refresh_attached_catalog(conn, db_path, quack_disable_ssl);
                                        trigger_explorer_refresh = true;
                                    }
                                    fallback_status = if count > 0 {
                                        format!(
                                            "Remote statement executed successfully. Returned {} rows in {:.2?}.",
                                            count, elapsed
                                        )
                                    } else {
                                        format!(
                                            "Remote statement executed successfully in {:.2?}.",
                                            elapsed
                                        )
                                    };
                                    executed_remote_fallback = true;
                                    append_redacted_sql_history(
                                        queries_dir,
                                        &query_to_execute,
                                        elapsed,
                                    );
                                }
                                Err(query_err) => {
                                    let err_str =
                                        redact_connection_credentials(&query_err.to_string());
                                    let diag =
                                        diagnose_remote_credential_error(&err_str, is_remote_db);
                                    fallback_status = format!("Execution Error: {}{}", err_str, diag);
                                    executed_remote_fallback = true;
                                }
                            }
                        }
                        Err(prep_err) => {
                            let err_str = redact_connection_credentials(&prep_err.to_string());
                            let diag = diagnose_remote_credential_error(&err_str, is_remote_db);
                            fallback_status = format!("Prepare Error: {}{}", err_str, diag);
                            executed_remote_fallback = true;
                        }
                    }
                }

                status_msg = if executed_remote_fallback {
                    fallback_status
                } else {
                    let err_str = redact_connection_credentials(&local_err_str);
                    handle_sql_error_position(&mut active_tab.textarea, &err_str);
                    let diag = diagnose_remote_credential_error(&err_str, is_remote_db);
                    format!("Prepare Error: {}{}", err_str, diag)
                };
            }
        }
    }

    Some(IdeQueryOutcome {
        status_msg,
        trigger_explorer_refresh,
    })
}

fn collect_rows_into_tab(rows: &mut duckdb::Rows<'_>, active_tab: &mut EditorTab) -> usize {
    let mut cols = Vec::new();
    if let Some(stmt_ref) = rows.as_ref() {
        for col in stmt_ref.column_names() {
            cols.push(col.to_string());
        }
    }
    active_tab.column_names = cols;
    let mut exec_rows = Vec::new();
    let mut count = 0;
    while let Ok(Some(row)) = rows.next() {
        count += 1;
        let mut str_row = Vec::new();
        for i in 0..active_tab.column_names.len() {
            let val: String = match row.get(i) {
                Ok(v) => format_duckdb_value(v),
                Err(_) => "NULL".to_string(),
            };
            str_row.push(val);
        }
        exec_rows.push(str_row);
    }
    active_tab.query_results = exec_rows;
    active_tab.results_state.select(Some(0));
    active_tab.horizontal_scroll = 0;
    count
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::load_explorer_metadata;
    use tui_textarea::TextArea;

    #[test]
    fn test_ide_query_runner_redacts_ddl_secret_history_and_enforces_0600() {
        let dir = std::env::temp_dir().join(format!(
            "dp_ide_exec_sec_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let queries_dir = dir.join("queries");
        std::fs::create_dir_all(&queries_dir).unwrap();

        let conn = Connection::open_in_memory().unwrap();
        let mut meta = load_explorer_metadata(&conn, &dir, false);
        let mut tab = EditorTab {
            active_file: None,
            textarea: TextArea::from([
                "CREATE OR REPLACE TEMP SECRET my_http (TYPE HTTP, BEARER_TOKEN 'super_secret_xyz');",
            ]),
            is_dirty: false,
            query_results: Vec::new(),
            column_names: Vec::new(),
            results_state: ratatui::widgets::TableState::default(),
            horizontal_scroll: 0,
            pagination_limit: 50,
            pagination_offset: 0,
            is_paginated: false,
            base_query: None,
            target_database: "memory".to_string(),
        };

        let outcome = execute_ide_query(
            &conn,
            &mut tab,
            &mut meta,
            &queries_dir,
            ":memory:",
            false,
            false,
            false,
            None,
        )
        .unwrap();
        assert!(
            outcome.status_msg.contains("executed successfully"),
            "Unexpected status: {}",
            outcome.status_msg
        );

        let hist_path = queries_dir.join(".history.sql");
        let hist = std::fs::read_to_string(&hist_path).unwrap();
        assert!(!hist.contains("super_secret_xyz"), "DDL secret leaked to .history.sql: {hist}");
        assert!(hist.contains("'***'"), "Expected redacted marker in history: {hist}");

        // Also verify S3 KEY_ID/SECRET redaction directly through append_redacted_sql_history
        append_redacted_sql_history(
            &queries_dir,
            "CREATE SECRET my_s3 (TYPE S3, KEY_ID 'AKIA_TEST_1', SECRET 's3_secret_999');",
            Duration::from_millis(5),
        );
        let hist2 = std::fs::read_to_string(&hist_path).unwrap();
        assert!(!hist2.contains("AKIA_TEST_1"), "KEY_ID leaked to .history.sql: {hist2}");
        assert!(!hist2.contains("s3_secret_999"), "SECRET leaked to .history.sql: {hist2}");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&hist_path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, ".history.sql must be 0600 on Unix");
        }

        // H1: Cloud path quoting escapes embedded single quotes
        let cloud_sql = build_cloud_file_select_sql("s3://bucket/o'hara.parquet");
        assert_eq!(
            cloud_sql,
            "SELECT * FROM read_parquet('s3://bucket/o''hara.parquet');"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
