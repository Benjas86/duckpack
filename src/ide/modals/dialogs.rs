use duckdb::Connection;
use ratatui::{
    crossterm::event::{KeyCode, KeyEvent},
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, List, ListItem, Paragraph},
    Frame,
};

use crate::catalog::{DatabaseInfo, ExplorerMetadata};
use crate::ide::explorer::db_icon;

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

