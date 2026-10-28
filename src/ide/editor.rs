use base64::Engine;
use ratatui::style::{Color, Modifier, Style};
use std::path::PathBuf;
use tui_textarea::{CursorMove, TextArea};

use crate::query::SQL_KEYWORD_PATTERN;

/// Manages the currently focused UI pane inside the IDE.
/// Determines which component receives keyboard inputs.
#[derive(PartialEq, Clone, Copy, Debug)]
pub(crate) enum FocusPane {
    Explorer,
    Editor,
    Results,
}

/// Represents an individual open Query Tab within the IDE.
/// Contains an isolated text area and its own dedicated query results and scrolling state.
pub(crate) struct EditorTab<'a> {
    pub(crate) textarea: TextArea<'a>,
    pub(crate) active_file: Option<PathBuf>,
    pub(crate) is_dirty: bool,
    pub(crate) query_results: Vec<Vec<String>>,
    pub(crate) column_names: Vec<String>,
    pub(crate) horizontal_scroll: usize,
    pub(crate) results_state: ratatui::widgets::TableState,
    pub(crate) base_query: Option<String>,
    pub(crate) pagination_offset: usize,
    pub(crate) pagination_limit: usize,
    pub(crate) is_paginated: bool,
    pub(crate) target_database: String,
}

impl<'a> EditorTab<'a> {
    pub(crate) fn new(target_database: String) -> Self {
        let mut textarea = TextArea::default();
        Self::apply_default_styling(&mut textarea);
        Self {
            textarea,
            active_file: None,
            is_dirty: false,
            query_results: Vec::new(),
            column_names: Vec::new(),
            horizontal_scroll: 0,
            results_state: ratatui::widgets::TableState::default(),
            base_query: None,
            pagination_offset: 0,
            pagination_limit: 500,
            is_paginated: false,
            target_database,
        }
    }

    pub(crate) fn apply_default_styling(textarea: &mut TextArea<'a>) {
        textarea.set_search_pattern(SQL_KEYWORD_PATTERN).unwrap_or(());
        textarea.set_search_style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD));
        textarea.set_line_number_style(Style::default().fg(Color::DarkGray));
    }

    /// Loads SQL/text content into the tab's `TextArea` with standard keyword highlighting and line numbers.
    pub(crate) fn load_content(&mut self, content: &str, active_file: Option<PathBuf>) {
        self.textarea = TextArea::from(content.lines().map(|s| s.to_string()).collect::<Vec<_>>());
        Self::apply_default_styling(&mut self.textarea);
        self.active_file = active_file;
        self.is_dirty = false;
    }

    /// Returns true if the tab is an untitled empty buffer.
    pub(crate) fn is_untitled_empty(&self) -> bool {
        self.active_file.is_none() && self.textarea.lines().join("").trim().is_empty()
    }
}

/// Dynamically calculates the required vertical height of the status bar pane
/// based on the length and line wraps of the status message.
pub fn calculate_status_height(status_msg: &str, content_width: usize, total_height: u16) -> u16 {
    let status_inner_width = content_width.saturating_sub(4).max(20);
    let status_line_count: usize = status_msg
        .split('\n')
        .map(|line| {
            let chars = line.chars().count();
            if chars == 0 {
                1
            } else {
                (chars + status_inner_width - 1) / status_inner_width
            }
        })
        .sum();
    let max_status_height = (total_height / 3).clamp(3, 8);
    ((status_line_count as u16) + 2).clamp(3, max_status_height)
}

