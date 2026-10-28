use duckdb::Connection;
use ratatui::{
    crossterm::event::{KeyCode, KeyEvent, KeyModifiers},
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, List, ListItem, Paragraph, Wrap},
    Frame,
};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::catalog::{DatabaseInfo, ExplorerMetadata};
use crate::ide::explorer::{
    apply_ide_diff_for_catalog, db_icon, generate_asset_boilerplate, get_asset_file_path,
    ProjectCategory,
};

/// State for the New Asset modal dialog (`a` or `n` in Project view).
#[derive(Clone, Debug)]
pub(crate) struct NewAssetModalState {
    pub(crate) is_open: bool,
    pub(crate) category_index: usize, // 0: Table, 1: View, 2: Macro, 3: Trigger, 4: Schema, 5: Query, 6: Seed
    pub(crate) name: String,
    pub(crate) active_field: usize, // 0: category, 1: name, 2: create, 3: cancel
    pub(crate) error_msg: Option<String>,
}

impl NewAssetModalState {
    pub(crate) fn new() -> Self {
        Self {
            is_open: false,
            category_index: 0,
            name: String::new(),
            active_field: 1,
            error_msg: None,
        }
    }

    pub(crate) fn reset(&mut self) {
        self.is_open = true;
        self.category_index = 0;
        self.name.clear();
        self.active_field = 1;
        self.error_msg = None;
    }

    pub(crate) fn current_category(&self) -> ProjectCategory {
        match self.category_index {
            0 => ProjectCategory::Table,
            1 => ProjectCategory::View,
            2 => ProjectCategory::Macro,
            3 => ProjectCategory::Trigger,
            4 => ProjectCategory::Schema,
            5 => ProjectCategory::Query,
            _ => ProjectCategory::Seed,
        }
    }

    pub(crate) fn category_label(&self) -> &'static str {
        match self.category_index {
            0 => "Table",
            1 => "View",
            2 => "Macro",
            3 => "Trigger",
            4 => "Schema",
            5 => "Query",
            _ => "Seed",
        }
    }

    pub(crate) fn render(&self, f: &mut Frame, project_dir: &Path) {
        let popup_width = 62.min(f.area().width.saturating_sub(4));
        let popup_height = 16.min(f.area().height.saturating_sub(2));
        let popup_area = Rect {
            x: (f.area().width.saturating_sub(popup_width)) / 2,
            y: (f.area().height.saturating_sub(popup_height)) / 2,
            width: popup_width,
            height: popup_height,
        };
        f.render_widget(ratatui::widgets::Clear, popup_area);

        let modal_block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .title(" ✨ New Project Asset (a / n) ")
            .border_style(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD));
        f.render_widget(modal_block.clone(), popup_area);

        let inner_area = modal_block.inner(popup_area);
        let field_layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3), // 0: Category
                Constraint::Length(3), // 1: Name
                Constraint::Length(2), // 2: Filepath preview
                Constraint::Length(2), // 3: Hint/Error
                Constraint::Length(3), // 4: Buttons
            ])
            .split(inner_area);

        // Field 0: Category
        let f0_style = if self.active_field == 0 {
            Style::default().fg(Color::Yellow)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        let cat_label = format!("  ◄ [ {} ] ►   (◄ / ► to switch)", self.category_label());
        let p0 = Paragraph::new(cat_label).block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .title(" Asset Type ")
                .border_style(f0_style),
        );
        f.render_widget(p0, field_layout[0]);

        // Field 1: Name
        let f1_style = if self.active_field == 1 {
            Style::default().fg(Color::Yellow)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        let p1 = Paragraph::new(format!(
            "{}{}",
            self.name,
            if self.active_field == 1 { "█" } else { "" }
        ))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .title(" Asset Name (e.g. users, sales_summary) ")
                .border_style(f1_style),
        );
        f.render_widget(p1, field_layout[1]);

        // Field 2: Preview target path
        let target_path = if self.name.trim().is_empty() {
            "<enter name>".to_string()
        } else {
            let p = get_asset_file_path(project_dir, self.current_category(), &self.name);
            p.strip_prefix(project_dir)
                .unwrap_or(&p)
                .display()
                .to_string()
        };
        let p_preview = Paragraph::new(format!("  📁 Target File: {}", target_path))
            .style(Style::default().fg(Color::Cyan));
        f.render_widget(p_preview, field_layout[2]);

        // Field 3: Error or Hint
        if let Some(ref err) = self.error_msg {
            f.render_widget(
                Paragraph::new(format!(" ⚠️ {}", err))
                    .style(Style::default().fg(Color::LightRed))
                    .wrap(Wrap { trim: true }),
                field_layout[3],
            );
        } else {
            f.render_widget(
                Paragraph::new("  Tab: Next Field | Enter: Create & Open | Esc: Cancel")
                    .style(Style::default().fg(Color::DarkGray))
                    .wrap(Wrap { trim: true }),
                field_layout[3],
            );
        }

        // Field 4: Buttons
        let create_btn_style = if self.active_field == 2 {
            Style::default()
                .bg(Color::Yellow)
                .fg(Color::Black)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Yellow)
        };
        let cancel_btn_style = if self.active_field == 3 {
            Style::default()
                .bg(Color::Red)
                .fg(Color::White)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        let buttons = Line::from(vec![
            Span::styled(" [ Create & Open ] ", create_btn_style),
            Span::raw("    "),
            Span::styled(" [ Cancel ] ", cancel_btn_style),
        ]);
        f.render_widget(
            Paragraph::new(buttons).alignment(Alignment::Center),
            field_layout[4],
        );
    }

    /// Processes a keyboard event while the New Asset modal is open.
    /// Returns `Some(path)` if a new asset file was created and should be opened in the editor.
    pub(crate) fn handle_key(&mut self, key: KeyEvent, project_dir: &Path) -> Option<PathBuf> {
        if key.code == KeyCode::Esc {
            self.is_open = false;
        } else if key.code == KeyCode::Tab || key.code == KeyCode::Down {
            self.active_field = (self.active_field + 1) % 4;
        } else if key.code == KeyCode::BackTab || key.code == KeyCode::Up {
            self.active_field = if self.active_field == 0 {
                3
            } else {
                self.active_field - 1
            };
        } else if self.active_field == 0
            && (key.code == KeyCode::Left || key.code == KeyCode::Char('h'))
        {
            self.category_index = if self.category_index == 0 {
                6
            } else {
                self.category_index - 1
            };
        } else if self.active_field == 0
            && (key.code == KeyCode::Right || key.code == KeyCode::Char('l'))
        {
            self.category_index = (self.category_index + 1) % 7;
        } else if self.active_field >= 2
            && (key.code == KeyCode::Left || key.code == KeyCode::Right)
        {
            self.active_field = if self.active_field == 2 { 3 } else { 2 };
        } else if key.code == KeyCode::Backspace {
            if self.active_field == 1 {
                self.name.pop();
            }
        } else if key.code == KeyCode::Enter {
            if self.active_field == 3 {
                self.is_open = false;
            } else {
                let asset_name = self.name.trim().to_string();
                if asset_name.is_empty() {
                    self.error_msg = Some("Asset name cannot be empty.".to_string());
                } else if let Err(e) = crate::diff::validate_safe_project_filename(&asset_name) {
                    self.error_msg = Some(e.to_string());
                } else {
                    let cat = self.current_category();
                    let path = get_asset_file_path(project_dir, cat, &asset_name);
                    if let Some(parent) = path.parent() {
                        let _ = std::fs::create_dir_all(parent);
                    }
                    if !path.exists() {
                        let boilerplate = generate_asset_boilerplate(cat, &asset_name);
                        let _ = std::fs::write(&path, boilerplate);
                    }
                    self.is_open = false;
                    return Some(path);
                }
            }
        } else if let KeyCode::Char(c) = key.code {
            if !key.modifiers.contains(KeyModifiers::CONTROL) && self.active_field == 1 {
                if c.is_alphanumeric() || c == '_' || c == '.' || c == '-' {
                    self.name.push(c);
                }
            }
        }
        None
    }
}

/// State for the in-IDE Diff and Apply modal dialog (F6 or Ctrl+D).
#[derive(Clone, Debug)]
pub(crate) struct InIdeDiffModalState {
    pub(crate) is_open: bool,
    pub(crate) diff_result: Option<crate::diff::DiffResult>,
    pub(crate) selected_index: usize,
    pub(crate) error_msg: Option<String>,
}

impl InIdeDiffModalState {
    pub(crate) fn new() -> Self {
        Self {
            is_open: false,
            diff_result: None,
            selected_index: 0,
            error_msg: None,
        }
    }

    pub(crate) fn reset(&mut self) {
        self.is_open = false;
        self.diff_result = None;
        self.selected_index = 0;
        self.error_msg = None;
    }

    pub(crate) fn render(&self, f: &mut Frame) {
        let popup_width = 80.min(f.area().width.saturating_sub(4));
        let popup_height = 22.min(f.area().height.saturating_sub(2));
        let popup_area = Rect {
            x: (f.area().width.saturating_sub(popup_width)) / 2,
            y: (f.area().height.saturating_sub(popup_height)) / 2,
            width: popup_width,
            height: popup_height,
        };
        f.render_widget(ratatui::widgets::Clear, popup_area);

        let modal_block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .title(" 🔍 Declarative Diff & Apply (F6 / Ctrl+D) ")
            .border_style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD));
        f.render_widget(modal_block.clone(), popup_area);

        let inner_area = modal_block.inner(popup_area);
        let err_height = if self.error_msg.is_some() { 3 } else { 2 };
        let diff_layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(2),          // 0: Summary header
                Constraint::Min(6),             // 1: Changes list
                Constraint::Length(err_height), // 2: Error message or hint
                Constraint::Length(3),          // 3: Action buttons
            ])
            .split(inner_area);

        if let Some(ref diff) = self.diff_result {
            let active_indices: Vec<usize> = diff
                .items
                .iter()
                .enumerate()
                .filter(|(_, i)| !i.ignored)
                .map(|(idx, _)| idx)
                .collect();
            if active_indices.is_empty() {
                let msg = Paragraph::new(
                    "\n  ✨ Target database catalog is completely up to date with project files!\n\n  No pending migrations or schema changes detected.",
                )
                .style(Style::default().fg(Color::Green).add_modifier(Modifier::BOLD));
                f.render_widget(msg, diff_layout[1]);

                let close_btn = Line::from(Span::styled(
                    " [ Close (Esc) ] ",
                    Style::default()
                        .bg(Color::Cyan)
                        .fg(Color::Black)
                        .add_modifier(Modifier::BOLD),
                ));
                f.render_widget(
                    Paragraph::new(close_btn).alignment(Alignment::Center),
                    diff_layout[3],
                );
            } else {
                let selected_count = active_indices
                    .iter()
                    .filter(|&&idx| diff.items[idx].selected && !diff.items[idx].locked)
                    .count();
                let summary_text = format!(
                    " Found {} pending declarative change(s) ({} selected):",
                    active_indices.len(),
                    selected_count
                );
                f.render_widget(
                    Paragraph::new(summary_text)
                        .style(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
                    diff_layout[0],
                );

                let list_lines: Vec<Line> = active_indices
                    .iter()
                    .enumerate()
                    .map(|(pos, &real_idx)| {
                        let item = &diff.items[real_idx];
                        let (badge, badge_style) = crate::tui::diff_item_compact_badge(&item.item_type);
                        let cursor = if pos == self.selected_index {
                            "▶ "
                        } else {
                            "  "
                        };
                        let (checkbox, item_style) = if item.locked {
                            (
                                Span::styled(
                                    "[L] ",
                                    Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                                ),
                                Style::default()
                                    .fg(Color::Red)
                                    .add_modifier(Modifier::CROSSED_OUT),
                            )
                        } else if item.selected {
                            (
                                Span::styled(
                                    "[x] ",
                                    Style::default().fg(Color::Green).add_modifier(Modifier::BOLD),
                                ),
                                badge_style,
                            )
                        } else {
                            (
                                Span::styled("[ ] ", Style::default().fg(Color::DarkGray)),
                                Style::default().fg(Color::DarkGray),
                            )
                        };

                        Line::from(vec![
                            Span::styled(cursor, Style::default().fg(Color::Yellow)),
                            checkbox,
                            Span::styled(
                                format!("{} ", badge),
                                item_style.add_modifier(Modifier::BOLD),
                            ),
                            Span::styled(
                                format!("{:?}: {}", item.item_type, item.obj_name),
                                if item.locked {
                                    Style::default()
                                        .fg(Color::DarkGray)
                                        .add_modifier(Modifier::CROSSED_OUT)
                                } else if !item.selected {
                                    Style::default().fg(Color::DarkGray)
                                } else {
                                    Style::default()
                                },
                            ),
                        ])
                    })
                    .collect();

                let visible_height = diff_layout[1].height.saturating_sub(2) as usize;
                let scroll_top = if self.selected_index >= visible_height {
                    (self.selected_index + 1).saturating_sub(visible_height)
                } else {
                    0
                };

                let changes_list = Paragraph::new(list_lines)
                    .scroll((scroll_top as u16, 0))
                    .block(
                        Block::default()
                            .borders(Borders::ALL)
                            .border_type(BorderType::Rounded)
                            .title(" Pending Changes "),
                    );
                f.render_widget(changes_list, diff_layout[1]);

                if let Some(ref err) = self.error_msg {
                    f.render_widget(
                        Paragraph::new(format!(" ⚠️ {}", err))
                            .style(Style::default().fg(Color::LightRed))
                            .wrap(Wrap { trim: true }),
                        diff_layout[2],
                    );
                } else {
                    f.render_widget(
                        Paragraph::new(
                            "  Space: Toggle Selection | a: Toggle All | Enter: Apply Selected | Esc: Cancel",
                        )
                        .style(Style::default().fg(Color::DarkGray)),
                        diff_layout[2],
                    );
                }

                let apply_btn_style = if selected_count > 0 {
                    Style::default()
                        .bg(Color::Green)
                        .fg(Color::Black)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().bg(Color::DarkGray).fg(Color::Gray)
                };
                let close_btn_style = Style::default().fg(Color::DarkGray);
                let buttons = Line::from(vec![
                    Span::styled(
                        format!(" [ Apply {} Selected Change(s) (Enter) ] ", selected_count),
                        apply_btn_style,
                    ),
                    Span::raw("    "),
                    Span::styled(" [ Cancel (Esc) ] ", close_btn_style),
                ]);
                f.render_widget(
                    Paragraph::new(buttons).alignment(Alignment::Center),
                    diff_layout[3],
                );
            }
        } else {
            let loading = Paragraph::new(" Computing diff against target database...")
                .style(Style::default().fg(Color::Cyan));
            f.render_widget(loading, diff_layout[1]);
        }
    }

    /// Handles keyboard input in the In-IDE Diff modal.
    /// Returns `Some(status_msg)` if changes were applied and the explorer should refresh.
    pub(crate) fn handle_key(
        &mut self,
        key: KeyEvent,
        project_dir: &Path,
        conn: &Connection,
        tab_catalog: Option<&str>,
    ) -> Option<String> {
        if key.code == KeyCode::Esc {
            self.is_open = false;
        } else if key.code == KeyCode::Char(' ') {
            if let Some(ref mut diff) = self.diff_result {
                let active_indices: Vec<usize> = diff
                    .items
                    .iter()
                    .enumerate()
                    .filter(|(_, i)| !i.ignored)
                    .map(|(idx, _)| idx)
                    .collect();
                if self.selected_index < active_indices.len() {
                    let real_idx = active_indices[self.selected_index];
                    if !diff.items[real_idx].locked && !diff.items[real_idx].ignored {
                        diff.items[real_idx].selected = !diff.items[real_idx].selected;
                        crate::tui::recompute_locks(&mut diff.items);
                        self.error_msg = None;
                    } else if diff.items[real_idx].locked {
                        self.error_msg = Some(format!(
                            "Cannot select '{}': dependent on a deselected item.",
                            diff.items[real_idx].obj_name
                        ));
                    }
                }
            }
        } else if key.code == KeyCode::Char('a') || key.code == KeyCode::Char('A') {
            if let Some(ref mut diff) = self.diff_result {
                let any_unselected = diff
                    .items
                    .iter()
                    .any(|i| !i.ignored && !i.locked && !i.selected);
                for item in diff.items.iter_mut() {
                    if !item.ignored {
                        item.selected = any_unselected;
                    }
                }
                crate::tui::recompute_locks(&mut diff.items);
                self.error_msg = None;
            }
        } else if key.code == KeyCode::Enter {
            if let Some(ref diff) = self.diff_result {
                let active_indices: Vec<usize> = diff
                    .items
                    .iter()
                    .enumerate()
                    .filter(|(_, i)| !i.ignored)
                    .map(|(idx, _)| idx)
                    .collect();
                if active_indices.is_empty() {
                    self.is_open = false;
                } else {
                    let selected_count = active_indices
                        .iter()
                        .filter(|&&idx| diff.items[idx].selected && !diff.items[idx].locked)
                        .count();
                    if selected_count > 0 {
                        match apply_ide_diff_for_catalog(project_dir, conn, diff, tab_catalog) {
                            Ok(_) => {
                                self.is_open = false;
                                return Some(format!(
                                    "✓ Applied {} selected change(s) to target catalog successfully!",
                                    selected_count
                                ));
                            }
                            Err(e) => {
                                self.error_msg = Some(format!("Apply Error: {}", e));
                            }
                        }
                    } else {
                        self.error_msg = Some(
                            "No changes selected to apply. Press Space to select changes or Esc to cancel."
                                .to_string(),
                        );
                    }
                }
            } else {
                self.is_open = false;
            }
        } else if key.code == KeyCode::Up {
            if self.selected_index > 0 {
                self.selected_index -= 1;
            }
        } else if key.code == KeyCode::Down {
            if let Some(ref diff) = self.diff_result {
                let total = diff.items.iter().filter(|i| !i.ignored).count();
                if self.selected_index + 1 < total {
                    self.selected_index += 1;
                }
            }
        }
        None
    }
}

