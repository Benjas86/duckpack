use ratatui::{
    crossterm::event::{KeyCode, KeyEvent, KeyModifiers},
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, List, ListItem, Paragraph, Wrap},
    Frame,
};

use crate::catalog::{DatabaseInfo, SnapshotEntry};
use super::input::{
    delete_char_at_cursor, delete_char_before_cursor, insert_char_at_cursor,
    render_text_with_cursor,
};

/// Time-travel query mode in the DuckLake Time-Travel Modal (`t`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TimeTravelMode {
    Version,
    Timestamp,
    TableChanges,
}

impl TimeTravelMode {
    pub(crate) fn name(&self) -> &'static str {
        match self {
            TimeTravelMode::Version => "AT (VERSION => N)",
            TimeTravelMode::Timestamp => "AT (TIMESTAMP => '...')",
            TimeTravelMode::TableChanges => "Table Changes (ducklake_table_changes)",
        }
    }

    pub(crate) fn next(&self) -> Self {
        match self {
            TimeTravelMode::Version => TimeTravelMode::Timestamp,
            TimeTravelMode::Timestamp => TimeTravelMode::TableChanges,
            TimeTravelMode::TableChanges => TimeTravelMode::Version,
        }
    }

    pub(crate) fn prev(&self) -> Self {
        match self {
            TimeTravelMode::Version => TimeTravelMode::TableChanges,
            TimeTravelMode::Timestamp => TimeTravelMode::Version,
            TimeTravelMode::TableChanges => TimeTravelMode::Timestamp,
        }
    }
}

/// Interactive modal for DuckLake snapshot time-travel and table-changes diffing (`t` in Explorer).
#[derive(Clone, Debug)]
pub(crate) struct TimeTravelModalState {
    pub(crate) is_open: bool,
    pub(crate) database: String,
    pub(crate) tables: Vec<(String, String)>, // (schema, table)
    pub(crate) selected_table_idx: usize,
    pub(crate) mode: TimeTravelMode,
    pub(crate) snapshots: Vec<SnapshotEntry>,
    pub(crate) selected_snapshot_idx: usize,
    pub(crate) version_input: String,
    pub(crate) timestamp_input: String,
    pub(crate) start_version_input: String,
    pub(crate) end_version_input: String,
    pub(crate) cursor_pos: usize,
    pub(crate) active_field: usize,
    pub(crate) error_msg: Option<String>,
}

fn quote_sql_ident(s: &str) -> String {
    if !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        s.to_string()
    } else {
        format!("\"{}\"", s.replace('"', "\"\""))
    }
}

fn quote_schema_path(schema: &str) -> String {
    schema
        .split('.')
        .map(quote_sql_ident)
        .collect::<Vec<_>>()
        .join(".")
}

impl TimeTravelModalState {
    pub(crate) fn new() -> Self {
        Self {
            is_open: false,
            database: String::new(),
            tables: Vec::new(),
            selected_table_idx: 0,
            mode: TimeTravelMode::Version,
            snapshots: Vec::new(),
            selected_snapshot_idx: 0,
            version_input: "1".to_string(),
            timestamp_input: String::new(),
            start_version_input: "0".to_string(),
            end_version_input: "1".to_string(),
            cursor_pos: 1,
            active_field: 2,
            error_msg: None,
        }
    }