/// Sets the clipboard text across multiple channels for maximum compatibility:
/// 1. `wl-copy` on Wayland sessions
/// 2. `xclip` on X11 sessions
/// 3. `arboard` (Wayland data-control & X11)
/// 4. OSC 52 ANSI escape sequence for terminal clipboard sync
pub fn set_clipboard_text(text: &str, os_clipboard: &mut Option<arboard::Clipboard>) -> bool {
    let mut success = false;

    // 1. Wayland CLI (wl-copy)
    if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        let res = if text.len() < 65536 {
            std::process::Command::new("wl-copy").arg("--").arg(text).status()
        } else {
            std::process::Command::new("wl-copy")
                .stdin(std::process::Stdio::piped())
                .spawn()
                .and_then(|mut child| {
                    if let Some(mut stdin) = child.stdin.take() {
                        use std::io::Write;
                        let _ = stdin.write_all(text.as_bytes());
                        drop(stdin);
                    }
                    child.wait()
                })
        };
        if let Ok(status) = res {
            if status.success() {
                success = true;
            }
        }
    }

    // 2. X11 CLI (xclip) if not Wayland or as secondary
    if !success && std::env::var_os("DISPLAY").is_some() {
        if let Ok(mut child) = std::process::Command::new("xclip")
            .args(["-selection", "clipboard"])
            .stdin(std::process::Stdio::piped())
            .spawn()
        {
            if let Some(mut stdin) = child.stdin.take() {
                use std::io::Write;
                let _ = stdin.write_all(text.as_bytes());
            }
            if let Ok(status) = child.wait() {
                if status.success() {
                    success = true;
                }
            }
        }
    }

    // 3. Native arboard
    if os_clipboard.is_none() {
        *os_clipboard = arboard::Clipboard::new().ok();
    }
    if let Some(cb) = os_clipboard {
        if cb.set_text(text).is_ok() {
            success = true;
        }
    }

    // 4. Terminal OSC 52 sequence (universal across SSH, Wayland, X11, macOS, etc.)
    let encoded = base64::engine::general_purpose::STANDARD.encode(text);
    use std::io::Write;
    let _ = write!(std::io::stdout(), "\x1b]52;c;{}\x07", encoded);
    let _ = std::io::stdout().flush();

    success || true
}

/// Reads clipboard text from system clipboard, checking wl-paste, arboard, and xclip.
pub fn get_clipboard_text(os_clipboard: &mut Option<arboard::Clipboard>) -> Option<String> {
    // 1. Try Wayland CLI (wl-paste) first if Wayland session
    if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        if let Ok(output) = std::process::Command::new("wl-paste")
            .args(["--no-newline"])
            .output()
        {
            if output.status.success() {
                if let Ok(text) = String::from_utf8(output.stdout) {
                    if !text.is_empty() {
                        return Some(text);
                    }
                }
            }
        }
    }

    // 2. Try native arboard
    if os_clipboard.is_none() {
        *os_clipboard = arboard::Clipboard::new().ok();
    }
    if let Some(cb) = os_clipboard {
        if let Ok(text) = cb.get_text() {
            if !text.is_empty() {
                return Some(text);
            }
        }
    }

    // 3. Try X11 CLI (xclip)
    if let Ok(output) = std::process::Command::new("xclip")
        .args(["-selection", "clipboard", "-o"])
        .output()
    {
        if output.status.success() {
            if let Ok(text) = String::from_utf8(output.stdout) {
                if !text.is_empty() {
                    return Some(text);
                }
            }
        }
    }

    None
}