/// Supported database presets for the Attach Database modal dialog (Ctrl+A).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum AttachProvider {
    DuckDB,
    Quack,
    MotherDuck,
    SQLite,
    PostgreSQL,
    MySQL,
    MSSQL,
}

impl AttachProvider {
    pub(crate) fn name(&self) -> &'static str {
        match self {
            AttachProvider::DuckDB => "DuckDB (Local / In-Memory)",
            AttachProvider::Quack => "Remote Quack Server",
            AttachProvider::MotherDuck => "MotherDuck Cloud",
            AttachProvider::SQLite => "SQLite",
            AttachProvider::PostgreSQL => "PostgreSQL",
            AttachProvider::MySQL => "MySQL",
            AttachProvider::MSSQL => "SQL Server / MSSQL (Community)",
        }
    }

    pub(crate) fn is_relational(&self) -> bool {
        matches!(
            self,
            AttachProvider::PostgreSQL | AttachProvider::MySQL | AttachProvider::MSSQL
        )
    }

    pub(crate) fn is_quack(&self) -> bool {
        matches!(self, AttachProvider::Quack)
    }

    pub(crate) fn is_motherduck(&self) -> bool {
        matches!(self, AttachProvider::MotherDuck)
    }

    #[allow(dead_code)]
    pub(crate) fn is_file_based(&self) -> bool {
        matches!(self, AttachProvider::DuckDB | AttachProvider::SQLite)
    }
}

/// State for the Attach Database modal dialog (Ctrl+A).
#[derive(Clone, Debug)]
pub(crate) struct AttachModalState {
    pub(crate) is_open: bool,
    pub(crate) provider_index: usize, // 0: DuckDB, 1: Quack, 2: MotherDuck, 3: SQLite, 4: PostgreSQL, 5: MySQL, 6: MSSQL
    pub(crate) path: String,          // For DuckDB / SQLite file path, MotherDuck DB name
    pub(crate) host: String,          // Host / Server (localhost, etc.)
    pub(crate) port: String,          // Port (1433, 5432, 3306, 9494)
    pub(crate) database: String,      // Database name (mydb, postgres, etc.)
    pub(crate) username: String,      // Username / User ID (sa, postgres, root)
    pub(crate) password: String,      // Password / Auth Token (Masked)
    pub(crate) alias: String,         // Catalog alias
    pub(crate) alias_is_custom: bool,
    pub(crate) read_only: bool,
    pub(crate) disable_ssl: bool,
    pub(crate) insecure: bool,
    pub(crate) active_field: usize,
    pub(crate) error_msg: Option<String>,
}

impl AttachModalState {
    pub(crate) fn new() -> Self {
        let mut s = Self {
            is_open: false,
            provider_index: 0,
            path: String::new(),
            host: String::new(),
            port: String::new(),
            database: String::new(),
            username: String::new(),
            password: String::new(),
            alias: String::new(),
            alias_is_custom: false,
            read_only: false,
            disable_ssl: false,
            insecure: false,
            active_field: 1,
            error_msg: None,
        };
        s.set_provider(0);
        s
    }

    pub(crate) fn reset(&mut self) {
        self.is_open = true;
        self.provider_index = 0;
        self.path.clear();
        self.host.clear();
        self.port.clear();
        self.database.clear();
        self.username.clear();
        self.password.clear();
        self.alias.clear();
        self.alias_is_custom = false;
        self.read_only = false;
        self.disable_ssl = false;
        self.insecure = false;
        self.active_field = 1;
        self.error_msg = None;
        self.set_provider(0);
    }

    pub(crate) fn provider(&self) -> AttachProvider {
        match self.provider_index {
            1 => AttachProvider::Quack,
            2 => AttachProvider::MotherDuck,
            3 => AttachProvider::SQLite,
            4 => AttachProvider::PostgreSQL,
            5 => AttachProvider::MySQL,
            6 => AttachProvider::MSSQL,
            _ => AttachProvider::DuckDB,
        }
    }

    pub(crate) fn set_provider(&mut self, idx: usize) {
        self.provider_index = idx % 7;
        self.error_msg = None;
        let is_default_port = matches!(self.port.as_str(), "" | "1433" | "5432" | "3306" | "9494");
        let is_default_user = matches!(self.username.as_str(), "" | "sa" | "postgres" | "root");

        match self.provider() {
            AttachProvider::MSSQL => {
                if self.host.is_empty() {
                    self.host = "localhost".to_string();
                }
                if is_default_port {
                    self.port = "1433".to_string();
                }
                if is_default_user {
                    self.username = "sa".to_string();
                }
                if !self.alias_is_custom {
                    self.alias = if !self.database.is_empty() {
                        self.database.clone()
                    } else {
                        "mssql_db".to_string()
                    };
                }
            }
            AttachProvider::PostgreSQL => {
                if self.host.is_empty() {
                    self.host = "localhost".to_string();
                }
                if is_default_port {
                    self.port = "5432".to_string();
                }
                if is_default_user {
                    self.username = "postgres".to_string();
                }
                if !self.alias_is_custom {
                    self.alias = if !self.database.is_empty() {
                        self.database.clone()
                    } else {
                        "pg_db".to_string()
                    };
                }
            }
            AttachProvider::MySQL => {
                if self.host.is_empty() {
                    self.host = "localhost".to_string();
                }
                if is_default_port {
                    self.port = "3306".to_string();
                }
                if is_default_user {
                    self.username = "root".to_string();
                }
                if !self.alias_is_custom {
                    self.alias = if !self.database.is_empty() {
                        self.database.clone()
                    } else {
                        "mysql_db".to_string()
                    };
                }
            }
            AttachProvider::Quack => {
                if self.host.is_empty() {
                    self.host = "localhost".to_string();
                }
                if is_default_port {
                    self.port = "9494".to_string();
                }
                if !self.alias_is_custom {
                    self.alias = "quack_remote".to_string();
                }
            }
            AttachProvider::MotherDuck => {
                if !self.alias_is_custom {
                    self.alias = if !self.path.is_empty() {
                        self.path.trim_start_matches("md:").to_string()
                    } else {
                        "my_db".to_string()
                    };
                }
            }
            AttachProvider::SQLite => {
                if !self.alias_is_custom {
                    self.alias = "sqlite_db".to_string();
                }
            }
            AttachProvider::DuckDB => {
                if !self.alias_is_custom {
                    self.alias = "attached_db".to_string();
                }
            }
        }
        let total = self.field_count();
        if self.active_field >= total {
            self.active_field = 1;
        }
    }

    pub(crate) fn field_count(&self) -> usize {
        let p = self.provider();
        if p.is_relational() {
            12
        } else if p.is_quack() {
            10
        } else if p.is_motherduck() {
            7
        } else {
            6
        }
    }

    #[allow(dead_code)]
    pub(crate) fn is_submit(&self) -> bool {
        let p = self.provider();
        if p.is_relational() {
            self.active_field == 10
        } else if p.is_quack() {
            self.active_field == 8
        } else if p.is_motherduck() {
            self.active_field == 5
        } else {
            self.active_field == 4
        }
    }

    pub(crate) fn is_cancel(&self) -> bool {
        let p = self.provider();
        if p.is_relational() {
            self.active_field == 11
        } else if p.is_quack() {
            self.active_field == 9
        } else if p.is_motherduck() {
            self.active_field == 6
        } else {
            self.active_field == 5
        }
    }

    pub(crate) fn is_toggle_field(&self) -> bool {
        let p = self.provider();
        if p.is_relational() {
            (7..=9).contains(&self.active_field)
        } else if p.is_quack() {
            (5..=7).contains(&self.active_field)
        } else if p.is_motherduck() {
            self.active_field == 4
        } else {
            self.active_field == 3
        }
    }

    pub(crate) fn toggle_active(&mut self) {
        let p = self.provider();
        if p.is_relational() {
            match self.active_field {
                7 => self.read_only = !self.read_only,
                8 => self.disable_ssl = !self.disable_ssl,
                9 => self.insecure = !self.insecure,
                _ => {}
            }
        } else if p.is_quack() {
            match self.active_field {
                5 => self.read_only = !self.read_only,
                6 => self.disable_ssl = !self.disable_ssl,
                7 => self.insecure = !self.insecure,
                _ => {}
            }
        } else if p.is_motherduck() {
            if self.active_field == 4 {
                self.read_only = !self.read_only;
            }
        } else if self.active_field == 3 {
            self.read_only = !self.read_only;
        }
    }

    pub(crate) fn next_field(&mut self) {
        let total = self.field_count();
        self.active_field = (self.active_field + 1) % total;
    }

    pub(crate) fn prev_field(&mut self) {
        let total = self.field_count();
        self.active_field = if self.active_field == 0 {
            total - 1
        } else {
            self.active_field - 1
        };
    }