    /// Opens the Time-Travel modal for a DuckLake database catalog, optionally pre-selecting a table or snapshot ID.
    pub(crate) fn open_for_ducklake(
        &mut self,
        db_info: &DatabaseInfo,
        initial_table: Option<(&str, &str)>,
        initial_snapshot_id: Option<i64>,
    ) {
        self.is_open = true;
        self.database = db_info.name.clone();
        self.tables = db_info
            .tables
            .iter()
            .filter(|t| !t.is_temporary)
            .map(|t| (t.schema.clone(), t.name.clone()))
            .collect();
        self.selected_table_idx = 0;
        if let Some((sch, tbl)) = initial_table {
            if let Some(pos) = self.tables.iter().position(|(s, t)| {
                s.eq_ignore_ascii_case(sch) && t.eq_ignore_ascii_case(tbl)
            }) {
                self.selected_table_idx = pos;
            } else {
                self.tables.insert(0, (sch.to_string(), tbl.to_string()));
                self.selected_table_idx = 0;
            }
        }

        self.snapshots = db_info.snapshots.clone();
        self.selected_snapshot_idx = 0;
        if let Some(target_id) = initial_snapshot_id {
            if let Some(pos) = self
                .snapshots
                .iter()
                .position(|s| s.snapshot_id == target_id)
            {
                self.selected_snapshot_idx = pos;
            }
        }

        self.mode = TimeTravelMode::Version;
        self.error_msg = None;
        self.sync_inputs_from_selected_snapshot(initial_snapshot_id);
        // Focus snapshot list when opened on a table, or table selector when opened on a snapshot
        self.active_field = if initial_snapshot_id.is_some() && !self.tables.is_empty() {
            0
        } else if !self.snapshots.is_empty() {
            2
        } else {
            3
        };
        self.sync_cursor_to_end();
    }

    pub(crate) fn sync_inputs_from_selected_snapshot(&mut self, fallback_id: Option<i64>) {
        if let Some(snap) = self.snapshots.get(self.selected_snapshot_idx) {
            self.version_input = snap.snapshot_id.to_string();
            self.timestamp_input = snap.snapshot_time.clone();
            let prev_id = self
                .snapshots
                .get(self.selected_snapshot_idx + 1)
                .map(|s| s.snapshot_id)
                .unwrap_or_else(|| (snap.snapshot_id - 1).max(0));
            self.start_version_input = prev_id.to_string();
            self.end_version_input = snap.snapshot_id.to_string();
        } else if let Some(id) = fallback_id {
            self.version_input = id.to_string();
            self.start_version_input = (id - 1).max(0).to_string();
            self.end_version_input = id.to_string();
        } else {
            self.version_input = "1".to_string();
            if self.timestamp_input.is_empty() {
                self.timestamp_input = "2026-01-01 00:00:00".to_string();
            }
            self.start_version_input = "0".to_string();
            self.end_version_input = "1".to_string();
        }
    }

    pub(crate) fn field_count(&self) -> usize {
        match self.mode {
            TimeTravelMode::Version | TimeTravelMode::Timestamp => 6,
            TimeTravelMode::TableChanges => 7,
        }
    }

    pub(crate) fn submit_index(&self) -> usize {
        self.field_count() - 2
    }

    pub(crate) fn cancel_index(&self) -> usize {
        self.field_count() - 1
    }

    pub(crate) fn is_text_field(&self) -> bool {
        match self.mode {
            TimeTravelMode::Version | TimeTravelMode::Timestamp => self.active_field == 3,
            TimeTravelMode::TableChanges => self.active_field == 3 || self.active_field == 4,
        }
    }

    pub(crate) fn active_text(&self) -> Option<&str> {
        match (self.mode, self.active_field) {
            (TimeTravelMode::Version, 3) => Some(&self.version_input),
            (TimeTravelMode::Timestamp, 3) => Some(&self.timestamp_input),
            (TimeTravelMode::TableChanges, 3) => Some(&self.start_version_input),
            (TimeTravelMode::TableChanges, 4) => Some(&self.end_version_input),
            _ => None,
        }
    }

    pub(crate) fn active_text_mut(&mut self) -> Option<&mut String> {
        match (self.mode, self.active_field) {
            (TimeTravelMode::Version, 3) => Some(&mut self.version_input),
            (TimeTravelMode::Timestamp, 3) => Some(&mut self.timestamp_input),
            (TimeTravelMode::TableChanges, 3) => Some(&mut self.start_version_input),
            (TimeTravelMode::TableChanges, 4) => Some(&mut self.end_version_input),
            _ => None,
        }
    }