/// Extracts error location (line, column) and token from DuckDB v2.0 PEG parser errors
/// or legacy Postgres parser errors, jumping the cursor and styling the error token in bold red.
pub fn handle_sql_error_position(textarea: &mut TextArea, err_str: &str) {
    let lower = err_str.to_lowercase();
    let mut found_pos: Option<(usize, usize)> = None;

    // Pattern 1: DuckDB PEG parser "line (\d+), column (\d+)" or "syntax error at line (\d+), column (\d+)"
    if let Some(line_idx) = lower.find("line ") {
        let rest = &lower[line_idx + 5..];
        let num_str: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        if let Ok(line_num) = num_str.parse::<usize>() {
            let after_line = &rest[num_str.len()..];
            let mut col_num = 1;
            if let Some(col_idx) = after_line.find("col") {
                let col_rest = &after_line[col_idx + 3..];
                let col_digits = col_rest
                    .trim_start_matches("umn")
                    .trim_start_matches(|c: char| c.is_whitespace() || c == ':');
                let col_num_str: String = col_digits.chars().take_while(|c| c.is_ascii_digit()).collect();
                if let Ok(parsed_col) = col_num_str.parse::<usize>() {
                    col_num = parsed_col;
                }
            }
            found_pos = Some((line_num, col_num));
        }
    }

    // Jump cursor if position was parsed
    if let Some((line, col)) = found_pos {
        let row_idx = (line.saturating_sub(1)) as u16;
        let col_idx = (col.saturating_sub(1)) as u16;
        textarea.move_cursor(CursorMove::Jump(row_idx, col_idx));
    }

    // Highlight error token:
    // Case A: legacy DuckDB 1.x "syntax error at or near \"<word>\""
    if err_str.contains("syntax error at or near") {
        if let Some(pos) = err_str.find('"') {
            if let Some(end_pos) = err_str[pos + 1..].find('"') {
                let broken_word = &err_str[pos + 1..pos + 1 + end_pos];
                textarea
                    .set_search_pattern(&format!("(?i)\\b{}\\b", regex::escape(broken_word)))
                    .unwrap_or(());
                textarea.set_search_style(Style::default().fg(Color::Red).add_modifier(Modifier::BOLD));
                return;
            }
        }
    }

    // Case B: token under cursor if line and col found
    if let Some((line, col)) = found_pos {
        let lines = textarea.lines();
        let row_idx = line.saturating_sub(1);
        if row_idx < lines.len() {
            let line_text = &lines[row_idx];
            let col_idx = col.saturating_sub(1);
            if col_idx < line_text.len() {
                let tail = &line_text[col_idx..];
                let token: String = tail.chars().take_while(|c| c.is_alphanumeric() || *c == '_').collect();
                if !token.is_empty() {
                    textarea
                        .set_search_pattern(&format!("(?i)\\b{}\\b", regex::escape(&token)))
                        .unwrap_or(());
                    textarea.set_search_style(Style::default().fg(Color::Red).add_modifier(Modifier::BOLD));
                }
            }
        }
    }
}

/// Translates native DuckDB data types into readable strings for the IDE Results pane.
/// Provides precise formatting for Dates, Timestamps, Numerics, and Arrays/Lists.
pub(crate) fn format_duckdb_value(v: duckdb::types::Value) -> String {
    use duckdb::types::{TimeUnit, Value};
    match v {
        Value::Null => "NULL".to_string(),
        Value::Boolean(b) => b.to_string(),
        Value::TinyInt(v) => v.to_string(),
        Value::SmallInt(v) => v.to_string(),
        Value::Int(v) => v.to_string(),
        Value::BigInt(v) => v.to_string(),
        Value::HugeInt(v) => v.to_string(),
        Value::UTinyInt(v) => v.to_string(),
        Value::USmallInt(v) => v.to_string(),
        Value::UInt(v) => v.to_string(),
        Value::UBigInt(v) => v.to_string(),
        Value::Float(v) => v.to_string(),
        Value::Double(v) => v.to_string(),
        Value::Text(t) => t,
        Value::Blob(_) => "[BLOB]".to_string(),
        Value::Decimal(d) => {
            let s = format!("{:?}", d);
            if s.starts_with("Decimal(") && s.ends_with(")") {
                s[8..s.len() - 1].to_string()
            } else {
                s
            }
        }
        Value::Timestamp(unit, val) => {
            let timestamp_secs = match unit {
                TimeUnit::Second => val,
                TimeUnit::Millisecond => val / 1000,
                TimeUnit::Microsecond => val / 1_000_000,
                TimeUnit::Nanosecond => val / 1_000_000_000,
            };
            let timestamp_nanos = match unit {
                TimeUnit::Second => 0,
                TimeUnit::Millisecond => (val % 1000) * 1_000_000,
                TimeUnit::Microsecond => (val % 1_000_000) * 1000,
                TimeUnit::Nanosecond => val % 1_000_000_000,
            };
            if let Some(dt) = chrono::DateTime::from_timestamp(timestamp_secs, timestamp_nanos as u32) {
                dt.naive_utc().format("%Y-%m-%d %H:%M:%S%.6f").to_string()
            } else {
                format!("{:?}", Value::Timestamp(unit, val))
            }
        }
        Value::Date32(days) => {
            if let Some(dt) = chrono::DateTime::from_timestamp((days as i64) * 86400, 0) {
                dt.naive_utc().date().format("%Y-%m-%d").to_string()
            } else {
                format!("{:?}", Value::Date32(days))
            }
        }
        Value::Time64(unit, val) => {
            let timestamp_nanos = match unit {
                TimeUnit::Second => val * 1_000_000_000,
                TimeUnit::Millisecond => val * 1_000_000,
                TimeUnit::Microsecond => val * 1000,
                TimeUnit::Nanosecond => val,
            };
            let secs = (timestamp_nanos / 1_000_000_000) as u32;
            let nanos = (timestamp_nanos % 1_000_000_000) as u32;
            if let Some(t) = chrono::NaiveTime::from_num_seconds_from_midnight_opt(secs, nanos) {
                t.format("%H:%M:%S%.6f").to_string()
            } else {
                format!("{:?}", Value::Time64(unit, val))
            }
        }
        _ => {
            let s = format!("{:?}", v);
            if let Some(stripped) = s.split('(').next() {
                if s.ends_with(')') && s.len() > stripped.len() {
                    return s[stripped.len() + 1..s.len() - 1].to_string();
                }
            }
            s
        }
    }
}