    pub(crate) fn nav_left(&mut self) {
        let p = self.provider();
        if self.active_field == 0 {
            let prev = if self.provider_index == 0 {
                6
            } else {
                self.provider_index - 1
            };
            self.set_provider(prev);
        } else if p.is_relational() {
            match self.active_field {
                1 => self.active_field = 2,
                2 => self.active_field = 1,
                3 => self.active_field = 4,
                4 => self.active_field = 3,
                5 => self.active_field = 6,
                6 => self.active_field = 5,
                7 => self.active_field = 9,
                8 => self.active_field = 7,
                9 => self.active_field = 8,
                10 => self.active_field = 11,
                11 => self.active_field = 10,
                _ => {}
            }
        } else if p.is_quack() {
            match self.active_field {
                1 => self.active_field = 2,
                2 => self.active_field = 1,
                3 => self.active_field = 4,
                4 => self.active_field = 3,
                5 => self.active_field = 7,
                6 => self.active_field = 5,
                7 => self.active_field = 6,
                8 => self.active_field = 9,
                9 => self.active_field = 8,
                _ => {}
            }
        } else if p.is_motherduck() {
            match self.active_field {
                2 => self.active_field = 3,
                3 => self.active_field = 2,
                5 => self.active_field = 6,
                6 => self.active_field = 5,
                _ => {}
            }
        } else {
            match self.active_field {
                4 => self.active_field = 5,
                5 => self.active_field = 4,
                _ => {}
            }
        }
    }

    pub(crate) fn nav_right(&mut self) {
        let p = self.provider();
        if self.active_field == 0 {
            let next = (self.provider_index + 1) % 7;
            self.set_provider(next);
        } else if p.is_relational() {
            match self.active_field {
                1 => self.active_field = 2,
                2 => self.active_field = 1,
                3 => self.active_field = 4,
                4 => self.active_field = 3,
                5 => self.active_field = 6,
                6 => self.active_field = 5,
                7 => self.active_field = 8,
                8 => self.active_field = 9,
                9 => self.active_field = 7,
                10 => self.active_field = 11,
                11 => self.active_field = 10,
                _ => {}
            }
        } else if p.is_quack() {
            match self.active_field {
                1 => self.active_field = 2,
                2 => self.active_field = 1,
                3 => self.active_field = 4,
                4 => self.active_field = 3,
                5 => self.active_field = 6,
                6 => self.active_field = 7,
                7 => self.active_field = 5,
                8 => self.active_field = 9,
                9 => self.active_field = 8,
                _ => {}
            }
        } else if p.is_motherduck() {
            match self.active_field {
                2 => self.active_field = 3,
                3 => self.active_field = 2,
                5 => self.active_field = 6,
                6 => self.active_field = 5,
                _ => {}
            }
        } else {
            match self.active_field {
                4 => self.active_field = 5,
                5 => self.active_field = 4,
                _ => {}
            }
        }
    }

    pub(crate) fn nav_down(&mut self) {
        let p = self.provider();
        if p.is_relational() {
            self.active_field = match self.active_field {
                0 => 1,
                1 | 2 => 3,
                3 | 4 => 5,
                5 | 6 => 7,
                7..=9 => 10,
                10 | 11 => 0,
                _ => 0,
            };
        } else if p.is_quack() {
            self.active_field = match self.active_field {
                0 => 1,
                1 | 2 => 3,
                3 | 4 => 5,
                5..=7 => 8,
                8 | 9 => 0,
                _ => 0,
            };
        } else if p.is_motherduck() {
            self.active_field = match self.active_field {
                0 => 1,
                1 => 2,
                2 | 3 => 4,
                4 => 5,
                5 | 6 => 0,
                _ => 0,
            };
        } else {
            self.active_field = match self.active_field {
                0 => 1,
                1 => 2,
                2 => 3,
                3 => 4,
                4 | 5 => 0,
                _ => 0,
            };
        }
    }

    pub(crate) fn nav_up(&mut self) {
        let p = self.provider();
        if p.is_relational() {
            self.active_field = match self.active_field {
                0 => 10,
                1 | 2 => 0,
                3 | 4 => 1,
                5 | 6 => 3,
                7..=9 => 5,
                10 | 11 => 7,
                _ => 0,
            };
        } else if p.is_quack() {
            self.active_field = match self.active_field {
                0 => 8,
                1 | 2 => 0,
                3 | 4 => 1,
                5..=7 => 3,
                8 | 9 => 5,
                _ => 0,
            };
        } else if p.is_motherduck() {
            self.active_field = match self.active_field {
                0 => 5,
                1 => 0,
                2 | 3 => 1,
                4 => 2,
                5 | 6 => 4,
                _ => 0,
            };
        } else {
            self.active_field = match self.active_field {
                0 => 4,
                1 => 0,
                2 => 1,
                3 => 2,
                4 | 5 => 3,
                _ => 0,
            };
        }
    }

    pub(crate) fn handle_char(&mut self, c: char) {
        let p = self.provider();
        if p.is_relational() {
            match self.active_field {
                1 => self.host.push(c),
                2 => {
                    if c.is_ascii_digit() {
                        self.port.push(c);
                    }
                }
                3 => {
                    self.database.push(c);
                    if !self.alias_is_custom {
                        self.alias = self.database.clone();
                    }
                }
                4 => self.username.push(c),
                5 => self.password.push(c),
                6 => {
                    self.alias.push(c);
                    self.alias_is_custom = true;
                }
                _ => {}
            }
        } else if p.is_quack() {
            match self.active_field {
                1 => self.host.push(c),
                2 => {
                    if c.is_ascii_digit() {
                        self.port.push(c);
                    }
                }
                3 => self.password.push(c),
                4 => {
                    self.alias.push(c);
                    self.alias_is_custom = true;
                }
                _ => {}
            }
        } else if p.is_motherduck() {
            match self.active_field {
                1 => {
                    self.path.push(c);
                    if !self.alias_is_custom {
                        self.alias = self.path.trim_start_matches("md:").to_string();
                    }
                }
                2 => self.password.push(c),
                3 => {
                    self.alias.push(c);
                    self.alias_is_custom = true;
                }
                _ => {}
            }
        } else {
            match self.active_field {
                1 => {
                    self.path.push(c);
                    if !self.alias_is_custom {
                        let stem = Path::new(&self.path)
                            .file_stem()
                            .and_then(|s| s.to_str())
                            .unwrap_or("");
                        self.alias = stem.to_string();
                    }
                }
                2 => {
                    self.alias.push(c);
                    self.alias_is_custom = true;
                }
                _ => {}
            }
        }
    }

    pub(crate) fn handle_backspace(&mut self) {
        let p = self.provider();
        if p.is_relational() {
            match self.active_field {
                1 => {
                    self.host.pop();
                }
                2 => {
                    self.port.pop();
                }
                3 => {
                    self.database.pop();
                    if !self.alias_is_custom {
                        self.alias = self.database.clone();
                    }
                }
                4 => {
                    self.username.pop();
                }
                5 => {
                    self.password.pop();
                }
                6 => {
                    self.alias.pop();
                    self.alias_is_custom = true;
                }
                _ => {}
            }
        } else if p.is_quack() {
            match self.active_field {
                1 => {
                    self.host.pop();
                }
                2 => {
                    self.port.pop();
                }
                3 => {
                    self.password.pop();
                }
                4 => {
                    self.alias.pop();
                    self.alias_is_custom = true;
                }
                _ => {}
            }
        } else if p.is_motherduck() {
            match self.active_field {
                1 => {
                    self.path.pop();
                    if !self.alias_is_custom {
                        self.alias = self.path.trim_start_matches("md:").to_string();
                    }
                }
                2 => {
                    self.password.pop();
                }
                3 => {
                    self.alias.pop();
                    self.alias_is_custom = true;
                }
                _ => {}
            }
        } else {
            match self.active_field {
                1 => {
                    self.path.pop();
                    if !self.alias_is_custom {
                        let stem = Path::new(&self.path)
                            .file_stem()
                            .and_then(|s| s.to_str())
                            .unwrap_or("");
                        self.alias = stem.to_string();
                    }
                }
                2 => {
                    self.alias.pop();
                    self.alias_is_custom = true;
                }
                _ => {}
            }
        }
    }

    pub(crate) fn build_connection_target(&self) -> std::result::Result<(String, String), String> {
        let provider = self.provider();
        match provider {
            AttachProvider::MSSQL => {
                if self.host.trim().is_empty() {
                    return Err("Host / Server address cannot be empty for MSSQL.".to_string());
                }
                let host_raw = self.host.trim();
                let mut conn_str = if host_raw.contains('=') {
                    host_raw.to_string()
                } else {
                    let port = if self.port.trim().is_empty() {
                        "1433"
                    } else {
                        self.port.trim()
                    };
                    format!("Server={},{}", host_raw, port)
                };
                if !self.database.trim().is_empty()
                    && !conn_str.to_lowercase().contains("database=")
                {
                    conn_str.push_str(&format!(";Database={}", self.database.trim()));
                }
                if !self.username.trim().is_empty()
                    && !conn_str.to_lowercase().contains("user id=")
                    && !conn_str.to_lowercase().contains("uid=")
                {
                    conn_str.push_str(&format!(";User Id={}", self.username.trim()));
                }
                if !self.password.trim().is_empty()
                    && !conn_str.to_lowercase().contains("password=")
                    && !conn_str.to_lowercase().contains("pwd=")
                {
                    conn_str.push_str(&format!(";Password={}", self.password.trim()));
                }
                if (self.insecure
                    || conn_str.contains("localhost")
                    || conn_str.contains("127.0.0.1"))
                    && !conn_str.to_lowercase().contains("trustservercertificate=")
                {
                    conn_str.push_str(";TrustServerCertificate=yes");
                }
                if self.disable_ssl && !conn_str.to_lowercase().contains("encrypt=") {
                    conn_str.push_str(";Encrypt=no");
                }
                let alias = if self.alias.trim().is_empty() {
                    if !self.database.trim().is_empty() {
                        self.database.trim().to_string()
                    } else {
                        "mssql_db".to_string()
                    }
                } else {
                    self.alias.trim().to_string()
                };
                Ok((conn_str.replace('\'', "''"), alias))
            }
            AttachProvider::PostgreSQL => {
                if self.host.trim().is_empty() {
                    return Err(
                        "Host / Server address cannot be empty for PostgreSQL.".to_string()
                    );
                }
                let host_raw = self.host.trim();
                let mut uri = if host_raw.contains("://") {
                    host_raw.to_string()
                } else {
                    let port = if self.port.trim().is_empty() {
                        "5432"
                    } else {
                        self.port.trim()
                    };
                    let user_part =
                        if !self.username.trim().is_empty() && !self.password.trim().is_empty() {
                            format!("{}:{}@", self.username.trim(), self.password.trim())
                        } else if !self.username.trim().is_empty() {
                            format!("{}@", self.username.trim())
                        } else {
                            String::new()
                        };
                    let db_part = if !self.database.trim().is_empty() {
                        format!("/{}", self.database.trim())
                    } else {
                        String::new()
                    };
                    format!("postgres://{}{}:{}{}", user_part, host_raw, port, db_part)
                };
                if self.disable_ssl {
                    if uri.contains('?') {
                        if !uri.to_lowercase().contains("sslmode=") {
                            uri.push_str("&sslmode=disable");
                        }
                    } else {
                        uri.push_str("?sslmode=disable");
                    }
                } else if self.insecure {
                    if uri.contains('?') {
                        if !uri.to_lowercase().contains("sslmode=") {
                            uri.push_str("&sslmode=prefer");
                        }
                    } else {
                        uri.push_str("?sslmode=prefer");
                    }
                }
                let alias = if self.alias.trim().is_empty() {
                    if !self.database.trim().is_empty() {
                        self.database.trim().to_string()
                    } else {
                        "pg_db".to_string()
                    }
                } else {
                    self.alias.trim().to_string()
                };
                Ok((uri.replace('\'', "''"), alias))
            }
            AttachProvider::MySQL => {
                if self.host.trim().is_empty() {
                    return Err("Host / Server address cannot be empty for MySQL.".to_string());
                }
                let host_raw = self.host.trim();
                let mut uri = if host_raw.contains("://") {
                    host_raw.to_string()
                } else {
                    let port = if self.port.trim().is_empty() {
                        "3306"
                    } else {
                        self.port.trim()
                    };
                    let user_part =
                        if !self.username.trim().is_empty() && !self.password.trim().is_empty() {
                            format!("{}:{}@", self.username.trim(), self.password.trim())
                        } else if !self.username.trim().is_empty() {
                            format!("{}@", self.username.trim())
                        } else {
                            String::new()
                        };
                    let db_part = if !self.database.trim().is_empty() {
                        format!("/{}", self.database.trim())
                    } else {
                        String::new()
                    };
                    format!("mysql://{}{}:{}{}", user_part, host_raw, port, db_part)
                };
                if self.disable_ssl {
                    if uri.contains('?') {
                        if !uri.to_lowercase().contains("ssl-mode=") {
                            uri.push_str("&ssl-mode=DISABLED");
                        }
                    } else {
                        uri.push_str("?ssl-mode=DISABLED");
                    }
                }
                let alias = if self.alias.trim().is_empty() {
                    if !self.database.trim().is_empty() {
                        self.database.trim().to_string()
                    } else {
                        "mysql_db".to_string()
                    }
                } else {
                    self.alias.trim().to_string()
                };
                Ok((uri.replace('\'', "''"), alias))
            }
            AttachProvider::Quack => {
                if self.host.trim().is_empty() {
                    return Err("Host / Server cannot be empty for Remote Quack.".to_string());
                }
                let host_raw = self.host.trim();
                let stripped = host_raw
                    .strip_prefix("quack://")
                    .or_else(|| host_raw.strip_prefix("quack:"))
                    .unwrap_or(host_raw);
                let endpoint = if stripped.contains(':') {
                    stripped.to_string()
                } else {
                    let port = if self.port.trim().is_empty() {
                        "9494"
                    } else {
                        self.port.trim()
                    };
                    format!("{}:{}", stripped, port)
                };
                let target = format!("quack:{}", endpoint);
                let alias = if self.alias.trim().is_empty() {
                    "quack_remote".to_string()
                } else {
                    self.alias.trim().to_string()
                };
                Ok((target.replace('\'', "''"), alias))
            }
            AttachProvider::MotherDuck => {
                if self.path.trim().is_empty() {
                    return Err("MotherDuck database name cannot be empty.".to_string());
                }
                let target = if self.path.trim().starts_with("md:") {
                    self.path.trim().to_string()
                } else {
                    format!("md:{}", self.path.trim())
                };
                let alias = if self.alias.trim().is_empty() {
                    self.path.trim().trim_start_matches("md:").to_string()
                } else {
                    self.alias.trim().to_string()
                };
                Ok((target.replace('\'', "''"), alias))
            }
            AttachProvider::SQLite | AttachProvider::DuckDB => {
                if self.path.trim().is_empty() {
                    return Err("Database file path cannot be empty.".to_string());
                }
                let alias = if self.alias.trim().is_empty() {
                    Path::new(self.path.trim())
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("attached_db")
                        .to_string()
                } else {
                    self.alias.trim().to_string()
                };
                Ok((self.path.trim().replace('\'', "''"), alias))
            }
        }
    }

