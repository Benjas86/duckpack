use duckdb::Connection;
use ratatui::{
    crossterm::event::{KeyCode, KeyEvent, KeyModifiers},
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph, Wrap},
    Frame,
};
use std::path::{Path, PathBuf};

use crate::ide::explorer::{
    apply_ide_diff_for_catalog, generate_asset_boilerplate, get_asset_file_path, ExplorerNode,
    NodeType, ProjectCategory,
};
use super::input::{
    delete_char_at_cursor, delete_char_before_cursor, insert_char_at_cursor,
    render_text_with_cursor,
};

/// State for the New Asset modal dialog (`a` or `n` in Project view).
#[derive(Clone, Debug)]
pub(crate) struct NewAssetModalState {
    pub(crate) is_open: bool,
    pub(crate) category_index: usize, // 0: Table, 1: View, 2: Macro, 3: Trigger, 4: Sequence, 5: Schema, 6: Query, 7: Seed
    pub(crate) name: String,
    pub(crate) cursor_pos: usize,
    pub(crate) active_field: usize, // 0: category, 1: name, 2: create, 3: cancel
    pub(crate) error_msg: Option<String>,
}

impl NewAssetModalState {
    pub(crate) fn new() -> Self {
        Self {
            is_open: false,
            category_index: 0,
            name: String::new(),
            cursor_pos: 0,
            active_field: 1,
            error_msg: None,
        }
    }

    pub(crate) fn reset(&mut self) {
        self.is_open = true;
        self.category_index = 0;
        self.name.clear();
        self.cursor_pos = 0;
        self.active_field = 1;
        self.error_msg = None;
    }

    pub(crate) fn set_category(&mut self, category: ProjectCategory) {
        match category {
            ProjectCategory::Table => self.category_index = 0,
            ProjectCategory::View => self.category_index = 1,
            ProjectCategory::Macro => self.category_index = 2,
            ProjectCategory::Trigger => self.category_index = 3,
            ProjectCategory::Sequence => self.category_index = 4,
            ProjectCategory::Schema => self.category_index = 5,
            ProjectCategory::Query => self.category_index = 6,
            ProjectCategory::Seed => self.category_index = 7,
            ProjectCategory::Config => {}
        }
    }

    pub(crate) fn open_for_group_name(&mut self, group_name: &str) {
        self.reset();
        if group_name.contains("tables") {
            self.set_category(ProjectCategory::Table);
        } else if group_name.contains("views") {
            self.set_category(ProjectCategory::View);
        } else if group_name.contains("macros") {
            self.set_category(ProjectCategory::Macro);
        } else if group_name.contains("triggers") {
            self.set_category(ProjectCategory::Trigger);
        } else if group_name.contains("sequences") {
            self.set_category(ProjectCategory::Sequence);
        } else if group_name.contains("schemas") {
            self.set_category(ProjectCategory::Schema);
        } else if group_name.contains("queries") {
            self.set_category(ProjectCategory::Query);
        } else if group_name.contains("seeds") {
            self.set_category(ProjectCategory::Seed);
        }
    }

    pub(crate) fn open_for_node(&mut self, node: Option<&ExplorerNode>) {
        self.reset();
        if let Some(n) = node {
            match &n.node_type {
                NodeType::Group => self.open_for_group_name(&n.name),
                NodeType::ProjectFile { category, .. } => self.set_category(*category),
                NodeType::Table { .. } => self.set_category(ProjectCategory::Table),
                NodeType::View { .. } => self.set_category(ProjectCategory::View),
                NodeType::Macro => self.set_category(ProjectCategory::Macro),
                NodeType::Trigger { .. } => self.set_category(ProjectCategory::Trigger),
                NodeType::Query => self.set_category(ProjectCategory::Query),
                _ => {}
            }
        }
    }

    pub(crate) fn current_category(&self) -> ProjectCategory {
        match self.category_index {
            0 => ProjectCategory::Table,
            1 => ProjectCategory::View,
            2 => ProjectCategory::Macro,
            3 => ProjectCategory::Trigger,
            4 => ProjectCategory::Sequence,
            5 => ProjectCategory::Schema,
            6 => ProjectCategory::Query,
            _ => ProjectCategory::Seed,
        }
    }

    pub(crate) fn category_label(&self) -> &'static str {
        match self.category_index {
            0 => "Table",
            1 => "View",
            2 => "Macro",
            3 => "Trigger",
            4 => "Sequence",
            5 => "Schema",
            6 => "Query",
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
        let p1 = Paragraph::new(render_text_with_cursor(
            &self.name,
            self.cursor_pos,
            self.active_field == 1,
            false,
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
            if self.active_field == 1 {
                self.cursor_pos = self.name.chars().count();
            }
        } else if key.code == KeyCode::BackTab || key.code == KeyCode::Up {
            self.active_field = if self.active_field == 0 {
                3
            } else {
                self.active_field - 1
            };
            if self.active_field == 1 {
                self.cursor_pos = self.name.chars().count();
            }
        } else if self.active_field == 0
            && (key.code == KeyCode::Left || key.code == KeyCode::Char('h'))
        {
            self.category_index = if self.category_index == 0 {
                7
            } else {
                self.category_index - 1
            };
        } else if self.active_field == 0
            && (key.code == KeyCode::Right || key.code == KeyCode::Char('l'))
        {
            self.category_index = (self.category_index + 1) % 8;
        } else if self.active_field >= 2
            && (key.code == KeyCode::Left || key.code == KeyCode::Right)
        {
            self.active_field = if self.active_field == 2 { 3 } else { 2 };
        } else if self.active_field == 1 && key.code == KeyCode::Left {
            self.cursor_pos = self.cursor_pos.saturating_sub(1);
        } else if self.active_field == 1 && key.code == KeyCode::Right {
            self.cursor_pos = (self.cursor_pos + 1).min(self.name.chars().count());
        } else if self.active_field == 1 && key.code == KeyCode::Home {
            self.cursor_pos = 0;
        } else if self.active_field == 1 && key.code == KeyCode::End {
            self.cursor_pos = self.name.chars().count();
        } else if self.active_field == 1 && key.code == KeyCode::Backspace {
            delete_char_before_cursor(&mut self.name, &mut self.cursor_pos);
        } else if self.active_field == 1 && key.code == KeyCode::Delete {
            delete_char_at_cursor(&mut self.name, self.cursor_pos);
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
                    insert_char_at_cursor(&mut self.name, &mut self.cursor_pos, c);
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