/// Generates an ASCII tree of the schema and copies a Mermaid diagram to the clipboard.
pub(crate) fn generate_schema_visualizer(
    conn: &duckdb::Connection,
    os_clipboard: &mut Option<arboard::Clipboard>,
    is_quack: bool,
) -> Result<String, Box<dyn std::error::Error>> {
    let mut remote_dbs: Vec<(String, String)> = Vec::new();
    if is_quack {
        let quack_db_sql = "SELECT database_name, type FROM target_db.query('SELECT database_name, type FROM duckdb_databases() WHERE internal = false')";
        if let Ok(mut stmt) = conn.prepare(quack_db_sql) {
            if let Ok(mut rows) = stmt.query([]) {
                while let Ok(Some(row)) = rows.next() {
                    if let (Ok(name), Ok(typ)) = (row.get::<_, String>(0), row.get::<_, String>(1)) {
                        remote_dbs.push((name, typ));
                    }
                }
            }
        }
    }
    let is_remote_multi_db = remote_dbs.len() > 1;

    let mut db_set: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let mut tables: std::collections::BTreeMap<(String, String, String), Vec<(String, String)>> =
        std::collections::BTreeMap::new();

    if is_remote_multi_db {
        let col_query = "SELECT database_name, schema_name, table_name, column_name, data_type FROM target_db.query('SELECT database_name, schema_name, table_name, column_name, data_type FROM duckdb_columns() WHERE internal = false ORDER BY database_name, schema_name, table_name, column_index');";
        if let Ok(mut stmt) = conn.prepare(col_query) {
            if let Ok(mut rows) = stmt.query([]) {
                while let Ok(Some(row)) = rows.next() {
                    if let (Ok(db_name), Ok(schema_name), Ok(table_name), Ok(col_name), Ok(data_type)) = (
                        row.get::<_, String>(0),
                        row.get::<_, String>(1),
                        row.get::<_, String>(2),
                        row.get::<_, String>(3),
                        row.get::<_, String>(4),
                    ) {
                        db_set.insert(db_name.clone());
                        tables
                            .entry((db_name, schema_name, table_name))
                            .or_default()
                            .push((col_name, data_type));
                    }
                }
            }
        }
    } else {
        // Query local columns on conn
        let col_query = "SELECT database_name, schema_name, table_name, column_name, data_type FROM duckdb_columns() WHERE internal = false ORDER BY database_name, schema_name, table_name, column_index;";
        if let Ok(mut stmt) = conn.prepare(col_query) {
            if let Ok(mut rows) = stmt.query([]) {
                while let Ok(Some(row)) = rows.next() {
                    if let (Ok(db_name), Ok(schema_name), Ok(table_name), Ok(col_name), Ok(data_type)) = (
                        row.get::<_, String>(0),
                        row.get::<_, String>(1),
                        row.get::<_, String>(2),
                        row.get::<_, String>(3),
                        row.get::<_, String>(4),
                    ) {
                        db_set.insert(db_name.clone());
                        tables
                            .entry((db_name, schema_name, table_name))
                            .or_default()
                            .push((col_name, data_type));
                    }
                }
            }
        }
        // If single remote quack is attached, query target_db columns directly
        if is_quack {
            let quack_cols = "SELECT 'target_db' AS database_name, schema_name, table_name, column_name, data_type FROM target_db.query('SELECT schema_name, table_name, column_name, data_type FROM duckdb_columns() WHERE internal = false ORDER BY schema_name, table_name, column_index')";
            if let Ok(mut stmt) = conn.prepare(quack_cols) {
                if let Ok(mut rows) = stmt.query([]) {
                    while let Ok(Some(row)) = rows.next() {
                        if let (Ok(db_name), Ok(schema_name), Ok(table_name), Ok(col_name), Ok(data_type)) = (
                            row.get::<_, String>(0),
                            row.get::<_, String>(1),
                            row.get::<_, String>(2),
                            row.get::<_, String>(3),
                            row.get::<_, String>(4),
                        ) {
                            db_set.insert(db_name.clone());
                            tables
                                .entry((db_name, schema_name, table_name))
                                .or_default()
                                .push((col_name, data_type));
                        }
                    }
                }
            }
        }
    }

    // Also get columns from attached quack databases
    let dbs_query = if is_remote_multi_db {
        "SELECT database_name, type FROM target_db.query('SELECT database_name, type FROM duckdb_databases() WHERE internal = false')".to_string()
    } else {
        "SELECT database_name, type FROM duckdb_databases() WHERE internal = false".to_string()
    };
    if let Ok(mut d_stmt) = conn.prepare(&dbs_query) {
        if let Ok(mut d_rows) = d_stmt.query([]) {
            while let Ok(Some(row)) = d_rows.next() {
                if let (Ok(name), Ok(typ)) = (row.get::<_, String>(0), row.get::<_, String>(1)) {
                    if typ.eq_ignore_ascii_case("quack") {
                        if !is_remote_multi_db && name == "target_db" {
                            continue;
                        }
                        let safe_name = name.replace("'", "''");
                        let q = if is_remote_multi_db {
                            format!(
                                "SELECT * FROM target_db.query('SELECT ''{}'' AS database_name, schema_name, table_name, column_name, data_type FROM {}.query(''SELECT schema_name, table_name, column_name, data_type FROM duckdb_columns() WHERE internal = false ORDER BY table_name, column_index'')')",
                                safe_name, safe_name
                            )
                        } else {
                            format!(
                                "SELECT '{}' AS database_name, schema_name, table_name, column_name, data_type FROM {}.query('SELECT schema_name, table_name, column_name, data_type FROM duckdb_columns() WHERE internal = false ORDER BY table_name, column_index')",
                                safe_name, safe_name
                            )
                        };
                        if let Ok(mut q_stmt) = conn.prepare(&q) {
                            if let Ok(mut q_rows) = q_stmt.query([]) {
                                while let Ok(Some(q_row)) = q_rows.next() {
                                    if let (Ok(db), Ok(schema), Ok(table), Ok(col), Ok(dtype)) = (
                                        q_row.get(0),
                                        q_row.get(1),
                                        q_row.get(2),
                                        q_row.get(3),
                                        q_row.get(4),
                                    ) {
                                        db_set.insert(db);
                                        tables
                                            .entry((name.clone(), schema, table))
                                            .or_default()
                                            .push((col, dtype));
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // 2. Get Foreign Keys
    let mut fks: Vec<(String, String, String, String)> = Vec::new();
    if let Ok(mut stmt_fk) = conn.prepare("SELECT table_name, constraint_column_names[1], referenced_table, referenced_column_names[1] FROM duckdb_constraints() WHERE constraint_type = 'FOREIGN KEY';") {
        if let Ok(mut rows_fk) = stmt_fk.query([]) {
            while let Ok(Some(row)) = rows_fk.next() {
                if let (Ok(table_name), Ok(col_name), Ok(ref_table), Ok(ref_col)) = (row.get(0), row.get(1), row.get(2), row.get(3)) {
                    fks.push((table_name, col_name, ref_table, ref_col));
                }
            }
        }
    }

    // 3. Generate Mermaid
    let mut mermaid = String::from("erDiagram\n");
    for ((db, _schema, table), cols) in &tables {
        let entity_name = if db_set.len() > 1 {
            format!(
                "{}_{}",
                db.replace(|c: char| !c.is_alphanumeric(), "_"),
                table.replace(|c: char| !c.is_alphanumeric(), "_")
            )
        } else {
            table.replace(|c: char| !c.is_alphanumeric(), "_")
        };
        mermaid.push_str(&format!("    {} {{\n", entity_name));
        for (col, dtype) in cols {
            mermaid.push_str(&format!(
                "        {} {}\n",
                dtype.replace(" ", "_"),
                col.replace(|c: char| !c.is_alphanumeric(), "_")
            ));
        }
        mermaid.push_str("    }\n");
    }
    for (table, _col, ref_table, _ref_col) in &fks {
        let t1 = table.replace(|c: char| !c.is_alphanumeric(), "_");
        let t2 = ref_table.replace(|c: char| !c.is_alphanumeric(), "_");
        mermaid.push_str(&format!("    {} }}|--|| {} : \"\"\n", t1, t2));
    }

    // Copy Mermaid diagram using multi-channel clipboard helper
    set_clipboard_text(&mermaid, os_clipboard);

    // 4. Generate ASCII Tree
    let mut tree = String::from("Database Schema (Mermaid.js diagram copied to clipboard!)\n");
    if db_set.len() > 1 {
        let db_list: Vec<&String> = db_set.iter().collect();
        for (i, &db) in db_list.iter().enumerate() {
            let is_last_db = i == db_list.len() - 1;
            let db_prefix = if is_last_db { "└──" } else { "├──" };
            tree.push_str(&format!("{} 📁 {}\n", db_prefix, db));

            let db_tables: Vec<(&(String, String, String), &Vec<(String, String)>)> =
                tables.iter().filter(|((d, _, _), _)| d == db).collect();
            for (j, ((_, schema, table), cols)) in db_tables.iter().enumerate() {
                let is_last_table = j == db_tables.len() - 1;
                let table_prefix = if is_last_db { "    " } else { "│   " };
                let branch = if is_last_table { "└──" } else { "├──" };
                let display_tbl = if schema == "main" {
                    table.clone()
                } else {
                    format!("{}.{}", schema, table)
                };
                tree.push_str(&format!("{}{} {}\n", table_prefix, branch, display_tbl));

                for (k, (col, dtype)) in cols.iter().enumerate() {
                    let is_last_col = k == cols.len() - 1;
                    let col_prefix = if is_last_db {
                        if is_last_table { "        " } else { "    │   " }
                    } else if is_last_table {
                        "│       "
                    } else {
                        "│   │   "
                    };
                    let col_branch = if is_last_col { "└──" } else { "├──" };
                    let fk = fks.iter().find(|(t, c, _, _)| t == table && c == col);
                    if let Some((_, _, ref_table, ref_col)) = fk {
                        tree.push_str(&format!(
                            "{}{}{} ({}) -> {}.{}\n",
                            col_prefix, col_branch, col, dtype, ref_table, ref_col
                        ));
                    } else {
                        tree.push_str(&format!("{}{}{} ({})\n", col_prefix, col_branch, col, dtype));
                    }
                }
            }
        }
    } else {
        let tbl_list: Vec<(&(String, String, String), &Vec<(String, String)>)> = tables.iter().collect();
        for (i, ((_, _schema, table), cols)) in tbl_list.iter().enumerate() {
            let is_last_table = i == tbl_list.len() - 1;
            let prefix = if is_last_table { "└──" } else { "├──" };
            tree.push_str(&format!("{} {}\n", prefix, table));

            for (j, (col, dtype)) in cols.iter().enumerate() {
                let is_last_col = j == cols.len() - 1;
                let col_prefix = if is_last_table { "    " } else { "│   " };
                let line_prefix = if is_last_col { "└──" } else { "├──" };

                let fk = fks.iter().find(|(t, c, _, _)| t == table && c == col);
                if let Some((_, _, ref_table, ref_col)) = fk {
                    tree.push_str(&format!(
                        "{}{} {} ({}) -> {}.{}\n",
                        col_prefix, line_prefix, col, dtype, ref_table, ref_col
                    ));
                } else {
                    tree.push_str(&format!("{}{} {} ({})\n", col_prefix, line_prefix, col, dtype));
                }
            }
        }
    }

    Ok(tree)
}