    pub(crate) fn render(&self, f: &mut Frame) {
        let popup_width = 76.min(f.area().width.saturating_sub(4));
        let err_lines = if self.error_msg.is_some() { 4 } else { 2 };
        let is_relational = self.provider().is_relational();
        let popup_height = if is_relational {
            (19 + err_lines).min(f.area().height.saturating_sub(2))
        } else {
            (16 + err_lines).min(f.area().height.saturating_sub(2))
        };
        let popup_area = Rect {
            x: (f.area().width.saturating_sub(popup_width)) / 2,
            y: (f.area().height.saturating_sub(popup_height)) / 2,
            width: popup_width,
            height: popup_height,
        };
        f.render_widget(ratatui::widgets::Clear, popup_area);

        let modal_block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .title(" 🔌 Attach Database (Ctrl+A) ")
            .border_style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD));
        f.render_widget(modal_block.clone(), popup_area);

        let inner_area = modal_block.inner(popup_area);

        if is_relational {
            let field_layout = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(3),         // 0: Provider
                    Constraint::Length(3),         // 1: Host & Port
                    Constraint::Length(3),         // 2: Database & Username
                    Constraint::Length(3),         // 3: Password & Alias
                    Constraint::Length(2),         // 4: Toggles
                    Constraint::Length(err_lines), // 5: Error/hint
                    Constraint::Length(3),         // 6: Buttons
                ])
                .split(inner_area);

            let f0_style = if self.active_field == 0 {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            let prov_label = format!("  ◄ [ {} ] ►   (◄ / ► to switch)", self.provider().name());
            let p0 = Paragraph::new(prov_label).block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .title(" Database Provider Preset ")
                    .border_style(f0_style),
            );
            f.render_widget(p0, field_layout[0]);

            let host_port_cols = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(72), Constraint::Percentage(28)])
                .split(field_layout[1]);
            let f1_style = if self.active_field == 1 {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            let p1 = Paragraph::new(format!(
                "{}{}",
                self.host,
                if self.active_field == 1 { "█" } else { "" }
            ))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .title(" Host / Server Address ")
                    .border_style(f1_style),
            );
            f.render_widget(p1, host_port_cols[0]);

            let f2_style = if self.active_field == 2 {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            let p2 = Paragraph::new(format!(
                "{}{}",
                self.port,
                if self.active_field == 2 { "█" } else { "" }
            ))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .title(" Port ")
                    .border_style(f2_style),
            );
            f.render_widget(p2, host_port_cols[1]);

            let db_user_cols = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
                .split(field_layout[2]);
            let f3_style = if self.active_field == 3 {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            let p3 = Paragraph::new(format!(
                "{}{}",
                self.database,
                if self.active_field == 3 { "█" } else { "" }
            ))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .title(" Database Name ")
                    .border_style(f3_style),
            );
            f.render_widget(p3, db_user_cols[0]);

            let user_title = match self.provider() {
                AttachProvider::MSSQL => " User ID (e.g. sa) ",
                AttachProvider::PostgreSQL => " Username (e.g. postgres) ",
                _ => " Username (e.g. root) ",
            };
            let f4_style = if self.active_field == 4 {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            let p4 = Paragraph::new(format!(
                "{}{}",
                self.username,
                if self.active_field == 4 { "█" } else { "" }
            ))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .title(user_title)
                    .border_style(f4_style),
            );
            f.render_widget(p4, db_user_cols[1]);

            let pass_alias_cols = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
                .split(field_layout[3]);
            let f5_style = if self.active_field == 5 {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            let masked_pass = "*".repeat(self.password.len());
            let p5 = Paragraph::new(format!(
                "{}{}",
                masked_pass,
                if self.active_field == 5 { "█" } else { "" }
            ))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .title(" Password (Masked) ")
                    .border_style(f5_style),
            );
            f.render_widget(p5, pass_alias_cols[0]);

            let f6_style = if self.active_field == 6 {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            let p6 = Paragraph::new(format!(
                "{}{}",
                self.alias,
                if self.active_field == 6 { "█" } else { "" }
            ))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .title(" Catalog Alias ")
                    .border_style(f6_style),
            );
            f.render_widget(p6, pass_alias_cols[1]);

            let toggle_cols = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([
                    Constraint::Percentage(33),
                    Constraint::Percentage(33),
                    Constraint::Percentage(34),
                ])
                .split(field_layout[4]);
            let f7_style = if self.active_field == 7 {
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White)
            };
            f.render_widget(
                Paragraph::new(format!(
                    "  [{}] Read-Only",
                    if self.read_only { "X" } else { " " }
                ))
                .style(f7_style),
                toggle_cols[0],
            );

            let f8_style = if self.active_field == 8 {
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White)
            };
            f.render_widget(
                Paragraph::new(format!(
                    "  [{}] Disable SSL",
                    if self.disable_ssl { "X" } else { " " }
                ))
                .style(f8_style),
                toggle_cols[1],
            );

            let f9_style = if self.active_field == 9 {
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White)
            };
            f.render_widget(
                Paragraph::new(format!(
                    "  [{}] Insecure / Trust",
                    if self.insecure { "X" } else { " " }
                ))
                .style(f9_style),
                toggle_cols[2],
            );

            if let Some(ref err) = self.error_msg {
                f.render_widget(
                    Paragraph::new(format!(" ⚠️ {}", err))
                        .style(Style::default().fg(Color::LightRed))
                        .wrap(Wrap { trim: true }),
                    field_layout[5],
                );
            } else {
                f.render_widget(
                    Paragraph::new(
                        "  Tab/Arrows: Navigate | Space: Toggle | Enter: Attach | Esc: Cancel",
                    )
                    .style(Style::default().fg(Color::DarkGray))
                    .wrap(Wrap { trim: true }),
                    field_layout[5],
                );
            }

            let attach_btn_style = if self.active_field == 10 {
                Style::default()
                    .bg(Color::Cyan)
                    .fg(Color::Black)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::Cyan)
            };
            let cancel_btn_style = if self.active_field == 11 {
                Style::default()
                    .bg(Color::Red)
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            let buttons = Line::from(vec![
                Span::styled(" [ Attach Database ] ", attach_btn_style),
                Span::raw("    "),
                Span::styled(" [ Cancel ] ", cancel_btn_style),
            ]);
            f.render_widget(
                Paragraph::new(buttons).alignment(Alignment::Center),
                field_layout[6],
            );
        } else if self.provider().is_quack() {
            let field_layout = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(3),         // 0: Provider
                    Constraint::Length(3),         // 1: Host & Port
                    Constraint::Length(3),         // 2: Token & Alias
                    Constraint::Length(2),         // 3: Toggles
                    Constraint::Length(err_lines), // 4: Error/hint
                    Constraint::Length(3),         // 5: Buttons
                ])
                .split(inner_area);

            let f0_style = if self.active_field == 0 {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            let prov_label = format!("  ◄ [ {} ] ►   (◄ / ► to switch)", self.provider().name());
            let p0 = Paragraph::new(prov_label).block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .title(" Database Provider Preset ")
                    .border_style(f0_style),
            );
            f.render_widget(p0, field_layout[0]);

            let host_port_cols = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(72), Constraint::Percentage(28)])
                .split(field_layout[1]);
            let f1_style = if self.active_field == 1 {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            let p1 = Paragraph::new(format!(
                "{}{}",
                self.host,
                if self.active_field == 1 { "█" } else { "" }
            ))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .title(" Remote Quack Host / IP (e.g. localhost) ")
                    .border_style(f1_style),
            );
            f.render_widget(p1, host_port_cols[0]);

            let f2_style = if self.active_field == 2 {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            let p2 = Paragraph::new(format!(
                "{}{}",
                self.port,
                if self.active_field == 2 { "█" } else { "" }
            ))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .title(" Port ")
                    .border_style(f2_style),
            );
            f.render_widget(p2, host_port_cols[1]);

            let tok_alias_cols = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
                .split(field_layout[2]);
            let f3_style = if self.active_field == 3 {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            let masked_token = "*".repeat(self.password.len());
            let p3 = Paragraph::new(format!(
                "{}{}",
                masked_token,
                if self.active_field == 3 { "█" } else { "" }
            ))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .title(" Auth Token (Optional, Masked) ")
                    .border_style(f3_style),
            );
            f.render_widget(p3, tok_alias_cols[0]);

            let f4_style = if self.active_field == 4 {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            let p4 = Paragraph::new(format!(
                "{}{}",
                self.alias,
                if self.active_field == 4 { "█" } else { "" }
            ))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .title(" Catalog Alias ")
                    .border_style(f4_style),
            );
            f.render_widget(p4, tok_alias_cols[1]);

            let toggle_cols = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([
                    Constraint::Percentage(33),
                    Constraint::Percentage(33),
                    Constraint::Percentage(34),
                ])
                .split(field_layout[3]);
            let f5_style = if self.active_field == 5 {
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White)
            };
            f.render_widget(
                Paragraph::new(format!(
                    "  [{}] Read-Only",
                    if self.read_only { "X" } else { " " }
                ))
                .style(f5_style),
                toggle_cols[0],
            );

            let f6_style = if self.active_field == 6 {
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White)
            };
            f.render_widget(
                Paragraph::new(format!(
                    "  [{}] Disable SSL",
                    if self.disable_ssl { "X" } else { " " }
                ))
                .style(f6_style),
                toggle_cols[1],
            );

            let f7_style = if self.active_field == 7 {
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White)
            };
            f.render_widget(
                Paragraph::new(format!(
                    "  [{}] Insecure / Trust",
                    if self.insecure { "X" } else { " " }
                ))
                .style(f7_style),
                toggle_cols[2],
            );

            if let Some(ref err) = self.error_msg {
                f.render_widget(
                    Paragraph::new(format!(" ⚠️ {}", err))
                        .style(Style::default().fg(Color::LightRed))
                        .wrap(Wrap { trim: true }),
                    field_layout[4],
                );
            } else {
                f.render_widget(
                    Paragraph::new(
                        "  Tab/Arrows: Navigate | Space: Toggle | Enter: Attach | Esc: Cancel",
                    )
                    .style(Style::default().fg(Color::DarkGray))
                    .wrap(Wrap { trim: true }),
                    field_layout[4],
                );
            }

            let attach_btn_style = if self.active_field == 8 {
                Style::default()
                    .bg(Color::Cyan)
                    .fg(Color::Black)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::Cyan)
            };
            let cancel_btn_style = if self.active_field == 9 {
                Style::default()
                    .bg(Color::Red)
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            let buttons = Line::from(vec![
                Span::styled(" [ Attach Database ] ", attach_btn_style),
                Span::raw("    "),
                Span::styled(" [ Cancel ] ", cancel_btn_style),
            ]);
            f.render_widget(
                Paragraph::new(buttons).alignment(Alignment::Center),
                field_layout[5],
            );
        } else if self.provider().is_motherduck() {
            let field_layout = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(3),         // 0: Provider
                    Constraint::Length(3),         // 1: Database Name
                    Constraint::Length(3),         // 2: Token & Alias
                    Constraint::Length(2),         // 3: Toggles
                    Constraint::Length(err_lines), // 4: Error/hint
                    Constraint::Length(3),         // 5: Buttons
                ])
                .split(inner_area);

            let f0_style = if self.active_field == 0 {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            let prov_label = format!("  ◄ [ {} ] ►   (◄ / ► to switch)", self.provider().name());
            let p0 = Paragraph::new(prov_label).block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .title(" Database Provider Preset ")
                    .border_style(f0_style),
            );
            f.render_widget(p0, field_layout[0]);

            let f1_style = if self.active_field == 1 {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            let p1 = Paragraph::new(format!(
                "{}{}",
                self.path,
                if self.active_field == 1 { "█" } else { "" }
            ))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .title(" MotherDuck Database Name (e.g. my_db or md:my_db) ")
                    .border_style(f1_style),
            );
            f.render_widget(p1, field_layout[1]);

            let tok_alias_cols = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
                .split(field_layout[2]);
            let f2_style = if self.active_field == 2 {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            let masked_token = "*".repeat(self.password.len());
            let p2 = Paragraph::new(format!(
                "{}{}",
                masked_token,
                if self.active_field == 2 { "█" } else { "" }
            ))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .title(" Service Token (Optional, Masked) ")
                    .border_style(f2_style),
            );
            f.render_widget(p2, tok_alias_cols[0]);

            let f3_style = if self.active_field == 3 {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            let p3 = Paragraph::new(format!(
                "{}{}",
                self.alias,
                if self.active_field == 3 { "█" } else { "" }
            ))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .title(" Catalog Alias ")
                    .border_style(f3_style),
            );
            f.render_widget(p3, tok_alias_cols[1]);

            let f4_style = if self.active_field == 4 {
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White)
            };
            f.render_widget(
                Paragraph::new(format!(
                    "  [{}] Read-Only",
                    if self.read_only { "X" } else { " " }
                ))
                .style(f4_style),
                field_layout[3],
            );

            if let Some(ref err) = self.error_msg {
                f.render_widget(
                    Paragraph::new(format!(" ⚠️ {}", err))
                        .style(Style::default().fg(Color::LightRed))
                        .wrap(Wrap { trim: true }),
                    field_layout[4],
                );
            } else {
                f.render_widget(
                    Paragraph::new(
                        "  Tab/Arrows: Navigate | Space: Toggle | Enter: Attach | Esc: Cancel",
                    )
                    .style(Style::default().fg(Color::DarkGray))
                    .wrap(Wrap { trim: true }),
                    field_layout[4],
                );
            }

            let attach_btn_style = if self.active_field == 5 {
                Style::default()
                    .bg(Color::Cyan)
                    .fg(Color::Black)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::Cyan)
            };
            let cancel_btn_style = if self.active_field == 6 {
                Style::default()
                    .bg(Color::Red)
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            let buttons = Line::from(vec![
                Span::styled(" [ Attach Database ] ", attach_btn_style),
                Span::raw("    "),
                Span::styled(" [ Cancel ] ", cancel_btn_style),
            ]);
            f.render_widget(
                Paragraph::new(buttons).alignment(Alignment::Center),
                field_layout[5],
            );
        } else {
            let field_layout = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(3),         // 0: Provider
                    Constraint::Length(3),         // 1: File Path
                    Constraint::Length(3),         // 2: Alias
                    Constraint::Length(2),         // 3: Read-Only toggle
                    Constraint::Length(err_lines), // 4: Error/hint
                    Constraint::Length(3),         // 5: Buttons
                ])
                .split(inner_area);

            let f0_style = if self.active_field == 0 {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            let prov_label = format!("  ◄ [ {} ] ►   (◄ / ► to switch)", self.provider().name());
            let p0 = Paragraph::new(prov_label).block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .title(" Database Provider Preset ")
                    .border_style(f0_style),
            );
            f.render_widget(p0, field_layout[0]);

            let path_title = if self.provider() == AttachProvider::DuckDB {
                " Database File Path (e.g. data.duckdb or :memory:) "
            } else {
                " SQLite Database File Path (.db, .sqlite) "
            };
            let f1_style = if self.active_field == 1 {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            let p1 = Paragraph::new(format!(
                "{}{}",
                self.path,
                if self.active_field == 1 { "█" } else { "" }
            ))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .title(path_title)
                    .border_style(f1_style),
            );
            f.render_widget(p1, field_layout[1]);

            let f2_style = if self.active_field == 2 {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            let p2 = Paragraph::new(format!(
                "{}{}",
                self.alias,
                if self.active_field == 2 { "█" } else { "" }
            ))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .title(" Catalog Alias ")
                    .border_style(f2_style),
            );
            f.render_widget(p2, field_layout[2]);

            let f3_style = if self.active_field == 3 {
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White)
            };
            f.render_widget(
                Paragraph::new(format!(
                    "  [{}] Read-Only",
                    if self.read_only { "X" } else { " " }
                ))
                .style(f3_style),
                field_layout[3],
            );

            if let Some(ref err) = self.error_msg {
                f.render_widget(
                    Paragraph::new(format!(" ⚠️ {}", err))
                        .style(Style::default().fg(Color::LightRed))
                        .wrap(Wrap { trim: true }),
                    field_layout[4],
                );
            } else {
                f.render_widget(
                    Paragraph::new(
                        "  Tab/Arrows: Navigate | Space: Toggle | Enter: Attach | Esc: Cancel",
                    )
                    .style(Style::default().fg(Color::DarkGray))
                    .wrap(Wrap { trim: true }),
                    field_layout[4],
                );
            }

            let attach_btn_style = if self.active_field == 4 {
                Style::default()
                    .bg(Color::Cyan)
                    .fg(Color::Black)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::Cyan)
            };
            let cancel_btn_style = if self.active_field == 5 {
                Style::default()
                    .bg(Color::Red)
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            let buttons = Line::from(vec![
                Span::styled(" [ Attach Database ] ", attach_btn_style),
                Span::raw("    "),
                Span::styled(" [ Cancel ] ", cancel_btn_style),
            ]);
            f.render_widget(
                Paragraph::new(buttons).alignment(Alignment::Center),
                field_layout[5],
            );
        }
    }

    /// Processes a keyboard event in the Attach Database modal.
    /// Returns `Some(attached_alias)` if a database was attached successfully.
    pub(crate) fn handle_key(&mut self, key: KeyEvent, conn: &Connection) -> Option<String> {
        if key.code == KeyCode::Esc {
            self.is_open = false;
        } else if key.code == KeyCode::Tab {
            self.next_field();
        } else if key.code == KeyCode::BackTab {
            self.prev_field();
        } else if key.code == KeyCode::Down {
            self.nav_down();
        } else if key.code == KeyCode::Up {
            self.nav_up();
        } else if key.code == KeyCode::Left
            || (self.active_field == 0 && key.code == KeyCode::Char('h'))
        {
            self.nav_left();
        } else if key.code == KeyCode::Right
            || (self.active_field == 0 && key.code == KeyCode::Char('l'))
        {
            self.nav_right();
        } else if key.code == KeyCode::Char(' ') && self.is_toggle_field() {
            self.toggle_active();
        } else if key.code == KeyCode::Enter {
            if self.is_cancel() {
                self.is_open = false;
            } else if self.is_toggle_field() {
                self.toggle_active();
            } else {
                match self.build_connection_target() {
                    Err(err) => {
                        self.error_msg = Some(err);
                    }
                    Ok((safe_path, alias)) => {
                        let provider = self.provider();
                        let ext_res = match provider {
                            AttachProvider::SQLite => {
                                crate::engine::ensure_extension_loaded(conn, "sqlite")
                            }
                            AttachProvider::PostgreSQL => {
                                crate::engine::ensure_extension_loaded(conn, "postgres")
                            }
                            AttachProvider::MySQL => {
                                crate::engine::ensure_extension_loaded(conn, "mysql")
                            }
                            AttachProvider::MSSQL => {
                                crate::engine::ensure_extension_loaded(conn, "mssql")
                            }
                            AttachProvider::MotherDuck => {
                                crate::engine::ensure_extension_loaded(conn, "motherduck")
                            }
                            AttachProvider::Quack => crate::engine::load_quack_extension(conn),
                            AttachProvider::DuckDB => Ok(()),
                        };

                        if let Err(e) = ext_res {
                            self.error_msg = Some(format!(
                                "Failed to load required extension for {}: {}",
                                provider.name(),
                                e
                            ));
                        } else {
                            if provider == AttachProvider::MotherDuck
                                && !self.password.trim().is_empty()
                            {
                                let safe_token = self.password.trim().replace('\'', "''");
                                let _ = conn.execute(
                                    &format!("SET motherduck_token='{}';", safe_token),
                                    [],
                                );
                            }
                            if provider == AttachProvider::Quack && !self.password.trim().is_empty()
                            {
                                let safe_token = self.password.trim().replace('\'', "''");
                                let _ = conn.execute(
                                    &format!("CREATE SECRET (TYPE QUACK, TOKEN '{}');", safe_token),
                                    [],
                                );
                            }

                            let mut attach_sql = format!("ATTACH '{}' AS {}", safe_path, alias);
                            let mut opts = Vec::new();
                            match provider {
                                AttachProvider::SQLite => opts.push("TYPE SQLITE".to_string()),
                                AttachProvider::PostgreSQL => {
                                    opts.push("TYPE POSTGRES".to_string())
                                }
                                AttachProvider::MySQL => opts.push("TYPE MYSQL".to_string()),
                                AttachProvider::MSSQL => opts.push("TYPE MSSQL".to_string()),
                                AttachProvider::Quack => {
                                    if self.disable_ssl
                                        || self.insecure
                                        || crate::engine::should_disable_quack_ssl(&safe_path, false)
                                    {
                                        opts.push("DISABLE_SSL true".to_string());
                                    }
                                }
                                _ => {}
                            }
                            if self.read_only {
                                opts.push("READ_ONLY".to_string());
                            }
                            if !opts.is_empty() {
                                attach_sql.push_str(&format!(" ({})", opts.join(", ")));
                            }
                            attach_sql.push(';');

                            match conn.execute(&attach_sql, []) {
                                Ok(_) => {
                                    self.is_open = false;
                                    return Some(alias);
                                }
                                Err(e) => {
                                    self.error_msg = Some(format!("Attach Error: {}", e));
                                }
                            }
                        }
                    }
                }
            }
        } else if key.code == KeyCode::Backspace {
            self.handle_backspace();
        } else if let KeyCode::Char(c) = key.code {
            if !key.modifiers.contains(KeyModifiers::CONTROL) {
                self.handle_char(c);
            }
        }
        None
    }
}