    pub(crate) fn sync_cursor_to_end(&mut self) {
        self.cursor_pos = self.active_text().map(|s| s.chars().count()).unwrap_or(0);
    }

    pub(crate) fn selected_table(&self) -> Option<(&str, &str)> {
        self.tables
            .get(self.selected_table_idx)
            .map(|(s, t)| (s.as_str(), t.as_str()))
    }

    /// Builds the validated, SQL-injection-safe time-travel or table-changes query.
    pub(crate) fn build_sql(&self) -> Result<String, String> {
        let (schema, table) = self.selected_table().ok_or_else(|| {
            format!(
                "No tables found in DuckLake catalog '{}'.",
                self.database
            )
        })?;

        let quoted_db = quote_sql_ident(&self.database);
        let quoted_schema = quote_schema_path(schema);
        let quoted_table = quote_sql_ident(table);
        let qualified_table = if schema == "main" {
            format!("{}.{}", quoted_db, quoted_table)
        } else {
            format!("{}.{}.{}", quoted_db, quoted_schema, quoted_table)
        };

        match self.mode {
            TimeTravelMode::Version => {
                let ver: i64 = self
                    .version_input
                    .trim()
                    .parse()
                    .map_err(|_| "Snapshot version must be a valid integer (e.g. 14).".to_string())?;
                if ver < 0 {
                    return Err("Snapshot version must be >= 0.".to_string());
                }
                Ok(format!(
                    "SELECT * FROM {} AT (VERSION => {}) LIMIT 100;",
                    qualified_table, ver
                ))
            }
            TimeTravelMode::Timestamp => {
                let ts = self.timestamp_input.trim();
                if ts.is_empty() {
                    return Err("Snapshot timestamp cannot be empty.".to_string());
                }
                let safe_ts = ts.replace('\'', "''");
                Ok(format!(
                    "SELECT * FROM {} AT (TIMESTAMP => TIMESTAMP '{}') LIMIT 100;",
                    qualified_table, safe_ts
                ))
            }
            TimeTravelMode::TableChanges => {
                let start_ver: i64 = self
                    .start_version_input
                    .trim()
                    .parse()
                    .map_err(|_| "Start snapshot version must be a valid integer.".to_string())?;
                let end_ver: i64 = self
                    .end_version_input
                    .trim()
                    .parse()
                    .map_err(|_| "End snapshot version must be a valid integer.".to_string())?;
                if start_ver < 0 || end_ver < 0 {
                    return Err("Snapshot versions must be >= 0.".to_string());
                }
                let safe_db = self.database.replace('\'', "''");
                let safe_schema = schema.replace('\'', "''");
                let safe_table = table.replace('\'', "''");
                Ok(format!(
                    "SELECT * FROM ducklake_table_changes('{}', '{}', '{}', {}, {}) LIMIT 100;",
                    safe_db, safe_schema, safe_table, start_ver, end_ver
                ))
            }
        }
    }