/// Supported cloud storage providers for secrets and bucket exploration.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum CloudProvider {
    S3,
    CloudflareR2,
    MinIO,
    Wasabi,
    CustomS3,
}

impl CloudProvider {
    pub(crate) fn name(&self) -> &'static str {
        match self {
            CloudProvider::S3 => "AWS S3",
            CloudProvider::CloudflareR2 => "Cloudflare R2",
            CloudProvider::MinIO => "MinIO (Self-hosted)",
            CloudProvider::Wasabi => "Wasabi Cloud",
            CloudProvider::CustomS3 => "Custom S3 Endpoint",
        }
    }
}

/// State for the Cloud Secrets & S3 modal dialog (Ctrl+K).
#[derive(Clone, Debug)]
pub(crate) struct SecretModalState {
    pub(crate) is_open: bool,
    pub(crate) provider_index: usize, // 0: S3, 1: R2, 2: MinIO, 3: Wasabi, 4: Custom
    pub(crate) name: String,
    pub(crate) key_id: String,
    pub(crate) secret_key: String,
    pub(crate) region: String,
    pub(crate) endpoint: String,
    pub(crate) bucket: String,
    pub(crate) use_ssl: bool,
    pub(crate) save_to_env: bool,
    pub(crate) active_field: usize, // 0: provider, 1: name, 2: key_id, 3: secret_key, 4: region, 5: endpoint, 6: bucket, 7: use_ssl, 8: save_to_env, 9: submit, 10: cancel
    pub(crate) error_msg: Option<String>,
}

impl SecretModalState {
    pub(crate) fn new() -> Self {
        Self {
            is_open: false,
            provider_index: 0,
            name: "s3_secret".to_string(),
            key_id: String::new(),
            secret_key: String::new(),
            region: "us-east-1".to_string(),
            endpoint: String::new(),
            bucket: String::new(),
            use_ssl: true,
            save_to_env: true,
            active_field: 2,
            error_msg: None,
        }
    }

    pub(crate) fn reset(&mut self) {
        self.is_open = true;
        self.provider_index = 0;
        self.name = "s3_secret".to_string();
        self.key_id.clear();
        self.secret_key.clear();
        self.region = "us-east-1".to_string();
        self.endpoint.clear();
        self.bucket.clear();
        self.use_ssl = true;
        self.save_to_env = true;
        self.active_field = 2;
        self.error_msg = None;
    }

    pub(crate) fn provider(&self) -> CloudProvider {
        match self.provider_index {
            1 => CloudProvider::CloudflareR2,
            2 => CloudProvider::MinIO,
            3 => CloudProvider::Wasabi,
            4 => CloudProvider::CustomS3,
            _ => CloudProvider::S3,
        }
    }

    pub(crate) fn set_provider(&mut self, idx: usize) {
        self.provider_index = idx % 5;
        match self.provider() {
            CloudProvider::S3 => {
                if self.endpoint == "localhost:9000"
                    || self.endpoint == "s3.wasabisys.com"
                    || self.endpoint.contains(".r2.cloudflarestorage.com")
                {
                    self.endpoint.clear();
                }
                if self.region == "auto" {
                    self.region = "us-east-1".to_string();
                }
                self.use_ssl = true;
            }
            CloudProvider::CloudflareR2 => {
                if self.endpoint.is_empty()
                    || self.endpoint == "localhost:9000"
                    || self.endpoint == "s3.wasabisys.com"
                {
                    self.endpoint = "<account_id>.r2.cloudflarestorage.com".to_string();
                }
                self.region = "auto".to_string();
                self.use_ssl = true;
            }
            CloudProvider::MinIO => {
                self.endpoint = "localhost:9000".to_string();
                self.region = "us-east-1".to_string();
                self.use_ssl = false;
            }
            CloudProvider::Wasabi => {
                self.endpoint = "s3.wasabisys.com".to_string();
                self.region = "us-east-1".to_string();
                self.use_ssl = true;
            }
            CloudProvider::CustomS3 => {
                self.use_ssl = true;
            }
        }
    }

    pub(crate) fn render(&self, f: &mut Frame) {
        let popup_width = 72.min(f.area().width.saturating_sub(4));
        let popup_height = 24.min(f.area().height.saturating_sub(2));
        let popup_area = Rect {
            x: (f.area().width.saturating_sub(popup_width)) / 2,
            y: (f.area().height.saturating_sub(popup_height)) / 2,
            width: popup_width,
            height: popup_height,
        };
        f.render_widget(ratatui::widgets::Clear, popup_area);

        let modal_block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .title(" 🔐 Cloud Secrets & S3 Buckets (Ctrl+K) ")
            .border_style(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD));
        f.render_widget(modal_block.clone(), popup_area);

        let inner_area = modal_block.inner(popup_area);
        let field_layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3), // 0: Provider
                Constraint::Length(3), // 1: Name
                Constraint::Length(3), // 2: Key ID
                Constraint::Length(3), // 3: Secret Key
                Constraint::Length(3), // 4: Region & Endpoint
                Constraint::Length(3), // 5: Bucket
                Constraint::Length(2), // 6: SSL & Save-to-.env toggles
                Constraint::Length(1), // 7: Hint / Error
                Constraint::Length(3), // 8: Buttons
            ])
            .split(inner_area);

        let f0_style = if self.active_field == 0 {
            Style::default().fg(Color::Yellow)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        let prov_label = format!("  ◄ [ {} ] ►   (◄ / ► to switch)", self.provider().name());
        let p0 = Paragraph::new(prov_label).block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .title(" Storage Provider ")
                .border_style(f0_style),
        );
        f.render_widget(p0, field_layout[0]);

        let f1_style = if self.active_field == 1 {
            Style::default().fg(Color::Yellow)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        let p1 = Paragraph::new(format!(
            "{}{}",
            self.name,
            if self.active_field == 1 { "█" } else { "" }
        ))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .title(" DuckDB Secret Identifier ")
                .border_style(f1_style),
        );
        f.render_widget(p1, field_layout[1]);

        let f2_style = if self.active_field == 2 {
            Style::default().fg(Color::Yellow)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        let p2 = Paragraph::new(format!(
            "{}{}",
            self.key_id,
            if self.active_field == 2 { "█" } else { "" }
        ))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .title(" Access Key ID (AWS_ACCESS_KEY_ID) ")
                .border_style(f2_style),
        );
        f.render_widget(p2, field_layout[2]);

        let f3_style = if self.active_field == 3 {
            Style::default().fg(Color::Yellow)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        let masked_secret = "*".repeat(self.secret_key.len());
        let p3 = Paragraph::new(format!(
            "{}{}",
            masked_secret,
            if self.active_field == 3 { "█" } else { "" }
        ))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .title(" Secret Access Key (AWS_SECRET_ACCESS_KEY, Masked) ")
                .border_style(f3_style),
        );
        f.render_widget(p3, field_layout[3]);

        let reg_end_cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(45), Constraint::Percentage(55)])
            .split(field_layout[4]);

        let f4_reg_style = if self.active_field == 4 {
            Style::default().fg(Color::Yellow)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        let p_reg = Paragraph::new(format!(
            "{}{}",
            self.region,
            if self.active_field == 4 { "█" } else { "" }
        ))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .title(" Region ")
                .border_style(f4_reg_style),
        );
        f.render_widget(p_reg, reg_end_cols[0]);

        let f5_end_style = if self.active_field == 5 {
            Style::default().fg(Color::Yellow)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        let p_end = Paragraph::new(format!(
            "{}{}",
            self.endpoint,
            if self.active_field == 5 { "█" } else { "" }
        ))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .title(" Custom Endpoint (Optional) ")
                .border_style(f5_end_style),
        );
        f.render_widget(p_end, reg_end_cols[1]);

        let f6_style = if self.active_field == 6 {
            Style::default().fg(Color::Yellow)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        let p6 = Paragraph::new(format!(
            "{}{}",
            self.bucket,
            if self.active_field == 6 { "█" } else { "" }
        ))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .title(" Default Bucket to Explore (e.g. s3://my-bucket) ")
                .border_style(f6_style),
        );
        f.render_widget(p6, field_layout[5]);

        let toggle_cols = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(field_layout[6]);

        let f7_ssl_style = if self.active_field == 7 {
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White)
        };
        let ssl_label = format!(
            "  [{}] Use SSL / HTTPS (Space)",
            if self.use_ssl { "X" } else { " " }
        );
        f.render_widget(Paragraph::new(ssl_label).style(f7_ssl_style), toggle_cols[0]);

        let f8_env_style = if self.active_field == 8 {
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White)
        };
        let env_label = format!(
            "  [{}] Persist in .env (Space)",
            if self.save_to_env { "X" } else { " " }
        );
        f.render_widget(Paragraph::new(env_label).style(f8_env_style), toggle_cols[1]);

        if let Some(ref err) = self.error_msg {
            f.render_widget(
                Paragraph::new(format!(" ⚠️ {}", err))
                    .style(Style::default().fg(Color::LightRed))
                    .wrap(Wrap { trim: true }),
                field_layout[7],
            );
        } else {
            f.render_widget(
                Paragraph::new(
                    "  Tab: Next Field | Space: Toggle | Enter: Save Secret | Esc: Cancel",
                )
                .style(Style::default().fg(Color::DarkGray))
                .wrap(Wrap { trim: true }),
                field_layout[7],
            );
        }

        let save_btn_style = if self.active_field == 9 {
            Style::default()
                .bg(Color::Yellow)
                .fg(Color::Black)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Yellow)
        };
        let cancel_btn_style = if self.active_field == 10 {
            Style::default()
                .bg(Color::Red)
                .fg(Color::White)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        let buttons = Line::from(vec![
            Span::styled(" [ Save & Attach Secret ] ", save_btn_style),
            Span::raw("    "),
            Span::styled(" [ Cancel ] ", cancel_btn_style),
        ]);
        f.render_widget(
            Paragraph::new(buttons).alignment(Alignment::Center),
            field_layout[8],
        );
    }

    /// Processes a keyboard event in the Cloud Secret modal.
    /// Returns `Some(status_msg)` if a secret was saved and the explorer should refresh.
    pub(crate) fn handle_key(
        &mut self,
        key: KeyEvent,
        conn: &Connection,
        project_dir: &Path,
    ) -> Option<String> {
        if key.code == KeyCode::Esc {
            self.is_open = false;
        } else if key.code == KeyCode::Tab || key.code == KeyCode::Down {
            self.active_field = (self.active_field + 1) % 11;
        } else if key.code == KeyCode::BackTab || key.code == KeyCode::Up {
            self.active_field = if self.active_field == 0 {
                10
            } else {
                self.active_field - 1
            };
        } else if self.active_field == 0
            && (key.code == KeyCode::Left || key.code == KeyCode::Char('h'))
        {
            let prev = if self.provider_index == 0 {
                4
            } else {
                self.provider_index - 1
            };
            self.set_provider(prev);
        } else if self.active_field == 0
            && (key.code == KeyCode::Right || key.code == KeyCode::Char('l'))
        {
            let next = (self.provider_index + 1) % 5;
            self.set_provider(next);
        } else if self.active_field >= 9
            && (key.code == KeyCode::Left || key.code == KeyCode::Right)
        {
            self.active_field = if self.active_field == 9 { 10 } else { 9 };
        } else if key.code == KeyCode::Char(' ') && self.active_field == 7 {
            self.use_ssl = !self.use_ssl;
        } else if key.code == KeyCode::Char(' ') && self.active_field == 8 {
            self.save_to_env = !self.save_to_env;
        } else if key.code == KeyCode::Enter {
            if self.active_field == 10 {
                self.is_open = false;
            } else if self.active_field == 7 {
                self.use_ssl = !self.use_ssl;
            } else if self.active_field == 8 {
                self.save_to_env = !self.save_to_env;
            } else {
                let safe_name = self.name.trim().replace("'", "''");
                let safe_key = self.key_id.trim().replace("'", "''");
                let safe_secret = self.secret_key.trim().replace("'", "''");
                let safe_region = self.region.trim().replace("'", "''");
                let safe_endpoint = self.endpoint.trim().replace("'", "''");

                if safe_name.is_empty() {
                    self.error_msg = Some("Secret name cannot be empty.".to_string());
                } else {
                    let _ = conn.execute("INSTALL httpfs; LOAD httpfs;", []);
                    let mut clauses = vec!["TYPE S3".to_string()];
                    if !safe_key.is_empty() {
                        clauses.push(format!("KEY_ID '{}'", safe_key));
                    }
                    if !safe_secret.is_empty() {
                        clauses.push(format!("SECRET '{}'", safe_secret));
                    }
                    if !safe_region.is_empty() {
                        clauses.push(format!("REGION '{}'", safe_region));
                    }
                    let mut use_ssl = self.use_ssl;
                    if !safe_endpoint.is_empty() {
                        let clean_ep = safe_endpoint
                            .trim_start_matches("http://")
                            .trim_start_matches("https://")
                            .trim_end_matches('/');
                        clauses.push(format!("ENDPOINT '{}'", clean_ep));
                        clauses.push("URL_STYLE 'path'".to_string());
                        if safe_endpoint.starts_with("http://")
                            || clean_ep.starts_with("localhost")
                            || clean_ep.starts_with("127.0.0.1")
                        {
                            use_ssl = false;
                        }
                    }
                    clauses.push(format!("USE_SSL {}", use_ssl));

                    let secret_body = clauses.join(", ");
                    let persistent_sql = format!(
                        "CREATE OR REPLACE PERSISTENT SECRET {} ({});",
                        safe_name, secret_body
                    );
                    let create_res = conn.execute(&persistent_sql, []).or_else(|_| {
                        let temp_sql =
                            format!("CREATE OR REPLACE SECRET {} ({});", safe_name, secret_body);
                        conn.execute(&temp_sql, [])
                    });

                    match create_res {
                        Ok(_) => {
                            if self.save_to_env {
                                let _ = crate::engine::save_cloud_secret_to_env(
                                    project_dir,
                                    self.key_id.trim(),
                                    self.secret_key.trim(),
                                    self.region.trim(),
                                    self.endpoint.trim(),
                                    self.bucket.trim(),
                                );
                            }

                            let mut bucket = self.bucket.trim().to_string();
                            if !bucket.is_empty() {
                                if !bucket.starts_with("s3://")
                                    && !bucket.starts_with("r2://")
                                    && !bucket.starts_with("gcs://")
                                {
                                    bucket = format!("s3://{}", bucket);
                                }
                                unsafe {
                                    std::env::set_var("DUCKPACK_S3_BUCKET", &bucket);
                                }
                            }

                            self.is_open = false;
                            return Some(format!(
                                "Configured cloud secret '{}' successfully.",
                                safe_name
                            ));
                        }
                        Err(e) => {
                            self.error_msg = Some(format!("Secret Error: {}", e));
                        }
                    }
                }
            }
        } else if key.code == KeyCode::Backspace {
            match self.active_field {
                1 => {
                    self.name.pop();
                }
                2 => {
                    self.key_id.pop();
                }
                3 => {
                    self.secret_key.pop();
                }
                4 => {
                    self.region.pop();
                }
                5 => {
                    self.endpoint.pop();
                }
                6 => {
                    self.bucket.pop();
                }
                _ => {}
            }
        } else if let KeyCode::Char(c) = key.code {
            if !key.modifiers.contains(KeyModifiers::CONTROL) {
                match self.active_field {
                    1 => {
                        self.name.push(c);
                    }
                    2 => {
                        self.key_id.push(c);
                    }
                    3 => {
                        self.secret_key.push(c);
                    }
                    4 => {
                        self.region.push(c);
                    }
                    5 => {
                        self.endpoint.push(c);
                    }
                    6 => {
                        self.bucket.push(c);
                    }
                    _ => {}
                }
            }
        }
        None
    }
}