    pub(crate) fn render(&self, f: &mut Frame) {
        let popup_width = 86.min(f.area().width.saturating_sub(4));
        let popup_height = 25.min(f.area().height.saturating_sub(2));
        let popup_area = Rect {
            x: (f.area().width.saturating_sub(popup_width)) / 2,
            y: (f.area().height.saturating_sub(popup_height)) / 2,
            width: popup_width,
            height: popup_height,
        };
        f.render_widget(ratatui::widgets::Clear, popup_area);

        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .title(format!(
                " 🕒 DuckLake Time-Travel & Snapshot Diff — {} ",
                self.database
            ))
            .border_style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD));
        f.render_widget(block.clone(), popup_area);

        let inner = block.inner(popup_area);
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3), // 0: Target Table selector
                Constraint::Length(3), // 1: Query Mode selector
                Constraint::Min(5),    // 2: Recent Snapshots list
                Constraint::Length(3), // 3: Input row (Version / Timestamp / Start-End)
                Constraint::Length(4), // 4: Live SQL Preview
                Constraint::Length(1), // 5: Error / Hint line
                Constraint::Length(1), // 6: Action buttons
            ])
            .split(inner);

        // 0: Target Table Selector
        let table_border = if self.active_field == 0 {
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        let table_label = match self.selected_table() {
            Some((sch, tbl)) if sch == "main" => format!("  ◄ [ {} ] ►  ({} tables)", tbl, self.tables.len()),
            Some((sch, tbl)) => format!("  ◄ [ {}.{} ] ►  ({} tables)", sch, tbl, self.tables.len()),
            None => "  (no tables in catalog)".to_string(),
        };
        let table_widget = Paragraph::new(table_label)
            .style(Style::default().fg(Color::White).add_modifier(Modifier::BOLD))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Target DuckLake Table (◄/► to switch) ")
                    .border_style(table_border),
            );
        f.render_widget(table_widget, chunks[0]);

        // 1: Query Mode Selector
        let mode_border = if self.active_field == 1 {
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        let mode_widget = Paragraph::new(format!("  ◄ [ {} ] ►", self.mode.name()))
            .style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Temporal Query Mode (◄/► to switch) ")
                    .border_style(mode_border),
            );
        f.render_widget(mode_widget, chunks[1]);

        // 2: Recent Snapshots List
        let snap_border = if self.active_field == 2 {
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        let snap_items: Vec<ListItem> = if self.snapshots.is_empty() {
            vec![ListItem::new(Line::from(Span::styled(
                "  (no snapshots returned by ducklake_snapshots — enter version/timestamp below)",
                Style::default().fg(Color::DarkGray),
            )))]
        } else {
            let visible_height = chunks[2].height.saturating_sub(2).max(1) as usize;
            let start_idx = self
                .selected_snapshot_idx
                .saturating_sub(visible_height.saturating_sub(1));
            self.snapshots
                .iter()
                .enumerate()
                .skip(start_idx)
                .take(visible_height)
                .map(|(idx, s)| {
                    let is_sel = idx == self.selected_snapshot_idx;
                    let prefix = if is_sel { " ▶ 🕒 " } else { "   🕒 " };
                    let style = if is_sel {
                        Style::default()
                            .bg(Color::DarkGray)
                            .fg(Color::Yellow)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(Color::White)
                    };
                    ListItem::new(Line::from(Span::styled(
                        format!("{}{}", prefix, s.display_summary()),
                        style,
                    )))
                })
                .collect()
        };
        let snap_list = List::new(snap_items).block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!(
                    " Recent Snapshots ({}) (▲/▼ to select) ",
                    self.snapshots.len()
                ))
                .border_style(snap_border),
        );
        f.render_widget(snap_list, chunks[2]);

        // 3: Editable Input Row
        match self.mode {
            TimeTravelMode::Version => {
                let is_active = self.active_field == 3;
                let border = if is_active {
                    Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::DarkGray)
                };
                let w = Paragraph::new(render_text_with_cursor(
                    &self.version_input,
                    self.cursor_pos,
                    is_active,
                    false,
                ))
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(" Snapshot Version (AT VERSION => N) ")
                        .border_style(border),
                );
                f.render_widget(w, chunks[3]);
            }
            TimeTravelMode::Timestamp => {
                let is_active = self.active_field == 3;
                let border = if is_active {
                    Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::DarkGray)
                };
                let w = Paragraph::new(render_text_with_cursor(
                    &self.timestamp_input,
                    self.cursor_pos,
                    is_active,
                    false,
                ))
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(" Snapshot Timestamp (AT TIMESTAMP => TIMESTAMP '...') ")
                        .border_style(border),
                );
                f.render_widget(w, chunks[3]);
            }
            TimeTravelMode::TableChanges => {
                let cols = Layout::default()
                    .direction(Direction::Horizontal)
                    .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
                    .split(chunks[3]);
                let start_active = self.active_field == 3;
                let end_active = self.active_field == 4;
                let start_w = Paragraph::new(render_text_with_cursor(
                    &self.start_version_input,
                    self.cursor_pos,
                    start_active,
                    false,
                ))
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(" Start Snapshot Version ")
                        .border_style(if start_active {
                            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
                        } else {
                            Style::default().fg(Color::DarkGray)
                        }),
                );
                let end_w = Paragraph::new(render_text_with_cursor(
                    &self.end_version_input,
                    self.cursor_pos,
                    end_active,
                    false,
                ))
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(" End Snapshot Version ")
                        .border_style(if end_active {
                            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
                        } else {
                            Style::default().fg(Color::DarkGray)
                        }),
                );
                f.render_widget(start_w, cols[0]);
                f.render_widget(end_w, cols[1]);
            }
        }

        // 4: Live SQL Preview
        let preview_text = match self.build_sql() {
            Ok(sql) => sql,
            Err(e) => format!("-- {}", e),
        };
        let preview = Paragraph::new(preview_text)
            .style(Style::default().fg(Color::LightGreen))
            .wrap(Wrap { trim: true })
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" Live SQL Preview ")
                    .border_style(Style::default().fg(Color::DarkGray)),
            );
        f.render_widget(preview, chunks[4]);

        // 5: Error or Hint line
        if let Some(ref err) = self.error_msg {
            let err_p = Paragraph::new(format!("⚠️ {}", err))
                .alignment(Alignment::Center)
                .style(Style::default().fg(Color::LightRed).add_modifier(Modifier::BOLD));
            f.render_widget(err_p, chunks[5]);
        } else {
            let hint_p = Paragraph::new(
                "Tab/Shift+Tab: Switch field | ◄/►: Toggle / Move cursor | Enter: Run Query | Esc: Cancel",
            )
            .alignment(Alignment::Center)
            .style(Style::default().fg(Color::DarkGray));
            f.render_widget(hint_p, chunks[5]);
        }

        // 6: Buttons
        let submit_idx = self.submit_index();
        let cancel_idx = self.cancel_index();
        let exec_style = if self.active_field == submit_idx {
            Style::default()
                .bg(Color::Green)
                .fg(Color::Black)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Green)
        };
        let cancel_style = if self.active_field == cancel_idx {
            Style::default()
                .bg(Color::Red)
                .fg(Color::White)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Red)
        };
        let buttons = Line::from(vec![
            Span::styled(" [ Enter: Run Time-Travel Query ] ", exec_style),
            Span::raw("    "),
            Span::styled(" [ Esc: Cancel ] ", cancel_style),
        ]);
        f.render_widget(Paragraph::new(buttons).alignment(Alignment::Center), chunks[6]);
    }

    /// Processes a keyboard event in the Time-Travel modal.
    /// Returns `Some((database_name, sql_query))` when the user executes the query.
    pub(crate) fn handle_key(&mut self, key: KeyEvent) -> Option<(String, String)> {
        if key.code == KeyCode::Esc {
            self.is_open = false;
            self.error_msg = None;
            return None;
        }

        let total_fields = self.field_count();
        if key.code == KeyCode::Tab {
            self.active_field = (self.active_field + 1) % total_fields;
            self.sync_cursor_to_end();
            return None;
        }
        if key.code == KeyCode::BackTab {
            self.active_field = (self.active_field + total_fields - 1) % total_fields;
            self.sync_cursor_to_end();
            return None;
        }

        match key.code {
            KeyCode::Up => {
                if self.active_field == 2 && !self.snapshots.is_empty() && self.selected_snapshot_idx > 0 {
                    self.selected_snapshot_idx -= 1;
                    self.sync_inputs_from_selected_snapshot(None);
                } else {
                    self.active_field = (self.active_field + total_fields - 1) % total_fields;
                    self.sync_cursor_to_end();
                }
            }
            KeyCode::Down => {
                if self.active_field == 2
                    && !self.snapshots.is_empty()
                    && self.selected_snapshot_idx + 1 < self.snapshots.len()
                {
                    self.selected_snapshot_idx += 1;
                    self.sync_inputs_from_selected_snapshot(None);
                } else {
                    self.active_field = (self.active_field + 1) % total_fields;
                    self.sync_cursor_to_end();
                }
            }
            KeyCode::Left => {
                if self.active_field == 0 {
                    if !self.tables.is_empty() {
                        self.selected_table_idx =
                            (self.selected_table_idx + self.tables.len() - 1) % self.tables.len();
                    }
                } else if self.active_field == 1 {
                    self.mode = self.mode.prev();
                } else if self.is_text_field() {
                    self.cursor_pos = self.cursor_pos.saturating_sub(1);
                } else if self.active_field == self.cancel_index() {
                    self.active_field = self.submit_index();
                }
            }
            KeyCode::Right => {
                if self.active_field == 0 {
                    if !self.tables.is_empty() {
                        self.selected_table_idx = (self.selected_table_idx + 1) % self.tables.len();
                    }
                } else if self.active_field == 1 {
                    self.mode = self.mode.next();
                } else if self.is_text_field() {
                    let max_len = self.active_text().map(|s| s.chars().count()).unwrap_or(0);
                    if self.cursor_pos < max_len {
                        self.cursor_pos += 1;
                    }
                } else if self.active_field == self.submit_index() {
                    self.active_field = self.cancel_index();
                }
            }
            KeyCode::Home => {
                if self.is_text_field() {
                    self.cursor_pos = 0;
                }
            }
            KeyCode::End => {
                if self.is_text_field() {
                    self.sync_cursor_to_end();
                }
            }
            KeyCode::Backspace => {
                if self.is_text_field() {
                    let mut pos = self.cursor_pos;
                    if let Some(buf) = self.active_text_mut() {
                        delete_char_before_cursor(buf, &mut pos);
                    }
                    self.cursor_pos = pos;
                }
            }
            KeyCode::Delete => {
                if self.is_text_field() {
                    let pos = self.cursor_pos;
                    if let Some(buf) = self.active_text_mut() {
                        delete_char_at_cursor(buf, pos);
                    }
                }
            }
            KeyCode::Enter => {
                if self.active_field == self.cancel_index() {
                    self.is_open = false;
                    self.error_msg = None;
                    return None;
                }
                match self.build_sql() {
                    Ok(sql) => {
                        self.is_open = false;
                        self.error_msg = None;
                        return Some((self.database.clone(), sql));
                    }
                    Err(e) => {
                        self.error_msg = Some(e);
                    }
                }
            }
            KeyCode::Char(c) => {
                if !key.modifiers.contains(KeyModifiers::CONTROL) && self.is_text_field() {
                    let allow = match self.mode {
                        TimeTravelMode::Version | TimeTravelMode::TableChanges => c.is_ascii_digit(),
                        TimeTravelMode::Timestamp => !c.is_control(),
                    };
                    if allow {
                        let mut pos = self.cursor_pos;
                        if let Some(buf) = self.active_text_mut() {
                            insert_char_at_cursor(buf, &mut pos, c);
                        }
                        self.cursor_pos = pos;
                        self.error_msg = None;
                    }
                } else if self.active_field == 2 && !self.snapshots.is_empty() {
                    if c == 'j' && self.selected_snapshot_idx + 1 < self.snapshots.len() {
                        self.selected_snapshot_idx += 1;
                        self.sync_inputs_from_selected_snapshot(None);
                    } else if c == 'k' && self.selected_snapshot_idx > 0 {
                        self.selected_snapshot_idx -= 1;
                        self.sync_inputs_from_selected_snapshot(None);
                    }
                }
            }
            _ => {}
        }

        None
    }
}