/// Populates `SecretModalState` with existing secret configuration from DuckDB and `.env` for in-place editing.
pub(crate) fn populate_secret_modal_for_edit(
    conn: &duckdb::Connection,
    project_dir: &std::path::Path,
    secret_name: &str,
    modal: &mut SecretModalState,
) {
    modal.is_open = true;
    modal.name = secret_name.to_string();
    modal.active_field = 2; // Focus on Key ID by default
    modal.error_msg = None;

    // 1. Query DuckDB's duckdb_secrets() to extract existing config
    let sql = format!(
        "SELECT secret_string FROM duckdb_secrets() WHERE name = '{}';",
        secret_name.replace('\'', "''")
    );
    let mut secret_string_opt = None;
    if let Ok(mut stmt) = conn.prepare(&sql) {
        if let Ok(mut rows) = stmt.query([]) {
            if let Ok(Some(row)) = rows.next() {
                secret_string_opt = row.get::<_, String>(0).ok();
            }
        }
    }
    if secret_string_opt.is_none() {
        let fallback_sql = format!(
            "SELECT secret_string FROM duckdb_secrets WHERE name = '{}';",
            secret_name.replace('\'', "''")
        );
        if let Ok(mut stmt) = conn.prepare(&fallback_sql) {
            if let Ok(mut rows) = stmt.query([]) {
                if let Ok(Some(row)) = rows.next() {
                    secret_string_opt = row.get::<_, String>(0).ok();
                }
            }
        }
    }
    if let Some(secret_string) = secret_string_opt {
        for part in secret_string.split(';') {
            if let Some((k, v)) = part.split_once('=') {
                match k.trim() {
                    "key_id" => modal.key_id = v.trim().to_string(),
                    "region" => modal.region = v.trim().to_string(),
                    "endpoint" => modal.endpoint = v.trim().to_string(),
                    "use_ssl" => modal.use_ssl = v.trim().eq_ignore_ascii_case("true"),
                    _ => {}
                }
            }
        }
    }

    // 2. Read .env file for secrets / bucket if available
    let env_path = project_dir.join(".env");
    if let Ok(content) = std::fs::read_to_string(&env_path) {
        for line in content.lines() {
            if let Some((k, v)) = line.split_once('=') {
                let key = k.trim();
                let val = v.trim().trim_matches('"').trim_matches('\'');
                match key {
                    "AWS_ACCESS_KEY_ID" if modal.key_id.is_empty() => {
                        modal.key_id = val.to_string()
                    }
                    "AWS_SECRET_ACCESS_KEY" => modal.secret_key = val.to_string(),
                    "AWS_REGION" if modal.region.is_empty() => modal.region = val.to_string(),
                    "AWS_ENDPOINT" if modal.endpoint.is_empty() => modal.endpoint = val.to_string(),
                    "DUCKPACK_S3_BUCKET" if modal.bucket.is_empty() => {
                        modal.bucket = val.to_string()
                    }
                    _ => {}
                }
            }
        }
    }

    // 3. Infer provider index from endpoint
    let ep_lower = modal.endpoint.to_lowercase();
    if ep_lower.contains("r2.cloudflarestorage.com") {
        modal.provider_index = 1;
    } else if ep_lower.contains("localhost:9000")
        || ep_lower.contains("127.0.0.1:9000")
        || ep_lower.contains("minio")
        || ep_lower.contains("rustfs")
    {
        modal.provider_index = 2;
    } else if ep_lower.contains("wasabisys.com") {
        modal.provider_index = 3;
    } else if !modal.endpoint.is_empty() {
        modal.provider_index = 4;
    } else {
        modal.provider_index = 0;
    }
}

/// State for the Add Cloud Bucket modal dialog.
#[derive(Clone, Debug)]
pub(crate) struct AddBucketModalState {
    pub(crate) is_open: bool,
    pub(crate) initial_uri: Option<String>,
    pub(crate) uri: String,
    pub(crate) save_to_env: bool,
    pub(crate) active_field: usize, // 0: uri, 1: save_to_env, 2: submit, 3: cancel
    pub(crate) error_msg: Option<String>,
}

impl AddBucketModalState {
    pub(crate) fn new() -> Self {
        Self {
            is_open: false,
            initial_uri: None,
            uri: String::new(),
            save_to_env: true,
            active_field: 0,
            error_msg: None,
        }
    }

    pub(crate) fn reset(&mut self) {
        self.is_open = true;
        self.initial_uri = None;
        self.uri.clear();
        self.save_to_env = true;
        self.active_field = 0;
        self.error_msg = None;
    }

    pub(crate) fn render(&self, f: &mut Frame) {
        let popup_width = 64.min(f.area().width.saturating_sub(4));
        let popup_height = 14.min(f.area().height.saturating_sub(2));
        let popup_area = Rect {
            x: (f.area().width.saturating_sub(popup_width)) / 2,
            y: (f.area().height.saturating_sub(popup_height)) / 2,
            width: popup_width,
            height: popup_height,
        };
        f.render_widget(ratatui::widgets::Clear, popup_area);

        let modal_block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .title(" 🪣 Add S3 / Cloud Bucket ")
            .border_style(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD));
        f.render_widget(modal_block.clone(), popup_area);

        let inner_area = modal_block.inner(popup_area);
        let field_layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3), // 0: Bucket URI
                Constraint::Length(2), // 1: Persist in .env toggle
                Constraint::Length(2), // 2: Hint / error
                Constraint::Length(3), // 3: Buttons
            ])
            .split(inner_area);

        let f0_style = if self.active_field == 0 {
            Style::default().fg(Color::Yellow)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        let p0 = Paragraph::new(format!(
            "{}{}",
            self.uri,
            if self.active_field == 0 { "█" } else { "" }
        ))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .title(" Bucket URI or Name (e.g. s3://my-lake) ")
                .border_style(f0_style),
        );
        f.render_widget(p0, field_layout[0]);

        let f1_style = if self.active_field == 1 {
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White)
        };
        let env_label = format!(
            "  [{}] Persist to .env (Space to toggle)",
            if self.save_to_env { "X" } else { " " }
        );
        f.render_widget(Paragraph::new(env_label).style(f1_style), field_layout[1]);

        if let Some(ref err) = self.error_msg {
            f.render_widget(
                Paragraph::new(format!(" ⚠️ {}", err))
                    .style(Style::default().fg(Color::LightRed))
                    .wrap(Wrap { trim: true }),
                field_layout[2],
            );
        } else {
            f.render_widget(
                Paragraph::new(
                    "  Tab: Next Field | Space: Toggle | Enter: Add Bucket | Esc: Cancel",
                )
                .style(Style::default().fg(Color::DarkGray))
                .wrap(Wrap { trim: true }),
                field_layout[2],
            );
        }

        let add_btn_style = if self.active_field == 2 {
            Style::default()
                .bg(Color::Yellow)
                .fg(Color::Black)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Yellow)
        };
        let cancel_btn_style = if self.active_field == 3 {
            Style::default()
                .bg(Color::Red)
                .fg(Color::White)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        let buttons = Line::from(vec![
            Span::styled(" [ Add Bucket ] ", add_btn_style),
            Span::raw("    "),
            Span::styled(" [ Cancel ] ", cancel_btn_style),
        ]);
        f.render_widget(
            Paragraph::new(buttons).alignment(Alignment::Center),
            field_layout[3],
        );
    }

    /// Processes a keyboard event in the Add Bucket modal.
    /// Returns `Some(status_msg)` if a bucket was added and the explorer should refresh.
    pub(crate) fn handle_key(
        &mut self,
        key: KeyEvent,
        project_dir: &Path,
        expanded_groups: &mut HashSet<String>,
    ) -> Option<String> {
        if key.code == KeyCode::Esc {
            self.is_open = false;
        } else if key.code == KeyCode::Tab || key.code == KeyCode::Down {
            self.active_field = (self.active_field + 1) % 4;
        } else if key.code == KeyCode::BackTab || key.code == KeyCode::Up {
            self.active_field = if self.active_field == 0 {
                3
            } else {
                self.active_field - 1
            };
        } else if self.active_field >= 2
            && (key.code == KeyCode::Left || key.code == KeyCode::Right)
        {
            self.active_field = if self.active_field == 2 { 3 } else { 2 };
        } else if key.code == KeyCode::Char(' ') && self.active_field == 1 {
            self.save_to_env = !self.save_to_env;
        } else if key.code == KeyCode::Enter {
            if self.active_field == 3 {
                self.is_open = false;
            } else if self.active_field == 1 {
                self.save_to_env = !self.save_to_env;
            } else {
                let clean_uri = self.uri.trim();
                if clean_uri.is_empty() {
                    self.error_msg = Some("Bucket URI or name cannot be empty.".to_string());
                } else {
                    let mut normalized = clean_uri.to_string();
                    if !normalized.starts_with("s3://")
                        && !normalized.starts_with("r2://")
                        && !normalized.starts_with("gcs://")
                        && !normalized.starts_with("azure://")
                    {
                        normalized = format!("s3://{}", normalized);
                    }

                    if let Some(ref old_uri) = self.initial_uri {
                        if old_uri != &normalized {
                            expanded_groups.remove(&format!("bucket:{}", old_uri));
                            unsafe {
                                let current = std::env::var("DUCKPACK_S3_BUCKETS")
                                    .or_else(|_| std::env::var("DUCKPACK_S3_BUCKET"))
                                    .unwrap_or_default();
                                let mut new_parts: Vec<&str> = current
                                    .split(',')
                                    .map(|s| s.trim())
                                    .filter(|s| {
                                        !s.is_empty()
                                            && *s != old_uri.as_str()
                                            && *s != old_uri.trim_start_matches("s3://")
                                    })
                                    .collect();
                                if !new_parts.contains(&normalized.as_str()) {
                                    new_parts.push(&normalized);
                                }
                                let joined = new_parts.join(",");
                                std::env::set_var("DUCKPACK_S3_BUCKETS", &joined);
                                std::env::set_var("DUCKPACK_S3_BUCKET", &joined);
                            }
                        }
                    }

                    if self.save_to_env {
                        let _ = crate::engine::append_cloud_bucket_to_env(project_dir, &normalized);
                    }

                    unsafe {
                        let current = std::env::var("DUCKPACK_S3_BUCKETS")
                            .or_else(|_| std::env::var("DUCKPACK_S3_BUCKET"))
                            .unwrap_or_default();
                        let new_var = if current.is_empty() {
                            normalized.clone()
                        } else if !current.split(',').any(|b| b.trim() == normalized) {
                            format!("{},{}", current, normalized)
                        } else {
                            current
                        };
                        std::env::set_var("DUCKPACK_S3_BUCKETS", &new_var);
                        std::env::set_var("DUCKPACK_S3_BUCKET", &new_var);
                    }

                    let bucket_key = format!("bucket:{}", normalized);
                    expanded_groups.insert("Secrets & Cloud Buckets".to_string());
                    expanded_groups.insert(bucket_key);

                    self.is_open = false;
                    return Some(format!("Added bucket '{}' to explorer.", normalized));
                }
            }
        } else if key.code == KeyCode::Backspace && self.active_field == 0 {
            self.uri.pop();
        } else if let KeyCode::Char(c) = key.code {
            if !key.modifiers.contains(KeyModifiers::CONTROL) && self.active_field == 0 {
                self.uri.push(c);
            }
        }
        None
    }
}

/// State for the Database Picker modal dialog (F2).
#[derive(Clone, Debug)]
pub(crate) struct DatabasePickerModalState {
    pub(crate) is_open: bool,
    pub(crate) selected_index: usize,
}

impl DatabasePickerModalState {
    pub(crate) fn new() -> Self {
        Self {
            is_open: false,
            selected_index: 0,
        }
    }

    pub(crate) fn open_for_current(&mut self, current_db: &str, databases: &[DatabaseInfo]) {
        self.is_open = true;
        if let Some(pos) = databases.iter().position(|d| d.name == current_db) {
            self.selected_index = pos;
        } else {
            self.selected_index = 0;
        }
    }

    pub(crate) fn next(&mut self, total: usize) {
        if total > 0 {
            self.selected_index = (self.selected_index + 1).min(total.saturating_sub(1));
        }
    }

    pub(crate) fn prev(&mut self) {
        self.selected_index = self.selected_index.saturating_sub(1);
    }

    pub(crate) fn render(&self, f: &mut Frame, databases: &[DatabaseInfo], active_tab_db: &str) {
        let popup_width = 58.min(f.area().width.saturating_sub(4));
        let item_count = databases.len().max(1);
        let popup_height = (item_count + 6).min(f.area().height.saturating_sub(2) as usize) as u16;
        let popup_area = Rect {
            x: (f.area().width.saturating_sub(popup_width)) / 2,
            y: (f.area().height.saturating_sub(popup_height)) / 2,
            width: popup_width,
            height: popup_height,
        };
        f.render_widget(ratatui::widgets::Clear, popup_area);

        let modal_block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .title(" 🗄️ Switch Attached Database (F2) ")
            .border_style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD));
        f.render_widget(modal_block.clone(), popup_area);

        let inner = modal_block.inner(popup_area);
        let picker_layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1), // Subtitle / hint
                Constraint::Min(1),    // List of databases
                Constraint::Length(1), // Controls hint
            ])
            .split(inner);

        let sub_hint = Paragraph::new(" Choose target database for current editor tab:")
            .style(Style::default().fg(Color::DarkGray));
        f.render_widget(sub_hint, picker_layout[0]);

        let list_items: Vec<ListItem> = databases
            .iter()
            .enumerate()
            .map(|(idx, db)| {
                let is_selected = idx == self.selected_index;
                let is_current = db.name == active_tab_db;
                let icon = db_icon(&db.db_type);
                let current_marker = if is_current { " [Active for Tab]" } else { "" };
                let type_tag = format!("({})", db.db_type);
                let cursor = if is_selected { "▶ " } else { "  " };

                let content = format!(
                    "{}{} {} {} {}",
                    cursor, icon, db.name, type_tag, current_marker
                );
                let style = if is_selected {
                    Style::default()
                        .bg(Color::DarkGray)
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::BOLD)
                } else if is_current {
                    Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::White)
                };
                ListItem::new(content).style(style)
            })
            .collect();

        let list_widget = List::new(list_items);
        f.render_widget(list_widget, picker_layout[1]);

        let footer_hint =
            Paragraph::new(" ▲/▼ or j/k: Navigate | Enter: Select | Esc: Cancel")
                .alignment(Alignment::Center)
                .style(Style::default().fg(Color::DarkGray));
        f.render_widget(footer_hint, picker_layout[2]);
    }

    /// Processes a keyboard event in the Database Picker modal.
    /// Returns `Some(selected_db_name)` if a database was chosen.
    pub(crate) fn handle_key(
        &mut self,
        key: KeyEvent,
        meta: &mut ExplorerMetadata,
        conn: &Connection,
    ) -> Option<String> {
        if key.code == KeyCode::Esc {
            self.is_open = false;
        } else if key.code == KeyCode::Up || key.code == KeyCode::Char('k') {
            self.prev();
        } else if key.code == KeyCode::Down || key.code == KeyCode::Char('j') {
            self.next(meta.databases.len());
        } else if key.code == KeyCode::Enter {
            let chosen = meta.databases.get(self.selected_index).map(|d| d.name.clone());
            self.is_open = false;
            if let Some(db_name) = chosen {
                let _ = conn.execute(&format!("USE \"{}\";", db_name.replace('"', "\"\"")), []);
                meta.active_database = db_name.clone();
                for db in &mut meta.databases {
                    db.is_active = db.name == db_name;
                }
                return Some(db_name);
            }
        }
        None
    }
}

/// State for the Exit Confirmation modal dialog (Esc).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ExitConfirmationModalState {
    pub(crate) is_open: bool,
    pub(crate) confirm_exit: bool,
}

impl ExitConfirmationModalState {
    pub(crate) fn new() -> Self {
        Self {
            is_open: false,
            confirm_exit: false,
        }
    }

    pub(crate) fn open(&mut self) {
        self.is_open = true;
        self.confirm_exit = false; // default to safe cancel
    }

    pub(crate) fn close(&mut self) {
        self.is_open = false;
        self.confirm_exit = false;
    }

    pub(crate) fn toggle(&mut self) {
        self.confirm_exit = !self.confirm_exit;
    }

    pub(crate) fn render(&self, f: &mut Frame, dirty_tab_count: usize) -> Rect {
        let warning_line = if dirty_tab_count == 0 {
            None
        } else if dirty_tab_count == 1 {
            Some("⚠️ 1 tab has unsaved changes!".to_string())
        } else {
            Some(format!("⚠️ {} tabs have unsaved changes!", dirty_tab_count))
        };
        self.render_with_warning(
            f,
            " ⚠️ Exit DuckPack IDE ",
            "Are you sure you want to exit DuckPack?",
            warning_line.as_deref(),
        )
    }

    pub(crate) fn render_with_warning(
        &self,
        f: &mut Frame,
        title: &str,
        question_str: &str,
        warning_line: Option<&str>,
    ) -> Rect {
        let has_warning = warning_line.is_some();
        let popup_width = 58.min(f.area().width.saturating_sub(4));
        let popup_height = if has_warning { 9 } else { 7 };
        let popup_area = Rect {
            x: (f.area().width.saturating_sub(popup_width)) / 2,
            y: (f.area().height.saturating_sub(popup_height)) / 2,
            width: popup_width,
            height: popup_height,
        };
        f.render_widget(ratatui::widgets::Clear, popup_area);

        let modal_block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .title(title)
            .border_style(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD));
        f.render_widget(modal_block.clone(), popup_area);

        let inner = modal_block.inner(popup_area);
        let exit_layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints(if has_warning {
                vec![
                    Constraint::Length(1), // Question
                    Constraint::Length(1), // Warning
                    Constraint::Min(1),    // Spacer
                    Constraint::Length(1), // Buttons
                ]
            } else {
                vec![
                    Constraint::Length(1), // Question
                    Constraint::Min(1),    // Spacer
                    Constraint::Length(1), // Buttons
                ]
            })
            .split(inner);

        let question = Paragraph::new(question_str)
            .alignment(Alignment::Center)
            .style(Style::default().fg(Color::White).add_modifier(Modifier::BOLD));
        f.render_widget(question, exit_layout[0]);

        if let Some(warning_text) = warning_line {
            let warning = Paragraph::new(warning_text)
                .alignment(Alignment::Center)
                .style(Style::default().fg(Color::LightRed).add_modifier(Modifier::BOLD));
            f.render_widget(warning, exit_layout[1]);
        }

        let btn_idx = if has_warning { 3 } else { 2 };
        let yes_style = if self.confirm_exit {
            Style::default()
                .bg(Color::Red)
                .fg(Color::White)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White)
        };
        let cancel_style = if !self.confirm_exit {
            Style::default()
                .bg(Color::Cyan)
                .fg(Color::Black)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White)
        };

        let buttons = Line::from(vec![
            Span::styled(" [y] Yes, Exit ", yes_style),
            Span::raw("    "),
            Span::styled(" [n / Esc] Cancel ", cancel_style),
        ]);
        f.render_widget(
            Paragraph::new(buttons).alignment(Alignment::Center),
            exit_layout[btn_idx],
        );

        popup_area
    }

    /// Processes a keyboard event in the Exit Confirmation modal.
    /// Returns `Some(true)` if the user confirmed exit, `Some(false)` if cancelled, `None` if navigating.
    pub(crate) fn handle_key(&mut self, key: KeyEvent) -> Option<bool> {
        if key.code == KeyCode::Char('y') || key.code == KeyCode::Char('Y') {
            Some(true)
        } else if key.code == KeyCode::Char('n')
            || key.code == KeyCode::Char('N')
            || key.code == KeyCode::Esc
        {
            self.close();
            Some(false)
        } else if key.code == KeyCode::Left
            || key.code == KeyCode::Right
            || key.code == KeyCode::Tab
            || key.code == KeyCode::BackTab
            || key.code == KeyCode::Char('h')
            || key.code == KeyCode::Char('l')
        {
            self.toggle();
            None
        } else if key.code == KeyCode::Enter {
            if self.confirm_exit {
                Some(true)
            } else {
                self.close();
                Some(false)
            }
        } else {
            None
        }
    }
}

/// Renders the Help overlay popup.
pub(crate) fn render_help_modal(f: &mut Frame) {
    let popup_area = Rect {
        x: f.area().width / 6,
        y: f.area().height / 6,
        width: f.area().width * 2 / 3,
        height: (f.area().height * 2 / 3).max(18),
    };
    f.render_widget(ratatui::widgets::Clear, popup_area);

    let help_lines = vec![
        Line::from(vec![
            Span::styled(" [?]       ", Style::default().fg(Color::Cyan)),
            Span::raw("Toggle Help Menu"),
        ]),
        Line::from(vec![
            Span::styled(" F2        ", Style::default().fg(Color::Cyan)),
            Span::raw("Switch Attached Database for Active Tab"),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled(" F3 / 1    ", Style::default().fg(Color::Yellow)),
            Span::raw("Switch Explorer to Database Catalog"),
        ]),
        Line::from(vec![
            Span::styled(" F4 / 2    ", Style::default().fg(Color::Yellow)),
            Span::raw("Switch Explorer to Project Files"),
        ]),
        Line::from(vec![
            Span::styled(" a / n     ", Style::default().fg(Color::Yellow)),
            Span::raw("New Project Asset (Table, View, Macro, Schema, Query)"),
        ]),
        Line::from(vec![
            Span::styled(" Ctrl+B / v", Style::default().fg(Color::Yellow)),
            Span::raw("Validate Project Syntax & Dependencies (Shadow DB)"),
        ]),
        Line::from(vec![
            Span::styled(" F6/Ctrl+D ", Style::default().fg(Color::Yellow)),
            Span::raw("In-IDE Declarative Diff & Apply to Target DB"),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled(" Ctrl+T    ", Style::default().fg(Color::Cyan)),
            Span::raw("New Tab"),
        ]),
        Line::from(vec![
            Span::styled(" Ctrl+W    ", Style::default().fg(Color::Cyan)),
            Span::raw("Close Tab"),
        ]),
        Line::from(vec![
            Span::styled(" Ctrl+N/P  ", Style::default().fg(Color::Cyan)),
            Span::raw("Next/Prev Tab"),
        ]),
        Line::from(vec![
            Span::styled(" Ctrl+E/F5 ", Style::default().fg(Color::Cyan)),
            Span::raw("Execute Query (or selected text)"),
        ]),
        Line::from(vec![
            Span::styled(" Ctrl+G    ", Style::default().fg(Color::Cyan)),
            Span::raw("Visualize Schema (Graph)"),
        ]),
        Line::from(vec![
            Span::styled(" Ctrl+A    ", Style::default().fg(Color::Cyan)),
            Span::raw("Attach Database (DuckDB, SQLite, PG, etc.)"),
        ]),
        Line::from(vec![
            Span::styled(" Ctrl+K    ", Style::default().fg(Color::Cyan)),
            Span::raw("Cloud Secrets & S3 Buckets (AWS, R2, MinIO)"),
        ]),
        Line::from(vec![
            Span::styled(" Ctrl+C/V  ", Style::default().fg(Color::Cyan)),
            Span::raw("Copy / Paste"),
        ]),
        Line::from(vec![
            Span::styled(" Ctrl+S    ", Style::default().fg(Color::Cyan)),
            Span::raw("Save Query / Project File"),
        ]),
        Line::from(vec![
            Span::styled(" Ctrl+F    ", Style::default().fg(Color::Cyan)),
            Span::raw("Format Query"),
        ]),
        Line::from(vec![
            Span::styled(" Ctrl+Space", Style::default().fg(Color::Cyan)),
            Span::raw("Autocomplete"),
        ]),
        Line::from(vec![
            Span::styled(" n / p / l ", Style::default().fg(Color::Cyan)),
            Span::raw("Next/Prev Page, 'l' to cycle Page Limit (500/2k/10k/50k/All)"),
        ]),
        Line::from(vec![
            Span::styled(" r / R     ", Style::default().fg(Color::Cyan)),
            Span::raw("Refresh Explorer (when in Explorer pane)"),
        ]),
        Line::from(vec![
            Span::styled(" W / Alt+W ", Style::default().fg(Color::Cyan)),
            Span::raw("Cycle Explorer Width (25% -> 35% -> 50%)"),
        ]),
        Line::from(vec![
            Span::styled(" +/- or [/]", Style::default().fg(Color::Cyan)),
            Span::raw("Resize Explorer Width (in 5% increments)"),
        ]),
        Line::from(vec![
            Span::styled(" ◄ / ► / h/l", Style::default().fg(Color::Cyan)),
            Span::raw("Scroll Explorer horizontally (Shift: 10 chars, Home: reset)"),
        ]),
        Line::from(vec![
            Span::styled(" q / s     ", Style::default().fg(Color::Cyan)),
            Span::raw("Query Table (in Explorer pane)"),
        ]),
        Line::from(vec![
            Span::styled(" e         ", Style::default().fg(Color::Cyan)),
            Span::raw("Edit Secret / Edit Bucket (in Explorer pane)"),
        ]),
        Line::from(vec![
            Span::styled(" a / n     ", Style::default().fg(Color::Cyan)),
            Span::raw("Add Asset / Secret / Bucket (in Explorer pane)"),
        ]),
        Line::from(vec![
            Span::styled(" d / Del   ", Style::default().fg(Color::Cyan)),
            Span::raw("Detach DB / Drop Secret / Remove Bucket (Explorer)"),
        ]),
        Line::from(vec![
            Span::styled(" ◄ / ►     ", Style::default().fg(Color::Cyan)),
            Span::raw("Scroll Columns (in Results pane, Shift+◄/► fast)"),
        ]),
        Line::from(vec![
            Span::styled(" Home/End  ", Style::default().fg(Color::Cyan)),
            Span::raw("First/Last Column (in Results pane)"),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled(" -- @remote", Style::default().fg(Color::Yellow)),
            Span::raw("Add at top of query to execute on remote Quack server (e.g. CREATE SECRET)"),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled(" Mouse     ", Style::default().fg(Color::Cyan)),
            Span::raw("Click tabs/buttons, scroll panes"),
        ]),
    ];

    let help_paragraph = Paragraph::new(help_lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Help Menu ")
                .border_style(Style::default().fg(Color::Cyan)),
        )
        .alignment(Alignment::Left);
    f.render_widget(help_paragraph, popup_area);
}

/// Renders the Schema Visualizer popup and returns `max_schema_scroll`.
pub(crate) fn render_schema_visualizer_modal(
    f: &mut Frame,
    schema_ascii_tree: &str,
    schema_scroll: u16,
) -> u16 {
    let popup_area = Rect {
        x: f.area().width / 8,
        y: f.area().height / 8,
        width: (f.area().width * 3) / 4,
        height: (f.area().height * 3) / 4,
    };
    f.render_widget(ratatui::widgets::Clear, popup_area);

    let lines: Vec<Line> = schema_ascii_tree
        .lines()
        .map(|l| Line::from(l.to_string()))
        .collect();
    let max_schema_scroll =
        lines.len().saturating_sub((popup_area.height as usize).saturating_sub(2)) as u16;

    let tree_paragraph = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!(
                    " Schema Visualizer (Scroll: {}/{}) ",
                    schema_scroll, max_schema_scroll
                ))
                .border_style(Style::default().fg(Color::Cyan)),
        )
        .alignment(Alignment::Left)
        .scroll((schema_scroll, 0));
    f.render_widget(tree_paragraph, popup_area);

    max_schema_scroll
}

