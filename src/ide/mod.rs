pub mod editor;
pub mod explorer;
pub mod modals;

pub use crate::catalog::*;
pub use crate::query::*;
pub(crate) use editor::{
    EditorTab, FocusPane, build_paginated_sql, format_duckdb_value, generate_schema_visualizer,
};
#[cfg(test)]
pub(crate) use editor::extract_explicit_sql_limit;
pub use editor::{
    calculate_status_height, get_clipboard_text, handle_sql_error_position, set_clipboard_text,
};
pub(crate) use explorer::{
    ExplorerMode, NodeType, ProjectCategory, compute_ide_diff_for_catalog, db_icon,
    get_git_branch_and_status, rebuild_explorer_nodes, rebuild_project_nodes,
    render_explorer_node_line,
};
#[cfg(test)]
pub(crate) use explorer::{
    ExplorerNode, apply_ide_diff, column_data_type_style, compute_ide_diff, format_column_display,
    generate_asset_boilerplate, get_asset_file_path,
};
pub(crate) use modals::{
    AddBucketModalState, AttachModalState, DatabasePickerModalState, ExitConfirmationModalState,
    InIdeDiffModalState, NewAssetModalState, SecretModalState, populate_secret_modal_for_edit,
    render_help_modal, render_schema_visualizer_modal,
};
#[cfg(test)]
pub(crate) use modals::{AttachProvider, CloudProvider};

use anyhow::Result;
use duckdb::Connection;
use ratatui::{
    Terminal,
    crossterm::event::{self, Event, KeyCode, KeyModifiers, MouseButton, MouseEventKind},
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block, BorderType, Borders, List, ListItem, ListState, Padding, Paragraph, Row, Table,
        Tabs, Wrap,
    },
};
use std::collections::HashSet;
use std::path::PathBuf;
#[cfg(test)]
use tui_textarea::TextArea;

/// Helper to open or focus a file path in the tab bar, reusing an empty untitled tab if available.
fn open_or_focus_file_in_tabs(
    tabs: &mut Vec<EditorTab>,
    active_tab_index: &mut usize,
    path: PathBuf,
) -> bool {
    if let Some(i) = tabs
        .iter()
        .position(|tab| tab.active_file.as_ref() == Some(&path))
    {
        *active_tab_index = i;
        return true;
    }
    if let Ok(content) = std::fs::read_to_string(&path) {
        if !tabs[*active_tab_index].is_untitled_empty() {
            let db = tabs[*active_tab_index].target_database.clone();
            tabs.push(EditorTab::new(db));
            *active_tab_index = tabs.len() - 1;
        }
        tabs[*active_tab_index].load_content(&content, Some(path));
        return true;
    }
    false
}

/// Helper to load an ad-hoc query into the active tab (or a new tab if the current tab is non-empty).
fn open_query_in_tabs(
    tabs: &mut Vec<EditorTab>,
    active_tab_index: &mut usize,
    query: String,
    target_db: Option<String>,
) {
    let db = target_db.unwrap_or_else(|| tabs[*active_tab_index].target_database.clone());
    if !tabs[*active_tab_index].is_untitled_empty() {
        tabs.push(EditorTab::new(db));
        *active_tab_index = tabs.len() - 1;
    } else {
        tabs[*active_tab_index].target_database = db;
    }
    tabs[*active_tab_index].load_content(&query, None);
}

/// The main execution loop for the IDE/Explorer feature.
/// Handles rendering the UI layout, capturing keyboard/mouse events, managing tabs,
/// and securely executing raw DuckDB queries from the active text buffer.
pub fn run_ide_loop(
    terminal: &mut Terminal<ratatui::backend::CrosstermBackend<std::io::Stdout>>,
    conn: &Connection,
    project_dir: &PathBuf,
    db_path: &str,
    quack_disable_ssl: bool,
) -> Result<()> {
    let is_quack = db_path.starts_with("quack:");
    let is_remote_db = is_quack || db_path.starts_with("md:") || db_path.starts_with("motherduck:");

    let local_duckdb_version: String = conn
        .query_row("SELECT version();", [], |r| r.get(0))
        .unwrap_or_else(|_| "unknown".to_string());

    let remote_duckdb_version: Option<String> = if is_quack {
        conn.query_row(
            "SELECT * FROM target_db.query('SELECT version()');",
            [],
            |r| r.get(0),
        )
        .ok()
    } else if is_remote_db {
        conn.query_row("SELECT version();", [], |r| r.get(0)).ok()
    } else {
        None
    };

    // Eagerly ensure fundamental cloud and analytical extensions (httpfs, parquet) are loaded
    let _ = crate::engine::ensure_extension_loaded(conn, "httpfs");
    let _ = crate::engine::ensure_extension_loaded(conn, "parquet");

    let mut meta = load_explorer_metadata(conn, project_dir, is_quack);
    let mut tabs: Vec<EditorTab> = vec![EditorTab::new(meta.active_database.clone())];
    let mut active_tab_index: usize = 0;
    let mut db_picker_modal = DatabasePickerModalState::new();
    let mut exit_confirm_modal = ExitConfirmationModalState::new();

    let mut focus = FocusPane::Editor;
    let mut status_msg = String::from("Ready. Press 'Tab' to switch panes. 'Esc' to exit.");

    let mut expanded_groups: HashSet<String> = if meta.databases.len() > 1 {
        let mut set = HashSet::new();
        for db in &meta.databases {
            set.insert(format!("db:{}", db.name));
            set.insert(format!("{}:Tables", db.name));
        }
        set.insert("Macros".to_string());
        set.insert("Queries".to_string());
        set
    } else {
        vec![
            "Tables",
            "Views",
            "Macros",
            "Queries",
            "Secrets & Cloud Buckets",
        ]
        .into_iter()
        .map(|s| s.to_string())
        .collect()
    };
    let mut expanded_tables: HashSet<String> = HashSet::new();
    let mut attach_modal = AttachModalState::new();
    let mut secret_modal = SecretModalState::new();
    let mut add_bucket_modal = AddBucketModalState::new();

    let mut dictionary = meta.dictionary.clone();
    let mut autocomplete_matches: Vec<String> = Vec::new();
    let mut autocomplete_index = 0;
    let mut autocomplete_original_word = String::new();

    let queries_dir = project_dir.join("queries");

    let mut explorer_mode = ExplorerMode::Database;
    let (mut git_branch, mut git_status) = get_git_branch_and_status(project_dir);
    let mut expanded_project_groups: HashSet<String> = vec![
        "proj:schemas",
        "proj:tables",
        "proj:views",
        "proj:macros",
        "proj:triggers",
        "proj:queries",
        "proj:seeds",
    ]
    .into_iter()
    .map(|s| s.to_string())
    .collect();

    let mut new_asset_modal = NewAssetModalState::new();
    let mut in_ide_diff_modal = InIdeDiffModalState::new();

    let mut explorer_state = ListState::default();
    let mut explorer_horizontal_scroll: usize = 0;
    let mut explorer_width_percent: u16 = 25;
    let mut explorer_items =
        rebuild_explorer_nodes(&meta, &expanded_groups, &expanded_tables, conn, is_quack);

    if !explorer_items.is_empty() {
        explorer_state.select(Some(0));
    }

    let mut current_editor_rect = Rect::default();
    let mut current_explorer_rect = Rect::default();
    let mut current_tabs_rect = Rect::default();
    let mut current_results_rect = Rect::default();
    let mut current_status_rect = Rect::default();
    let mut current_exit_dialog_rect = Rect::default();
    let mut show_help = false;
    let mut show_schema_visualizer = false;
    let mut schema_ascii_tree = String::new();
    let mut schema_scroll = 0;
    let mut max_schema_scroll = 0;
    let mut os_clipboard: Option<arboard::Clipboard> = arboard::Clipboard::new().ok();

    loop {
        let current_target = &tabs[active_tab_index].target_database;
        if !current_target.is_empty() && meta.active_database != *current_target {
            let _ = conn.execute(
                &format!("USE \"{}\";", current_target.replace('"', "\"\"")),
                [],
            );
            meta.active_database = current_target.clone();
            for db in &mut meta.databases {
                db.is_active = db.name == *current_target;
            }
        }

        terminal.draw(|f| {
            let main_chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Length(3), Constraint::Min(0)].as_ref())
                .split(f.area());

            let ssh_prefix = if std::env::var("SSH_CLIENT").is_ok() {
                "[SSH Remote] "
            } else {
                ""
            };
            let version_badge = if let Some(ref r_ver) = remote_duckdb_version {
                format!(" (Remote: {} | Local: {})", r_ver, local_duckdb_version)
            } else {
                format!(" ({})", local_duckdb_version)
            };
            let top_header = Paragraph::new("").block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .title_top(
                        Line::from(format!(
                            " Target DB: {}{}{} ",
                            ssh_prefix, db_path, version_badge
                        ))
                        .alignment(Alignment::Right)
                        .style(Style::default().fg(Color::Cyan)),
                    )
                    .title_top(
                        Line::from(format!(" DuckPack v{} ", crate::engine::duckpack_version()))
                            .alignment(Alignment::Center)
                            .style(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
                    ),
            );
            f.render_widget(top_header, main_chunks[0]);

            let exp_pct = explorer_width_percent.clamp(15, 60);
            let right_pct = 100 - exp_pct;
            let chunks = Layout::default()
                .direction(Direction::Horizontal)
                .constraints(
                    [
                        Constraint::Percentage(exp_pct),
                        Constraint::Percentage(right_pct),
                    ]
                    .as_ref(),
                )
                .split(main_chunks[1]);

            let is_error_status = status_msg.contains("Error")
                || status_msg.contains("⚠️")
                || status_msg.contains("Failed");
            let status_height =
                calculate_status_height(&status_msg, chunks[1].width as usize, chunks[1].height);

            let right_chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints(
                    [
                        Constraint::Length(3),
                        Constraint::Min(8),
                        Constraint::Percentage(45),
                        Constraint::Length(status_height),
                    ]
                    .as_ref(),
                )
                .split(chunks[1]);

            let max_item_width = explorer_items
                .iter()
                .map(|item| {
                    let indent = item.level * 2;
                    let expand_indicator = if item.is_expandable { 2 } else { 0 };
                    indent + expand_indicator + item.display.chars().count()
                })
                .max()
                .unwrap_or(0);

            let explorer_inner_width = chunks[0].width.saturating_sub(2) as usize;
            let max_scroll = max_item_width.saturating_sub(explorer_inner_width);
            if explorer_horizontal_scroll > max_scroll {
                explorer_horizontal_scroll = max_scroll;
            }

            // Explorer Pane
            let mut list_items = Vec::new();
            for item in &explorer_items {
                list_items.push(ListItem::new(render_explorer_node_line(
                    item,
                    explorer_horizontal_scroll,
                )));
            }

            let explorer_border_style = if focus == FocusPane::Explorer {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default()
            };

            let explorer_title = match explorer_mode {
                ExplorerMode::Database => {
                    if focus == FocusPane::Explorer {
                        format!(
                            " [F3: DB* | F4: Project] Database ({}%) [W: Width | +/-: Resize | q: Query] ",
                            exp_pct
                        )
                    } else {
                        format!(" [F3: DB* | F4: Project] Database ({}%) ", exp_pct)
                    }
                }
                ExplorerMode::Project => {
                    let git_tag = if let Some(ref b) = git_branch {
                        format!(" ({})", b)
                    } else {
                        String::new()
                    };
                    if focus == FocusPane::Explorer {
                        format!(
                            " [F3: DB | F4: Project*] Project{} ({}%) [a: New | Enter: Open | F6: Diff] ",
                            git_tag, exp_pct
                        )
                    } else {
                        format!(" [F3: DB | F4: Project*] Project{} ({}%) ", git_tag, exp_pct)
                    }
                }
            };

            let explorer_list = List::new(list_items)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(explorer_title)
                        .border_style(explorer_border_style),
                )
                .highlight_style(
                    Style::default()
                        .bg(Color::DarkGray)
                        .add_modifier(Modifier::BOLD),
                );
            f.render_stateful_widget(explorer_list, chunks[0], &mut explorer_state);
            current_explorer_rect = chunks[0];

            // Tabs Pane
            let tab_titles: Vec<Line> = tabs
                .iter()
                .enumerate()
                .map(|(i, tab)| {
                    let name = if let Some(ref path) = tab.active_file {
                        path.file_name()
                            .unwrap_or(std::ffi::OsStr::new("unknown"))
                            .to_string_lossy()
                            .to_string()
                    } else {
                        format!("New Query {}", i + 1)
                    };
                    let dirty = if tab.is_dirty { " *" } else { "" };
                    let db_badge = if !tab.target_database.is_empty() {
                        format!(" ({})", tab.target_database)
                    } else {
                        String::new()
                    };
                    let style = if i == active_tab_index {
                        Style::default()
                            .fg(Color::Yellow)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(Color::DarkGray)
                    };
                    Line::from(vec![Span::styled(
                        format!(" {}{}{} ", name, db_badge, dirty),
                        style,
                    )])
                })
                .collect();

            let tabs_widget = Tabs::new(tab_titles)
                .block(Block::default().borders(Borders::ALL).title(" Open Tabs "))
                .select(active_tab_index)
                .highlight_style(
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::REVERSED),
                );
            f.render_widget(tabs_widget, right_chunks[0]);
            current_tabs_rect = right_chunks[0];

            // Editor Pane
            let active_tab = &mut tabs[active_tab_index];
            let file_display = if let Some(ref path) = active_tab.active_file {
                path.file_name()
                    .unwrap_or(std::ffi::OsStr::new("unknown"))
                    .to_string_lossy()
                    .to_string()
            } else {
                format!("New Query {}", active_tab_index + 1)
            };
            let dirty = if active_tab.is_dirty { " *" } else { "" };
            let (icon, db_type_tag) = if let Some(d) = meta
                .databases
                .iter()
                .find(|d| d.name == active_tab.target_database)
            {
                (db_icon(&d.db_type), format!(" ({})", d.db_type))
            } else {
                ("📁", String::new())
            };
            let editor_title = if !active_tab.target_database.is_empty() {
                format!(
                    " Editor [{}{}] ── DB: {} {}{} [F2: Switch DB] ",
                    file_display, dirty, icon, active_tab.target_database, db_type_tag
                )
            } else {
                format!(" Editor [{}{}] [F2: Switch DB] ", file_display, dirty)
            };

            let sql_keywords = SQL_KEYWORD_PATTERN;
            if active_tab.textarea.is_selecting() {
                active_tab.textarea.set_search_pattern("a^").unwrap_or(());
            } else {
                active_tab
                    .textarea
                    .set_search_pattern(sql_keywords)
                    .unwrap_or(());
            }

            let editor_border_style = if focus == FocusPane::Editor {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default()
            };

            active_tab.textarea.set_block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(editor_title)
                    .title_bottom(
                        Line::from(
                            " ➕ New (Ctrl+T)  |  ❌ Close (Ctrl+W)  |  💾 Save (Ctrl+S)  |  ✨ Format (Ctrl+F)  |  🗄️ DB (F2) ",
                        )
                        .alignment(Alignment::Left),
                    )
                    .title_bottom(Line::from(" ▶ Run (F5) ").alignment(Alignment::Right))
                    .border_style(editor_border_style),
            );
            f.render_widget(&active_tab.textarea, right_chunks[1]);
            current_editor_rect = right_chunks[1];

            // Results Pane
            let results_border_style = if focus == FocusPane::Results {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default()
            };

            let active_tab = &mut tabs[active_tab_index];

            if active_tab.query_results.is_empty() && active_tab.column_names.is_empty() {
                let results_block = Block::default()
                    .borders(Borders::ALL)
                    .title(" Results ")
                    .title_bottom(Line::from(" [?] Help ").alignment(Alignment::Right))
                    .border_style(results_border_style);
                f.render_widget(
                    Paragraph::new("No results.").block(results_block),
                    right_chunks[2],
                );
            } else {
                let total_cols = active_tab.column_names.len();
                let total_rows = active_tab.query_results.len();

                if total_cols > 0 && active_tab.horizontal_scroll >= total_cols {
                    active_tab.horizontal_scroll = total_cols.saturating_sub(1);
                }

                let avail_width = (right_chunks[2].width.saturating_sub(2) as usize).max(10);

                let mut visible_indices = Vec::new();
                let mut widths = Vec::new();
                let mut used_width: usize = 0;

                for col_idx in active_tab.horizontal_scroll..total_cols {
                    let col_name = &active_tab.column_names[col_idx];
                    let mut max_len = col_name.len();
                    for row in active_tab.query_results.iter().take(50) {
                        if let Some(cell) = row.get(col_idx) {
                            max_len = max_len.max(cell.len());
                        }
                    }
                    let col_w = (max_len + 2).clamp(6, 30);

                    if visible_indices.is_empty() || used_width + col_w <= avail_width {
                        used_width += col_w;
                        widths.push(Constraint::Length(col_w as u16));
                        visible_indices.push(col_idx);
                    } else {
                        break;
                    }
                }

                let (start_col, end_col) = if visible_indices.is_empty() {
                    (0, 0)
                } else {
                    (
                        visible_indices[0] + 1,
                        visible_indices.last().unwrap() + 1,
                    )
                };

                let row_info = if active_tab.is_paginated {
                    let offset = active_tab.pagination_offset;
                    format!(
                        "Rows {} - {} | Page Limit: {}",
                        offset + 1,
                        offset + total_rows,
                        active_tab.pagination_limit_label()
                    )
                } else {
                    format!(
                        "{} rows | Page Limit: {}",
                        total_rows,
                        active_tab.pagination_limit_label()
                    )
                };

                let col_info = if total_cols > 0 {
                    format!(" [Cols {}-{} of {}]", start_col, end_col, total_cols)
                } else {
                    String::new()
                };

                let title = format!(" Results ({}{}) ", row_info, col_info);

                let nav_hint = if total_cols > visible_indices.len() {
                    format!(
                        " ◄/► Scroll Cols | [n/p] Page | [l] Limit ({}) | [?] Help ",
                        active_tab.pagination_limit_label()
                    )
                } else {
                    format!(
                        " [n/p] Page | [l] Limit ({}) | [?] Help ",
                        active_tab.pagination_limit_label()
                    )
                };

                let results_block = Block::default()
                    .borders(Borders::ALL)
                    .title(title)
                    .title_bottom(Line::from(nav_hint).alignment(Alignment::Right))
                    .border_style(results_border_style);

                let header = Row::new(
                    visible_indices
                        .iter()
                        .map(|&i| active_tab.column_names[i].as_str()),
                )
                .style(Style::default().add_modifier(Modifier::BOLD));

                let rows: Vec<Row> = active_tab
                    .query_results
                    .iter()
                    .map(|r| {
                        let display_cells: Vec<&str> = visible_indices
                            .iter()
                            .map(|&i| r.get(i).map(|s| s.as_str()).unwrap_or(""))
                            .collect();
                        Row::new(display_cells)
                    })
                    .collect();

                let table = Table::new(rows, widths)
                    .header(header)
                    .block(results_block)
                    .row_highlight_style(
                        Style::default()
                            .bg(Color::DarkGray)
                            .add_modifier(Modifier::BOLD),
                    );
                f.render_stateful_widget(table, right_chunks[2], &mut active_tab.results_state);
            }
            current_results_rect = right_chunks[2];

            // Modal overlays
            if exit_confirm_modal.is_open {
                let dirty_count = tabs.iter().filter(|t| t.is_dirty).count();
                current_exit_dialog_rect = exit_confirm_modal.render(f, dirty_count);
            } else if show_help {
                render_help_modal(f);
            } else if show_schema_visualizer {
                max_schema_scroll =
                    render_schema_visualizer_modal(f, &schema_ascii_tree, schema_scroll);
            } else if db_picker_modal.is_open {
                db_picker_modal.render(
                    f,
                    &meta.databases,
                    &tabs[active_tab_index].target_database,
                );
            } else if attach_modal.is_open {
                attach_modal.render(f);
            } else if secret_modal.is_open {
                secret_modal.render(f);
            } else if add_bucket_modal.is_open {
                add_bucket_modal.render(f);
            } else if new_asset_modal.is_open {
                new_asset_modal.render(f, project_dir);
            } else if in_ide_diff_modal.is_open {
                in_ide_diff_modal.render(f);
            }

            // Status Bar
            let status_title = if status_msg.starts_with("📋 [Copied] ") {
                " Status [Copied to clipboard! 📋] "
            } else if is_error_status {
                " Status / Error [Click box to copy 📋] "
            } else {
                " Status [Click box to copy 📋] "
            };

            let status_block = Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .padding(Padding::horizontal(1))
                .border_style(if is_error_status {
                    Style::default()
                        .fg(Color::LightRed)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::DarkGray)
                })
                .title(Line::from(status_title).style(if is_error_status {
                    Style::default()
                        .fg(Color::LightRed)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::Cyan)
                }));

            let status = Paragraph::new(status_msg.as_str())
                .block(status_block)
                .style(if is_error_status {
                    Style::default().fg(Color::LightRed)
                } else {
                    Style::default().fg(Color::White)
                })
                .wrap(Wrap { trim: false });
            f.render_widget(status, right_chunks[3]);
            current_status_rect = right_chunks[3];
        })?;

        let mut execute_query = false;
        let mut save_query = false;
        let mut export_query = false;
        let mut format_query = false;
        let mut new_tab = false;
        let mut close_tab = false;
        let mut trigger_explorer_action = false;
        let mut trigger_explorer_refresh = false;
        let mut trigger_pagination: Option<i32> = None;
        let ev = event::read()?;

        match ev {
            Event::Mouse(mouse) => {
                if mouse.kind == MouseEventKind::Down(MouseButton::Left) {
                    if exit_confirm_modal.is_open {
                        if mouse.row == current_exit_dialog_rect.bottom().saturating_sub(2) {
                            let center_x = current_exit_dialog_rect.left()
                                + current_exit_dialog_rect.width / 2;
                            if mouse.column >= current_exit_dialog_rect.left()
                                && mouse.column < center_x
                            {
                                break;
                            } else if mouse.column >= center_x
                                && mouse.column <= current_exit_dialog_rect.right()
                            {
                                exit_confirm_modal.close();
                                status_msg = "Exit cancelled".to_string();
                            }
                        } else if mouse.row < current_exit_dialog_rect.top()
                            || mouse.row >= current_exit_dialog_rect.bottom()
                            || mouse.column < current_exit_dialog_rect.left()
                            || mouse.column >= current_exit_dialog_rect.right()
                        {
                            exit_confirm_modal.close();
                            status_msg = "Exit cancelled".to_string();
                        }
                    } else if mouse.row >= current_status_rect.top()
                        && mouse.row < current_status_rect.bottom()
                        && mouse.column >= current_status_rect.left()
                        && mouse.column < current_status_rect.right()
                    {
                        let clean_text =
                            if let Some(stripped) = status_msg.strip_prefix("📋 [Copied] ") {
                                stripped.to_string()
                            } else {
                                status_msg.clone()
                            };
                        if !clean_text.is_empty() {
                            if set_clipboard_text(&clean_text, &mut os_clipboard) {
                                if !status_msg.starts_with("📋 [Copied] ") {
                                    status_msg = format!("📋 [Copied] {}", status_msg);
                                }
                            }
                        }
                    } else if mouse.row == current_editor_rect.bottom().saturating_sub(1) {
                        if mouse.column >= current_editor_rect.right().saturating_sub(15)
                            && mouse.column <= current_editor_rect.right()
                        {
                            execute_query = true;
                        }
                        if mouse.column >= current_editor_rect.left()
                            && mouse.column <= current_editor_rect.left() + 18
                        {
                            new_tab = true;
                        }
                        if mouse.column > current_editor_rect.left() + 18
                            && mouse.column <= current_editor_rect.left() + 40
                        {
                            close_tab = true;
                        }
                        if mouse.column > current_editor_rect.left() + 40
                            && mouse.column <= current_editor_rect.left() + 61
                        {
                            save_query = true;
                        }
                        if mouse.column > current_editor_rect.left() + 61
                            && mouse.column <= current_editor_rect.left() + 85
                        {
                            format_query = true;
                        }
                        if mouse.column > current_editor_rect.left() + 85
                            && mouse.column <= current_editor_rect.left() + 105
                        {
                            db_picker_modal.open_for_current(
                                &tabs[active_tab_index].target_database,
                                &meta.databases,
                            );
                        }
                    } else if mouse.row == current_results_rect.bottom().saturating_sub(1) {
                        if mouse.column >= current_results_rect.right().saturating_sub(10)
                            && mouse.column <= current_results_rect.right()
                        {
                            show_help = !show_help;
                        }
                    } else if mouse.row >= current_editor_rect.top()
                        && mouse.row < current_editor_rect.bottom()
                    {
                        if mouse.column >= current_editor_rect.left()
                            && mouse.column <= current_editor_rect.right()
                        {
                            focus = FocusPane::Editor;
                        }
                    }

                    // Explorer Click
                    if mouse.row == current_explorer_rect.top() {
                        if mouse.column >= current_explorer_rect.left() + 1
                            && mouse.column <= current_explorer_rect.left() + 8
                        {
                            explorer_mode = ExplorerMode::Database;
                            explorer_items = rebuild_explorer_nodes(
                                &meta,
                                &expanded_groups,
                                &expanded_tables,
                                conn,
                                is_quack,
                            );
                            explorer_state.select(Some(0));
                            status_msg = "Switched to Database Catalog view".to_string();
                        } else if mouse.column >= current_explorer_rect.left() + 9
                            && mouse.column <= current_explorer_rect.left() + 22
                        {
                            explorer_mode = ExplorerMode::Project;
                            let (b, st) = get_git_branch_and_status(project_dir);
                            git_branch = b;
                            git_status = st;
                            explorer_items = rebuild_project_nodes(
                                project_dir,
                                &expanded_project_groups,
                                &git_status,
                            );
                            explorer_state.select(Some(0));
                            status_msg = "Switched to Project Files view".to_string();
                        } else if explorer_mode == ExplorerMode::Project
                            && mouse.column >= current_explorer_rect.right().saturating_sub(20)
                            && mouse.column <= current_explorer_rect.right()
                        {
                            new_asset_modal.reset();
                        } else if explorer_mode == ExplorerMode::Database
                            && mouse.column >= current_explorer_rect.right().saturating_sub(18)
                            && mouse.column <= current_explorer_rect.right()
                        {
                            attach_modal.reset();
                        }
                    } else if mouse.row >= current_explorer_rect.top() + 1
                        && mouse.row < current_explorer_rect.bottom() - 1
                    {
                        if mouse.column >= current_explorer_rect.left() + 1
                            && mouse.column < current_explorer_rect.right() - 1
                        {
                            let list_offset = explorer_state.offset();
                            let clicked_idx = (mouse.row - current_explorer_rect.top() - 1)
                                as usize
                                + list_offset;
                            if clicked_idx < explorer_items.len() {
                                focus = FocusPane::Explorer;
                                explorer_state.select(Some(clicked_idx));
                                trigger_explorer_action = true;
                            }
                        }
                    }

                    // Tabs Click
                    if mouse.row == current_tabs_rect.top() + 1
                        && mouse.column >= current_tabs_rect.left() + 1
                        && mouse.column < current_tabs_rect.right() - 1
                    {
                        let mut current_x = current_tabs_rect.left() + 1;
                        for (i, tab) in tabs.iter().enumerate() {
                            let tab_name = if let Some(ref path) = tab.active_file {
                                path.file_name()
                                    .unwrap_or(std::ffi::OsStr::new("unknown"))
                                    .to_string_lossy()
                                    .to_string()
                            } else {
                                format!("New Query {}", i + 1)
                            };
                            let dirty = if tab.is_dirty { " *" } else { "" };
                            let db_badge = if !tab.target_database.is_empty() {
                                format!(" ({})", tab.target_database)
                            } else {
                                String::new()
                            };
                            let tab_width = (tab_name.chars().count()
                                + db_badge.chars().count()
                                + dirty.chars().count()
                                + 2) as u16;
                            let display_width = tab_width + 3; // Ratatui divider " | "
                            if mouse.column >= current_x && mouse.column < current_x + display_width
                            {
                                active_tab_index = i;
                                focus = FocusPane::Editor;
                                break;
                            }
                            current_x += display_width;
                        }
                    }
                } else if mouse.kind == MouseEventKind::ScrollDown {
                    if mouse.modifiers.contains(KeyModifiers::SHIFT) {
                        if mouse.row >= current_explorer_rect.top()
                            && mouse.row <= current_explorer_rect.bottom()
                            && mouse.column >= current_explorer_rect.left()
                            && mouse.column <= current_explorer_rect.right()
                        {
                            explorer_horizontal_scroll += 4;
                        } else if mouse.row >= current_results_rect.top()
                            && mouse.row <= current_results_rect.bottom()
                            && mouse.column >= current_results_rect.left()
                            && mouse.column <= current_results_rect.right()
                        {
                            let max_col =
                                tabs[active_tab_index].column_names.len().saturating_sub(1);
                            if tabs[active_tab_index].horizontal_scroll < max_col {
                                tabs[active_tab_index].horizontal_scroll += 1;
                            }
                        } else if mouse.row >= current_editor_rect.top()
                            && mouse.row <= current_editor_rect.bottom()
                            && mouse.column >= current_editor_rect.left()
                            && mouse.column <= current_editor_rect.right()
                        {
                            tabs[active_tab_index].textarea.scroll((0, 1));
                        }
                    } else if mouse.row >= current_explorer_rect.top()
                        && mouse.row <= current_explorer_rect.bottom()
                        && mouse.column >= current_explorer_rect.left()
                        && mouse.column <= current_explorer_rect.right()
                    {
                        if let Some(selected) = explorer_state.selected() {
                            if selected < explorer_items.len().saturating_sub(1) {
                                explorer_state.select(Some(selected + 1));
                            }
                        } else if !explorer_items.is_empty() {
                            explorer_state.select(Some(0));
                        }
                    } else if mouse.row >= current_results_rect.top()
                        && mouse.row <= current_results_rect.bottom()
                        && mouse.column >= current_results_rect.left()
                        && mouse.column <= current_results_rect.right()
                    {
                        let active_tab = &mut tabs[active_tab_index];
                        if let Some(selected) = active_tab.results_state.selected() {
                            if selected < active_tab.query_results.len().saturating_sub(1) {
                                active_tab.results_state.select(Some(selected + 1));
                            }
                        } else if !active_tab.query_results.is_empty() {
                            active_tab.results_state.select(Some(0));
                        }
                    } else if mouse.row >= current_editor_rect.top()
                        && mouse.row <= current_editor_rect.bottom()
                        && mouse.column >= current_editor_rect.left()
                        && mouse.column <= current_editor_rect.right()
                    {
                        tabs[active_tab_index].textarea.scroll((1, 0));
                    }
                } else if mouse.kind == MouseEventKind::ScrollUp {
                    if mouse.modifiers.contains(KeyModifiers::SHIFT) {
                        if mouse.row >= current_explorer_rect.top()
                            && mouse.row <= current_explorer_rect.bottom()
                            && mouse.column >= current_explorer_rect.left()
                            && mouse.column <= current_explorer_rect.right()
                        {
                            explorer_horizontal_scroll =
                                explorer_horizontal_scroll.saturating_sub(4);
                        } else if mouse.row >= current_results_rect.top()
                            && mouse.row <= current_results_rect.bottom()
                            && mouse.column >= current_results_rect.left()
                            && mouse.column <= current_results_rect.right()
                        {
                            let active_tab = &mut tabs[active_tab_index];
                            if active_tab.horizontal_scroll > 0 {
                                active_tab.horizontal_scroll -= 1;
                            }
                        } else if mouse.row >= current_editor_rect.top()
                            && mouse.row <= current_editor_rect.bottom()
                            && mouse.column >= current_editor_rect.left()
                            && mouse.column <= current_editor_rect.right()
                        {
                            tabs[active_tab_index].textarea.scroll((0, -1));
                        }
                    } else if mouse.row >= current_explorer_rect.top()
                        && mouse.row <= current_explorer_rect.bottom()
                        && mouse.column >= current_explorer_rect.left()
                        && mouse.column <= current_explorer_rect.right()
                    {
                        if let Some(selected) = explorer_state.selected() {
                            if selected > 0 {
                                explorer_state.select(Some(selected - 1));
                            }
                        }
                    } else if mouse.row >= current_results_rect.top()
                        && mouse.row <= current_results_rect.bottom()
                        && mouse.column >= current_results_rect.left()
                        && mouse.column <= current_results_rect.right()
                    {
                        let active_tab = &mut tabs[active_tab_index];
                        if let Some(selected) = active_tab.results_state.selected() {
                            if selected > 0 {
                                active_tab.results_state.select(Some(selected - 1));
                            }
                        }
                    } else if mouse.row >= current_editor_rect.top()
                        && mouse.row <= current_editor_rect.bottom()
                        && mouse.column >= current_editor_rect.left()
                        && mouse.column <= current_editor_rect.right()
                    {
                        tabs[active_tab_index].textarea.scroll((-1, 0));
                    }
                } else if mouse.kind == MouseEventKind::ScrollLeft {
                    if mouse.row >= current_explorer_rect.top()
                        && mouse.row <= current_explorer_rect.bottom()
                        && mouse.column >= current_explorer_rect.left()
                        && mouse.column <= current_explorer_rect.right()
                    {
                        explorer_horizontal_scroll = explorer_horizontal_scroll.saturating_sub(4);
                    } else if mouse.row >= current_results_rect.top()
                        && mouse.row <= current_results_rect.bottom()
                        && mouse.column >= current_results_rect.left()
                        && mouse.column <= current_results_rect.right()
                    {
                        let active_tab = &mut tabs[active_tab_index];
                        if active_tab.horizontal_scroll > 0 {
                            active_tab.horizontal_scroll -= 1;
                        }
                    } else if mouse.row >= current_editor_rect.top()
                        && mouse.row <= current_editor_rect.bottom()
                        && mouse.column >= current_editor_rect.left()
                        && mouse.column <= current_editor_rect.right()
                    {
                        tabs[active_tab_index].textarea.scroll((0, -1));
                    }
                } else if mouse.kind == MouseEventKind::ScrollRight {
                    if mouse.row >= current_explorer_rect.top()
                        && mouse.row <= current_explorer_rect.bottom()
                        && mouse.column >= current_explorer_rect.left()
                        && mouse.column <= current_explorer_rect.right()
                    {
                        explorer_horizontal_scroll += 4;
                    } else if mouse.row >= current_results_rect.top()
                        && mouse.row <= current_results_rect.bottom()
                        && mouse.column >= current_results_rect.left()
                        && mouse.column <= current_results_rect.right()
                    {
                        let max_col = tabs[active_tab_index].column_names.len().saturating_sub(1);
                        if tabs[active_tab_index].horizontal_scroll < max_col {
                            tabs[active_tab_index].horizontal_scroll += 1;
                        }
                    } else if mouse.row >= current_editor_rect.top()
                        && mouse.row <= current_editor_rect.bottom()
                        && mouse.column >= current_editor_rect.left()
                        && mouse.column <= current_editor_rect.right()
                    {
                        tabs[active_tab_index].textarea.scroll((0, 1));
                    }
                }

                // Forward mouse events to textarea if inside editor bounds
                if mouse.row >= current_editor_rect.top()
                    && mouse.row < current_editor_rect.bottom() - 1
                    && mouse.column >= current_editor_rect.left()
                    && mouse.column <= current_editor_rect.right()
                    && focus == FocusPane::Editor
                {
                    tabs[active_tab_index].textarea.input(ev.clone());
                }
            }
            Event::Key(key) => {
                if exit_confirm_modal.is_open {
                    match exit_confirm_modal.handle_key(key) {
                        Some(true) => break,
                        Some(false) => status_msg = "Exit cancelled".to_string(),
                        None => {}
                    }
                } else if new_asset_modal.is_open {
                    if let Some(created_path) = new_asset_modal.handle_key(key, project_dir) {
                        open_or_focus_file_in_tabs(
                            &mut tabs,
                            &mut active_tab_index,
                            created_path.clone(),
                        );
                        focus = FocusPane::Editor;
                        trigger_explorer_refresh = true;
                        status_msg = format!(
                            "Created and opened {}",
                            created_path
                                .file_name()
                                .unwrap_or_default()
                                .to_string_lossy()
                        );
                    }
                } else if in_ide_diff_modal.is_open {
                    let tab_cat = tabs
                        .get(active_tab_index)
                        .map(|t| t.target_database.clone())
                        .filter(|c| !c.is_empty() && c != "memory");
                    if let Some(msg) =
                        in_ide_diff_modal.handle_key(key, project_dir, conn, tab_cat.as_deref())
                    {
                        status_msg = msg;
                        trigger_explorer_refresh = true;
                    }
                } else if attach_modal.is_open {
                    if let Some(alias) = attach_modal.handle_key(key, conn) {
                        tabs[active_tab_index].target_database = alias.clone();
                        status_msg = format!(
                            "Attached database '{}' successfully and switched active tab context.",
                            alias
                        );
                        trigger_explorer_refresh = true;
                    }
                } else if secret_modal.is_open {
                    if let Some(msg) = secret_modal.handle_key(key, conn, project_dir) {
                        status_msg = msg;
                        trigger_explorer_refresh = true;
                    }
                } else if add_bucket_modal.is_open {
                    if let Some(msg) =
                        add_bucket_modal.handle_key(key, project_dir, &mut expanded_groups)
                    {
                        status_msg = msg;
                        trigger_explorer_refresh = true;
                    }
                } else if db_picker_modal.is_open {
                    if let Some(db_name) = db_picker_modal.handle_key(key, &mut meta, conn) {
                        tabs[active_tab_index].target_database = db_name.clone();
                        status_msg =
                            format!("Target database switched to '{}' for active tab.", db_name);
                    }
                } else if key.code == KeyCode::Char('a')
                    && key.modifiers.contains(KeyModifiers::CONTROL)
                {
                    attach_modal.reset();
                } else if key.code == KeyCode::Char('k')
                    && key.modifiers.contains(KeyModifiers::CONTROL)
                {
                    secret_modal.reset();
                } else if show_help || show_schema_visualizer {
                    if key.code == KeyCode::Esc
                        || key.code == KeyCode::Char('q')
                        || key.code == KeyCode::Char('?')
                    {
                        show_help = false;
                        show_schema_visualizer = false;
                    } else if show_schema_visualizer {
                        if key.code == KeyCode::Down || key.code == KeyCode::Char('j') {
                            schema_scroll = (schema_scroll + 1).min(max_schema_scroll);
                        } else if key.code == KeyCode::Up || key.code == KeyCode::Char('k') {
                            schema_scroll = schema_scroll.saturating_sub(1);
                        }
                    }
                } else {
                    if key.code == KeyCode::F(2) {
                        db_picker_modal.open_for_current(
                            &tabs[active_tab_index].target_database,
                            &meta.databases,
                        );
                        continue;
                    }

                    if key.code == KeyCode::Char('?') {
                        show_help = true;
                        continue;
                    }

                    if key.code == KeyCode::Char('g')
                        && key.modifiers.contains(KeyModifiers::CONTROL)
                    {
                        show_schema_visualizer = true;
                        schema_scroll = 0;
                        schema_ascii_tree =
                            generate_schema_visualizer(conn, &mut os_clipboard, is_quack)
                                .unwrap_or_else(|e| {
                                    format!("Failed to generate schema visualizer: {}", e)
                                });
                        continue;
                    }

                    if key.code == KeyCode::F(3) {
                        explorer_mode = ExplorerMode::Database;
                        explorer_items = rebuild_explorer_nodes(
                            &meta,
                            &expanded_groups,
                            &expanded_tables,
                            conn,
                            is_quack,
                        );
                        explorer_state.select(Some(0));
                        status_msg = "Switched to Database Catalog view".to_string();
                        continue;
                    }

                    if key.code == KeyCode::F(4) {
                        explorer_mode = ExplorerMode::Project;
                        let (b, st) = get_git_branch_and_status(project_dir);
                        git_branch = b;
                        git_status = st;
                        explorer_items = rebuild_project_nodes(
                            project_dir,
                            &expanded_project_groups,
                            &git_status,
                        );
                        explorer_state.select(Some(0));
                        status_msg = "Switched to Project Files view".to_string();
                        continue;
                    }

                    if key.code == KeyCode::F(6)
                        || (key.code == KeyCode::Char('d')
                            && key.modifiers.contains(KeyModifiers::CONTROL))
                    {
                        let tab_cat = tabs
                            .get(active_tab_index)
                            .map(|t| t.target_database.as_str())
                            .filter(|c| !c.is_empty() && *c != "memory");
                        match compute_ide_diff_for_catalog(project_dir, conn, tab_cat) {
                            Ok(diff) => {
                                in_ide_diff_modal.reset();
                                in_ide_diff_modal.diff_result = Some(diff);
                                in_ide_diff_modal.is_open = true;
                            }
                            Err(e) => {
                                let err_str = e.to_string();
                                status_msg = format!("⚠️ Diff Failed: {}", err_str);
                                handle_sql_error_position(
                                    &mut tabs[active_tab_index].textarea,
                                    &err_str,
                                );
                            }
                        }
                        continue;
                    }

                    if key.code == KeyCode::Char('b')
                        && key.modifiers.contains(KeyModifiers::CONTROL)
                    {
                        match crate::engine::build_shadow_db(project_dir) {
                            Ok(_) => {
                                status_msg =
                                    "✓ Project validated successfully (0 syntax or schema errors)."
                                        .to_string();
                            }
                            Err(e) => {
                                let err_str = e.to_string();
                                status_msg = format!("⚠️ Validation Error: {}", err_str);
                                handle_sql_error_position(
                                    &mut tabs[active_tab_index].textarea,
                                    &err_str,
                                );
                            }
                        }
                        continue;
                    }

                    if key.code == KeyCode::F(5)
                        || (key.code == KeyCode::Char('e')
                            && key.modifiers.contains(KeyModifiers::CONTROL))
                    {
                        execute_query = true;
                    }

                    // Global Alt shortcuts for explorer resizing
                    if key.modifiers.contains(KeyModifiers::ALT) {
                        if key.code == KeyCode::Char('w') || key.code == KeyCode::Char('W') {
                            explorer_width_percent = match explorer_width_percent {
                                p if p < 30 => 35,
                                p if p < 45 => 50,
                                _ => 25,
                            };
                            status_msg =
                                format!("Explorer width preset: {}%", explorer_width_percent);
                            continue;
                        } else if key.code == KeyCode::Char(']')
                            || key.code == KeyCode::Char('+')
                            || key.code == KeyCode::Char('=')
                        {
                            explorer_width_percent = (explorer_width_percent + 5).min(60);
                            status_msg = format!("Explorer width: {}%", explorer_width_percent);
                            continue;
                        } else if key.code == KeyCode::Char('[') || key.code == KeyCode::Char('-') {
                            explorer_width_percent =
                                explorer_width_percent.saturating_sub(5).max(15);
                            status_msg = format!("Explorer width: {}%", explorer_width_percent);
                            continue;
                        }
                    }

                    // Global quit confirmation prompt
                    if key.code == KeyCode::Esc {
                        exit_confirm_modal.open();
                        continue;
                    }

                    // Global tab navigation (forward)
                    if key.code == KeyCode::Tab {
                        focus = match focus {
                            FocusPane::Explorer => FocusPane::Editor,
                            FocusPane::Editor => FocusPane::Results,
                            FocusPane::Results => FocusPane::Explorer,
                        };
                        continue;
                    }

                    // Global tab navigation (backward)
                    if key.code == KeyCode::BackTab {
                        focus = match focus {
                            FocusPane::Explorer => FocusPane::Results,
                            FocusPane::Editor => FocusPane::Explorer,
                            FocusPane::Results => FocusPane::Editor,
                        };
                        continue;
                    }

                    if key.code == KeyCode::Char('s')
                        && key.modifiers.contains(KeyModifiers::CONTROL)
                    {
                        save_query = true;
                    }
                    if key.code == KeyCode::Char('o')
                        && key.modifiers.contains(KeyModifiers::CONTROL)
                    {
                        export_query = true;
                    }
                    if key.code == KeyCode::Char('f')
                        && key.modifiers.contains(KeyModifiers::CONTROL)
                    {
                        format_query = true;
                    }
                    if key.code == KeyCode::Char('t')
                        && key.modifiers.contains(KeyModifiers::CONTROL)
                    {
                        new_tab = true;
                    }
                    if key.code == KeyCode::Char('w')
                        && key.modifiers.contains(KeyModifiers::CONTROL)
                    {
                        tabs.remove(active_tab_index);
                        if tabs.is_empty() {
                            tabs.push(EditorTab::new(meta.active_database.clone()));
                        }
                        if active_tab_index >= tabs.len() {
                            active_tab_index = tabs.len() - 1;
                        }
                        status_msg = "Closed tab".to_string();
                    }
                    if key.code == KeyCode::Char('n')
                        && key.modifiers.contains(KeyModifiers::CONTROL)
                    {
                        active_tab_index = (active_tab_index + 1) % tabs.len();
                    }
                    if key.code == KeyCode::Char('p')
                        && key.modifiers.contains(KeyModifiers::CONTROL)
                    {
                        if active_tab_index == 0 {
                            active_tab_index = tabs.len() - 1;
                        } else {
                            active_tab_index -= 1;
                        }
                    }

                    // Copy to OS Clipboard
                    if key.code == KeyCode::Char('c')
                        && key.modifiers.contains(KeyModifiers::CONTROL)
                        && focus == FocusPane::Editor
                    {
                        let active_tab = &mut tabs[active_tab_index];
                        let text_to_copy = if active_tab.textarea.selection_range().is_some() {
                            active_tab.textarea.copy();
                            active_tab.textarea.yank_text().to_string()
                        } else {
                            let cursor = active_tab.textarea.cursor();
                            if cursor.0 < active_tab.textarea.lines().len() {
                                format!("{}\n", active_tab.textarea.lines()[cursor.0])
                            } else {
                                active_tab.textarea.copy();
                                active_tab.textarea.yank_text().to_string()
                            }
                        };
                        if !text_to_copy.is_empty()
                            && set_clipboard_text(&text_to_copy, &mut os_clipboard)
                        {
                            status_msg = "Copied to clipboard".to_string();
                        }
                        continue;
                    }

                    // Paste from OS Clipboard
                    if key.code == KeyCode::Char('v')
                        && key.modifiers.contains(KeyModifiers::CONTROL)
                        && focus == FocusPane::Editor
                    {
                        if let Some(text) = get_clipboard_text(&mut os_clipboard) {
                            let active_tab = &mut tabs[active_tab_index];
                            active_tab.textarea.insert_str(&text);
                            status_msg = "Pasted from clipboard".to_string();
                        }
                        continue;
                    }

                    // Pane-specific events
                    match focus {
                        FocusPane::Explorer => {
                            if key.code == KeyCode::Up {
                                if let Some(selected) = explorer_state.selected() {
                                    if selected > 0 {
                                        explorer_state.select(Some(selected - 1));
                                    }
                                }
                            } else if key.code == KeyCode::Down {
                                if let Some(selected) = explorer_state.selected() {
                                    if selected < explorer_items.len().saturating_sub(1) {
                                        explorer_state.select(Some(selected + 1));
                                    }
                                }
                            } else if key.code == KeyCode::Left || key.code == KeyCode::Char('h') {
                                if key.modifiers.contains(KeyModifiers::SHIFT) {
                                    explorer_horizontal_scroll =
                                        explorer_horizontal_scroll.saturating_sub(10);
                                } else {
                                    explorer_horizontal_scroll =
                                        explorer_horizontal_scroll.saturating_sub(4);
                                }
                            } else if key.code == KeyCode::Right || key.code == KeyCode::Char('l') {
                                if key.modifiers.contains(KeyModifiers::SHIFT) {
                                    explorer_horizontal_scroll += 10;
                                } else {
                                    explorer_horizontal_scroll += 4;
                                }
                            } else if key.code == KeyCode::Home {
                                explorer_horizontal_scroll = 0;
                            } else if key.code == KeyCode::Char('w')
                                || key.code == KeyCode::Char('W')
                            {
                                explorer_width_percent = match explorer_width_percent {
                                    p if p < 30 => 35,
                                    p if p < 45 => 50,
                                    _ => 25,
                                };
                                status_msg =
                                    format!("Explorer width preset: {}%", explorer_width_percent);
                            } else if key.code == KeyCode::Char('+')
                                || key.code == KeyCode::Char('=')
                                || key.code == KeyCode::Char(']')
                            {
                                explorer_width_percent = (explorer_width_percent + 5).min(60);
                                status_msg = format!("Explorer width: {}%", explorer_width_percent);
                            } else if key.code == KeyCode::Char('-')
                                || key.code == KeyCode::Char('[')
                            {
                                explorer_width_percent =
                                    explorer_width_percent.saturating_sub(5).max(15);
                                status_msg = format!("Explorer width: {}%", explorer_width_percent);
                            } else if key.code == KeyCode::Enter || key.code == KeyCode::Char(' ') {
                                trigger_explorer_action = true;
                            } else if key.code == KeyCode::Char('1') {
                                explorer_mode = ExplorerMode::Database;
                                explorer_items = rebuild_explorer_nodes(
                                    &meta,
                                    &expanded_groups,
                                    &expanded_tables,
                                    conn,
                                    is_quack,
                                );
                                explorer_state.select(Some(0));
                                status_msg = "Switched to Database Catalog view".to_string();
                            } else if key.code == KeyCode::Char('2') {
                                explorer_mode = ExplorerMode::Project;
                                let (b, st) = get_git_branch_and_status(project_dir);
                                git_branch = b;
                                git_status = st;
                                explorer_items = rebuild_project_nodes(
                                    project_dir,
                                    &expanded_project_groups,
                                    &git_status,
                                );
                                explorer_state.select(Some(0));
                                status_msg = "Switched to Project Files view".to_string();
                            } else if key.code == KeyCode::Char('a')
                                || key.code == KeyCode::Char('n')
                            {
                                if explorer_mode == ExplorerMode::Project {
                                    new_asset_modal.reset();
                                    if let Some(selected) = explorer_state.selected() {
                                        if selected < explorer_items.len() {
                                            match &explorer_items[selected].node_type {
                                                NodeType::Group => {
                                                    let name = &explorer_items[selected].name;
                                                    if name.contains("tables") {
                                                        new_asset_modal.category_index = 0;
                                                    } else if name.contains("views") {
                                                        new_asset_modal.category_index = 1;
                                                    } else if name.contains("macros") {
                                                        new_asset_modal.category_index = 2;
                                                    } else if name.contains("triggers") {
                                                        new_asset_modal.category_index = 3;
                                                    } else if name.contains("schemas") {
                                                        new_asset_modal.category_index = 4;
                                                    } else if name.contains("queries") {
                                                        new_asset_modal.category_index = 5;
                                                    } else if name.contains("seeds") {
                                                        new_asset_modal.category_index = 6;
                                                    }
                                                }
                                                NodeType::ProjectFile { category, .. } => {
                                                    match category {
                                                        ProjectCategory::Table => {
                                                            new_asset_modal.category_index = 0
                                                        }
                                                        ProjectCategory::View => {
                                                            new_asset_modal.category_index = 1
                                                        }
                                                        ProjectCategory::Macro => {
                                                            new_asset_modal.category_index = 2
                                                        }
                                                        ProjectCategory::Trigger => {
                                                            new_asset_modal.category_index = 3
                                                        }
                                                        ProjectCategory::Schema => {
                                                            new_asset_modal.category_index = 4
                                                        }
                                                        ProjectCategory::Query => {
                                                            new_asset_modal.category_index = 5
                                                        }
                                                        ProjectCategory::Seed => {
                                                            new_asset_modal.category_index = 6
                                                        }
                                                        _ => {}
                                                    }
                                                }
                                                _ => {}
                                            }
                                        }
                                    }
                                } else if let Some(selected) = explorer_state.selected() {
                                    if selected < explorer_items.len() {
                                        match &explorer_items[selected].node_type {
                                            NodeType::Group
                                                if explorer_items[selected].name
                                                    == "Secrets & Cloud Buckets" =>
                                            {
                                                secret_modal.reset();
                                            }
                                            NodeType::AddSecretAction
                                            | NodeType::CloudSecret { .. } => {
                                                secret_modal.reset();
                                            }
                                            NodeType::AddBucketAction
                                            | NodeType::CloudBucket { .. }
                                            | NodeType::CloudFile { .. } => {
                                                add_bucket_modal.reset();
                                            }
                                            _ => {
                                                attach_modal.reset();
                                            }
                                        }
                                    } else {
                                        attach_modal.reset();
                                    }
                                } else {
                                    attach_modal.reset();
                                }
                            } else if key.code == KeyCode::Char('v')
                                || key.code == KeyCode::Char('V')
                            {
                                if explorer_mode == ExplorerMode::Project {
                                    match crate::engine::build_shadow_db(project_dir) {
                                        Ok(_) => {
                                            status_msg = "✓ Project validated successfully (0 syntax or schema errors).".to_string();
                                        }
                                        Err(e) => {
                                            let err_str = e.to_string();
                                            status_msg =
                                                format!("⚠️ Validation Error: {}", err_str);
                                            handle_sql_error_position(
                                                &mut tabs[active_tab_index].textarea,
                                                &err_str,
                                            );
                                        }
                                    }
                                }
                            } else if key.code == KeyCode::Char('r')
                                || key.code == KeyCode::Char('R')
                            {
                                if explorer_mode == ExplorerMode::Project {
                                    let (b, st) = get_git_branch_and_status(project_dir);
                                    git_branch = b;
                                    git_status = st;
                                    explorer_items = rebuild_project_nodes(
                                        project_dir,
                                        &expanded_project_groups,
                                        &git_status,
                                    );
                                    status_msg =
                                        "Project files and git status refreshed.".to_string();
                                } else {
                                    refresh_attached_catalog(conn, db_path, quack_disable_ssl);
                                    trigger_explorer_refresh = true;
                                    status_msg = "Explorer refreshed.".to_string();
                                }
                            } else if key.code == KeyCode::Char('q')
                                || key.code == KeyCode::Char('s')
                            {
                                if let Some(selected) = explorer_state.selected() {
                                    if selected < explorer_items.len() {
                                        let item_type = &explorer_items[selected].node_type;
                                        let item_name = &explorer_items[selected].name;
                                        match item_type {
                                            NodeType::Table {
                                                database,
                                                schema,
                                                is_temporary,
                                            }
                                            | NodeType::View {
                                                database,
                                                schema,
                                                is_temporary,
                                            } => {
                                                let db_name = database
                                                    .as_deref()
                                                    .unwrap_or(&meta.active_database);
                                                let schema_name =
                                                    schema.as_deref().unwrap_or("main");

                                                let quote_part = |s: &str| -> String {
                                                    if s.chars()
                                                        .any(|c| !c.is_alphanumeric() && c != '_')
                                                    {
                                                        format!("\"{}\"", s.replace('"', "\"\""))
                                                    } else {
                                                        s.to_string()
                                                    }
                                                };
                                                let quote_schema_path = |s: &str| -> String {
                                                    s.split('.')
                                                        .map(quote_part)
                                                        .collect::<Vec<_>>()
                                                        .join(".")
                                                };
                                                let quoted_item = quote_part(item_name);
                                                let quoted_schema = quote_schema_path(schema_name);
                                                let quoted_db = quote_part(db_name);

                                                let qualified_sql = if *is_temporary {
                                                    format!("SELECT * FROM {};", quoted_item)
                                                } else if is_quack
                                                    && db_name != meta.active_database
                                                    && db_name != "memory"
                                                    && db_name != "target_db"
                                                {
                                                    if schema_name == "main" {
                                                        format!(
                                                            "-- @remote\nSELECT * FROM {}.{};",
                                                            quoted_db, quoted_item
                                                        )
                                                    } else {
                                                        format!(
                                                            "-- @remote\nSELECT * FROM {}.{}.{};",
                                                            quoted_db, quoted_schema, quoted_item
                                                        )
                                                    }
                                                } else if meta.databases.len() > 1 {
                                                    if schema_name == "main" {
                                                        format!(
                                                            "SELECT * FROM {}.{};",
                                                            quoted_db, quoted_item
                                                        )
                                                    } else {
                                                        format!(
                                                            "SELECT * FROM {}.{}.{};",
                                                            quoted_db, quoted_schema, quoted_item
                                                        )
                                                    }
                                                } else if db_name == meta.active_database
                                                    && schema_name == "main"
                                                {
                                                    format!("SELECT * FROM {};", quoted_item)
                                                } else if schema_name == "main" {
                                                    format!(
                                                        "SELECT * FROM {}.{};",
                                                        quoted_db, quoted_item
                                                    )
                                                } else {
                                                    format!(
                                                        "SELECT * FROM {}.{}.{};",
                                                        quoted_db, quoted_schema, quoted_item
                                                    )
                                                };

                                                open_query_in_tabs(
                                                    &mut tabs,
                                                    &mut active_tab_index,
                                                    qualified_sql,
                                                    Some(db_name.to_string()),
                                                );
                                                focus = FocusPane::Editor;
                                                execute_query = true;
                                                let display_table = if schema_name == "main" {
                                                    item_name.clone()
                                                } else {
                                                    format!("{}.{}", schema_name, item_name)
                                                };
                                                status_msg =
                                                    format!("Queried table '{}'", display_table);
                                            }
                                            NodeType::CloudFile { path, is_dir } if !*is_dir => {
                                                let ext = path
                                                    .rsplit('.')
                                                    .next()
                                                    .unwrap_or("")
                                                    .to_lowercase();
                                                let read_fn = match ext.as_str() {
                                                    "parquet" => "read_parquet",
                                                    "csv" => "read_csv",
                                                    "json" | "jsonl" | "ndjson" => "read_json",
                                                    _ => "read_parquet",
                                                };
                                                let query =
                                                    format!("SELECT * FROM {}('{}');", read_fn, path);
                                                open_query_in_tabs(
                                                    &mut tabs,
                                                    &mut active_tab_index,
                                                    query,
                                                    None,
                                                );
                                                focus = FocusPane::Editor;
                                                execute_query = true;
                                                let fname = path.split('/').last().unwrap_or(path);
                                                status_msg =
                                                    format!("Queried cloud file '{}'", fname);
                                            }
                                            NodeType::CloudSecret { name, .. } => {
                                                let query = format!(
                                                    "SELECT * FROM duckdb_secrets() WHERE name = '{}';",
                                                    name
                                                );
                                                open_query_in_tabs(
                                                    &mut tabs,
                                                    &mut active_tab_index,
                                                    query,
                                                    None,
                                                );
                                                focus = FocusPane::Editor;
                                                execute_query = true;
                                                status_msg =
                                                    format!("Inspecting secret '{}'", name);
                                            }
                                            _ => {}
                                        }
                                    }
                                }
                            } else if key.code == KeyCode::Char('d') || key.code == KeyCode::Delete
                            {
                                if let Some(selected) = explorer_state.selected() {
                                    if selected < explorer_items.len() {
                                        match &explorer_items[selected].node_type {
                                            NodeType::Database { is_active, .. } => {
                                                if *is_active {
                                                    status_msg = "⚠️ Cannot detach the primary active database.".to_string();
                                                } else {
                                                    let db_name = explorer_items[selected]
                                                        .name
                                                        .strip_prefix("db:")
                                                        .unwrap_or(&explorer_items[selected].name);
                                                    match conn.execute(
                                                        &format!("DETACH {};", db_name),
                                                        [],
                                                    ) {
                                                        Ok(_) => {
                                                            status_msg = format!(
                                                                "Detached database '{}' successfully.",
                                                                db_name
                                                            );
                                                            trigger_explorer_refresh = true;
                                                        }
                                                        Err(e) => {
                                                            status_msg = format!(
                                                                "Failed to detach '{}': {}",
                                                                db_name, e
                                                            );
                                                        }
                                                    }
                                                }
                                            }
                                            NodeType::CloudSecret { name, .. } => {
                                                match conn.execute(
                                                    &format!("DROP SECRET IF EXISTS {};", name),
                                                    [],
                                                ) {
                                                    Ok(_) => {
                                                        status_msg = format!(
                                                            "Dropped secret '{}' successfully.",
                                                            name
                                                        );
                                                        trigger_explorer_refresh = true;
                                                    }
                                                    Err(e) => {
                                                        status_msg = format!(
                                                            "Failed to drop secret '{}': {}",
                                                            name, e
                                                        );
                                                    }
                                                }
                                            }
                                            NodeType::CloudBucket { bucket } => {
                                                let current = std::env::var("DUCKPACK_S3_BUCKETS")
                                                    .or_else(|_| {
                                                        std::env::var("DUCKPACK_S3_BUCKET")
                                                    })
                                                    .unwrap_or_default();
                                                let parts: Vec<&str> = current
                                                    .split(',')
                                                    .map(|s| s.trim())
                                                    .filter(|s| {
                                                        !s.is_empty()
                                                            && *s != bucket.as_str()
                                                            && *s
                                                                != bucket
                                                                    .trim_start_matches("s3://")
                                                    })
                                                    .collect();
                                                let new_val = parts.join(",");
                                                unsafe {
                                                    std::env::set_var(
                                                        "DUCKPACK_S3_BUCKETS",
                                                        &new_val,
                                                    );
                                                    std::env::set_var(
                                                        "DUCKPACK_S3_BUCKET",
                                                        &new_val,
                                                    );
                                                }
                                                status_msg = format!(
                                                    "Removed bucket '{}' from session view.",
                                                    bucket
                                                );
                                                trigger_explorer_refresh = true;
                                            }
                                            _ => {}
                                        }
                                    }
                                }
                            } else if key.code == KeyCode::Char('e')
                                || key.code == KeyCode::Char('E')
                            {
                                if let Some(selected) = explorer_state.selected() {
                                    if selected < explorer_items.len() {
                                        match &explorer_items[selected].node_type {
                                            NodeType::CloudSecret { name, .. } => {
                                                populate_secret_modal_for_edit(
                                                    conn,
                                                    project_dir,
                                                    name,
                                                    &mut secret_modal,
                                                );
                                                status_msg =
                                                    format!("Editing cloud secret '{}'", name);
                                            }
                                            NodeType::CloudBucket { bucket } => {
                                                add_bucket_modal.is_open = true;
                                                add_bucket_modal.initial_uri = Some(bucket.clone());
                                                add_bucket_modal.uri = bucket.clone();
                                                add_bucket_modal.active_field = 0;
                                                add_bucket_modal.error_msg = None;
                                                status_msg =
                                                    format!("Editing bucket URI '{}'", bucket);
                                            }
                                            _ => {}
                                        }
                                    }
                                }
                            }
                        }
                        FocusPane::Editor => {
                            if key.code == KeyCode::Char(' ')
                                && key.modifiers.contains(KeyModifiers::CONTROL)
                            {
                                let cursor = tabs[active_tab_index].textarea.cursor();
                                if cursor.0 < tabs[active_tab_index].textarea.lines().len() {
                                    let line = &tabs[active_tab_index].textarea.lines()[cursor.0];
                                    if cursor.1 <= line.chars().count() {
                                        let char_idx = cursor.1;
                                        let prefix: String = line.chars().take(char_idx).collect();

                                        let word_start = prefix
                                            .rfind(|c: char| {
                                                !c.is_alphanumeric() && c != '_' && c != '.'
                                            })
                                            .map(|i| i + 1)
                                            .unwrap_or(0);
                                        let current_word = &prefix[word_start..];

                                        if !current_word.is_empty() {
                                            let mut is_cycling = false;
                                            if !autocomplete_matches.is_empty()
                                                && current_word
                                                    == autocomplete_matches[autocomplete_index]
                                            {
                                                is_cycling = true;
                                            }

                                            if is_cycling {
                                                autocomplete_index = (autocomplete_index + 1)
                                                    % autocomplete_matches.len();
                                            } else {
                                                autocomplete_original_word =
                                                    current_word.to_string();
                                                let mut matches: Vec<String> = dictionary
                                                    .iter()
                                                    .filter(|d| {
                                                        d.to_lowercase().starts_with(
                                                            &autocomplete_original_word
                                                                .to_lowercase(),
                                                        )
                                                    })
                                                    .cloned()
                                                    .collect();
                                                let target_db =
                                                    &tabs[active_tab_index].target_database;
                                                if !target_db.is_empty() {
                                                    let target_prefix =
                                                        format!("{}.", target_db.to_lowercase());
                                                    let target_entities: HashSet<String> =
                                                        if let Some(d) = meta
                                                            .databases
                                                            .iter()
                                                            .find(|d| d.name == *target_db)
                                                        {
                                                            let mut set = HashSet::new();
                                                            set.insert(d.name.to_lowercase());
                                                            for t in &d.tables {
                                                                set.insert(t.name.to_lowercase());
                                                                set.insert(
                                                                    t.display_name().to_lowercase(),
                                                                );
                                                            }
                                                            for v in &d.views {
                                                                set.insert(v.name.to_lowercase());
                                                                set.insert(
                                                                    v.display_name().to_lowercase(),
                                                                );
                                                            }
                                                            set
                                                        } else {
                                                            HashSet::new()
                                                        };

                                                    matches.sort_by(|a, b| {
                                                        let a_low = a.to_lowercase();
                                                        let b_low = b.to_lowercase();
                                                        let a_score = if a_low
                                                            .starts_with(&target_prefix)
                                                            || target_entities.contains(&a_low)
                                                        {
                                                            0
                                                        } else {
                                                            1
                                                        };
                                                        let b_score = if b_low
                                                            .starts_with(&target_prefix)
                                                            || target_entities.contains(&b_low)
                                                        {
                                                            0
                                                        } else {
                                                            1
                                                        };
                                                        a_score
                                                            .cmp(&b_score)
                                                            .then_with(|| a.len().cmp(&b.len()))
                                                            .then_with(|| a.cmp(b))
                                                    });
                                                }
                                                autocomplete_matches = matches;
                                                autocomplete_index = 0;
                                            }

                                            if !autocomplete_matches.is_empty() {
                                                let match_str =
                                                    &autocomplete_matches[autocomplete_index];
                                                for _ in 0..current_word.chars().count() {
                                                    tabs[active_tab_index].textarea.delete_char();
                                                }
                                                tabs[active_tab_index]
                                                    .textarea
                                                    .insert_str(match_str);
                                                tabs[active_tab_index].is_dirty = true;
                                                status_msg = format!(
                                                    "Autocomplete: {} ({}/{})",
                                                    match_str,
                                                    autocomplete_index + 1,
                                                    autocomplete_matches.len()
                                                );
                                            } else {
                                                status_msg = format!(
                                                    "No autocomplete matches for '{}'",
                                                    autocomplete_original_word
                                                );
                                            }
                                        }
                                    }
                                }
                            } else if tabs[active_tab_index].textarea.input(key) {
                                tabs[active_tab_index].is_dirty = true;
                            }
                        }
                        FocusPane::Results => {
                            let active_tab = &mut tabs[active_tab_index];
                            if key.code == KeyCode::Up {
                                if let Some(selected) = active_tab.results_state.selected() {
                                    if selected > 0 {
                                        active_tab.results_state.select(Some(selected - 1));
                                    }
                                }
                            } else if key.code == KeyCode::Down {
                                let len = active_tab.query_results.len();
                                if len > 0 {
                                    let selected = active_tab.results_state.selected().unwrap_or(0);
                                    if selected < len.saturating_sub(1) {
                                        active_tab.results_state.select(Some(selected + 1));
                                    } else if active_tab.results_state.selected().is_none() {
                                        active_tab.results_state.select(Some(0));
                                    }
                                }
                            } else if key.code == KeyCode::PageDown {
                                if key.modifiers.contains(KeyModifiers::SHIFT) {
                                    if active_tab.pagination_limit > 0
                                        && active_tab.query_results.len()
                                            >= active_tab.pagination_limit
                                    {
                                        trigger_pagination = Some(1);
                                    }
                                } else {
                                    let len = active_tab.query_results.len();
                                    if len > 0 {
                                        let selected =
                                            active_tab.results_state.selected().unwrap_or(0);
                                        let new_sel = (selected + 10).min(len.saturating_sub(1));
                                        active_tab.results_state.select(Some(new_sel));
                                    }
                                }
                            } else if key.code == KeyCode::Char('n')
                                || key.code == KeyCode::Char('N')
                            {
                                if active_tab.pagination_limit > 0
                                    && active_tab.query_results.len() >= active_tab.pagination_limit
                                {
                                    trigger_pagination = Some(1);
                                }
                            } else if key.code == KeyCode::Char('p')
                                || key.code == KeyCode::Char('P')
                            {
                                if active_tab.pagination_limit > 0 {
                                    trigger_pagination = Some(-1);
                                }
                            } else if key.code == KeyCode::Char('l')
                                || key.code == KeyCode::Char('L')
                            {
                                active_tab.cycle_pagination_limit();
                                status_msg = format!(
                                    "Results page limit set to {} rows",
                                    active_tab.pagination_limit_label()
                                );
                                if active_tab.base_query.is_some() {
                                    trigger_pagination = Some(0);
                                }
                            } else if key.code == KeyCode::PageUp {
                                if key.modifiers.contains(KeyModifiers::SHIFT) {
                                    if active_tab.pagination_limit > 0 {
                                        trigger_pagination = Some(-1);
                                    }
                                } else if let Some(selected) = active_tab.results_state.selected() {
                                    let new_sel = selected.saturating_sub(10);
                                    active_tab.results_state.select(Some(new_sel));
                                }
                            } else if key.code == KeyCode::Left {
                                if key.modifiers.contains(KeyModifiers::SHIFT)
                                    || key.modifiers.contains(KeyModifiers::CONTROL)
                                {
                                    active_tab.horizontal_scroll =
                                        active_tab.horizontal_scroll.saturating_sub(5);
                                } else {
                                    active_tab.horizontal_scroll =
                                        active_tab.horizontal_scroll.saturating_sub(1);
                                }
                            } else if key.code == KeyCode::Right {
                                let max_col = active_tab.column_names.len().saturating_sub(1);
                                if key.modifiers.contains(KeyModifiers::SHIFT)
                                    || key.modifiers.contains(KeyModifiers::CONTROL)
                                {
                                    active_tab.horizontal_scroll =
                                        (active_tab.horizontal_scroll + 5).min(max_col);
                                } else if active_tab.horizontal_scroll < max_col {
                                    active_tab.horizontal_scroll += 1;
                                }
                            } else if key.code == KeyCode::Home {
                                active_tab.horizontal_scroll = 0;
                            } else if key.code == KeyCode::End {
                                active_tab.horizontal_scroll =
                                    active_tab.column_names.len().saturating_sub(1);
                            }
                        }
                    }
                }
            }
            _ => {}
        }

        if new_tab {
            tabs.push(EditorTab::new(
                tabs[active_tab_index].target_database.clone(),
            ));
            active_tab_index = tabs.len() - 1;
            status_msg = "New tab opened".to_string();
        }

        if close_tab {
            tabs.remove(active_tab_index);
            if tabs.is_empty() {
                tabs.push(EditorTab::new(meta.active_database.clone()));
            }
            if active_tab_index >= tabs.len() {
                active_tab_index = tabs.len() - 1;
            }
            status_msg = "Closed tab".to_string();
        }

        if format_query {
            let query = tabs[active_tab_index].textarea.lines().join("\n");
            let formatted = sqlformat::format(
                &query,
                &sqlformat::QueryParams::None,
                &sqlformat::FormatOptions::default(),
            );
            let active_file = tabs[active_tab_index].active_file.clone();
            tabs[active_tab_index].load_content(&formatted, active_file);
            tabs[active_tab_index].is_dirty = true;
            status_msg = "Formatted SQL".to_string();
        }

        if save_query {
            let content = tabs[active_tab_index].textarea.lines().join("\n");
            if let Some(path) = tabs[active_tab_index].active_file.clone() {
                if let Err(e) = std::fs::write(&path, content) {
                    status_msg = format!("Error saving file: {}", e);
                } else {
                    tabs[active_tab_index].is_dirty = false;
                    status_msg = format!("Saved {}", path.display());
                    trigger_explorer_refresh = true;
                }
            } else {
                let name = format!(
                    "query_{}.sql",
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs()
                );
                let path = queries_dir.join(&name);
                if let Err(e) = std::fs::write(&path, content) {
                    status_msg = format!("Error saving new file: {}", e);
                } else {
                    tabs[active_tab_index].active_file = Some(path.clone());
                    tabs[active_tab_index].is_dirty = false;
                    status_msg = format!("Saved as {}", name);
                    trigger_explorer_refresh = true;
                }
            }
        }

        if export_query {
            let active_tab = &tabs[active_tab_index];
            if active_tab.query_results.is_empty() {
                status_msg = "No results to export!".to_string();
            } else {
                let name = format!(
                    "export_{}.csv",
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs()
                );
                let path = queries_dir.join(&name);
                let mut csv_content = String::new();
                csv_content.push_str(&active_tab.column_names.join(","));
                csv_content.push('\n');
                for row in &active_tab.query_results {
                    let escaped_row: Vec<String> = row
                        .iter()
                        .map(|c| {
                            if c.contains(',') || c.contains('"') || c.contains('\n') {
                                format!("\"{}\"", c.replace("\"", "\"\""))
                            } else {
                                c.clone()
                            }
                        })
                        .collect();
                    csv_content.push_str(&escaped_row.join(","));
                    csv_content.push('\n');
                }
                if let Err(e) = std::fs::write(&path, csv_content) {
                    status_msg = format!("Error exporting CSV: {}", e);
                } else {
                    status_msg = format!("Results exported to {}", name);
                }
            }
        }

        if execute_query || trigger_pagination.is_some() {
            let active_tab = &mut tabs[active_tab_index];
            if !active_tab.target_database.is_empty() {
                let _ = conn.execute(
                    &format!(
                        "USE \"{}\";",
                        active_tab.target_database.replace('"', "\"\"")
                    ),
                    [],
                );
                meta.active_database = active_tab.target_database.clone();
                for db in &mut meta.databases {
                    db.is_active = db.name == active_tab.target_database;
                }
            }

            let query_to_execute = if let Some(dir) = trigger_pagination {
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
            };

            if query_to_execute.trim().is_empty() {
                if trigger_pagination.is_none() {
                    status_msg = "Error: Query is empty".to_string();
                }
            } else {
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
                let (exec_query, is_paginatable, is_mutation) = if is_explicit_remote {
                    let (eq, is_pag) = wrap_remote_query(&query_to_execute);
                    (eq, is_pag, false)
                } else {
                    let mut clean_lower = query_to_execute.trim().to_lowercase();
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

                if is_mutation {
                    active_tab.is_paginated = false;
                    let start_time = std::time::Instant::now();
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
                            status_msg =
                                format!("Statement executed successfully in {:.2?}.", elapsed);

                            let history_path = queries_dir.join(".history.sql");
                            let timestamp =
                                chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
                            let history_entry = format!(
                                "-- Executed at {} (Took {:.2?})\n{};\n\n",
                                timestamp,
                                elapsed,
                                query_to_execute.trim()
                            );
                            use std::io::Write;
                            if let Ok(mut f) = std::fs::OpenOptions::new()
                                .create(true)
                                .append(true)
                                .open(history_path)
                            {
                                let _ = f.write_all(history_entry.as_bytes());
                            }
                        }
                        Err(e) => {
                            let err_str = e.to_string();
                            handle_sql_error_position(
                                &mut tabs[active_tab_index].textarea,
                                &err_str,
                            );
                            let diag = diagnose_remote_credential_error(&err_str, is_remote_db);
                            status_msg = format!("Execution Error: {}{}", e, diag);
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
                            let mut exec_rows = Vec::new();
                            let mut count = 0;

                            let start_time = std::time::Instant::now();
                            match stmt.query([]) {
                                Ok(mut rows) => {
                                    let mut cols = Vec::new();
                                    if let Some(stmt_ref) = rows.as_ref() {
                                        for col in stmt_ref.column_names() {
                                            cols.push(col.to_string());
                                        }
                                    }
                                    active_tab.column_names = cols;

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
                                    let elapsed = start_time.elapsed();

                                    let lower = query_to_execute.to_lowercase();
                                    let is_catalog_mutation = lower.contains("attach ")
                                        || lower.contains("detach ")
                                        || lower.contains("create ")
                                        || lower.contains("drop ")
                                        || lower.contains("alter ")
                                        || lower.contains("use ");

                                    if is_explicit_remote {
                                        if is_catalog_mutation {
                                            refresh_attached_catalog(
                                                conn,
                                                db_path,
                                                quack_disable_ssl,
                                            );
                                            trigger_explorer_refresh = true;
                                        }
                                        let hint = " (Hint: '-- @remote' is deprecated in DuckDB v2.0; queries now execute natively via CONNECT target_db.)";
                                        if count > 0 {
                                            status_msg = format!(
                                                "Remote statement executed successfully. Returned {} rows in {:.2?}.{}",
                                                count, elapsed, hint
                                            );
                                        } else {
                                            status_msg = format!(
                                                "Remote statement executed successfully in {:.2?}.{}",
                                                elapsed, hint
                                            );
                                        }
                                    } else if is_remote_db
                                        && is_unadorned_secret_or_setting(&query_to_execute)
                                    {
                                        status_msg = "⚠️ Applied LOCALLY on DuckPack! Use '-- @remote' at top of query to apply to remote Quack.".to_string();
                                    } else {
                                        if is_catalog_mutation {
                                            trigger_explorer_refresh = true;
                                        }
                                        status_msg = format!(
                                            "Query executed successfully. Returned {} rows in {:.2?}.",
                                            count, elapsed
                                        );
                                    }

                                    let history_path = queries_dir.join(".history.sql");
                                    let timestamp = chrono::Local::now()
                                        .format("%Y-%m-%d %H:%M:%S")
                                        .to_string();
                                    let history_entry = format!(
                                        "-- Executed at {} (Took {:.2?})\n{};\n\n",
                                        timestamp,
                                        elapsed,
                                        query_to_execute.trim()
                                    );
                                    use std::io::Write;
                                    if let Ok(mut f) = std::fs::OpenOptions::new()
                                        .create(true)
                                        .append(true)
                                        .open(history_path)
                                    {
                                        let _ = f.write_all(history_entry.as_bytes());
                                    }
                                }
                                Err(e) => {
                                    let err_str = e.to_string();
                                    handle_sql_error_position(
                                        &mut tabs[active_tab_index].textarea,
                                        &err_str,
                                    );
                                    let diag =
                                        diagnose_remote_credential_error(&err_str, is_remote_db);
                                    status_msg = format!("Execution Error: {}{}", e, diag);
                                }
                            }
                        }
                        Err(e) => {
                            let mut executed_remote_fallback = false;
                            let local_err_str = e.to_string();
                            let is_syntax_error = local_err_str.contains("syntax error at or near");
                            let lower_q = query_to_execute.to_lowercase();
                            let is_temp_mutation = lower_q.contains("temp table")
                                || lower_q.contains("temporary table")
                                || lower_q.contains("temp view")
                                || lower_q.contains("temporary view");
                            if is_quack
                                && !is_explicit_remote
                                && !is_syntax_error
                                && !is_temp_mutation
                            {
                                let clean_query = query_to_execute
                                    .trim()
                                    .strip_suffix(';')
                                    .unwrap_or(query_to_execute.trim());
                                let remote_sql = if is_paginatable {
                                    let inner = format!(
                                        "SELECT * FROM target_db.query('{}')",
                                        clean_query.replace("'", "''")
                                    );
                                    if active_tab.is_paginated && active_tab.pagination_limit > 0 {
                                        format!(
                                            "{} LIMIT {} OFFSET {}",
                                            inner,
                                            active_tab.pagination_limit,
                                            active_tab.pagination_offset
                                        )
                                    } else {
                                        format!("{};", inner)
                                    }
                                } else {
                                    format!(
                                        "FROM target_db.query('{}');",
                                        clean_query.replace("'", "''")
                                    )
                                };
                                match conn.prepare(&remote_sql) {
                                    Ok(mut remote_stmt) => {
                                        let start_time = std::time::Instant::now();
                                        match remote_stmt.query([]) {
                                            Ok(mut rows) => {
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
                                                let elapsed = start_time.elapsed();

                                                let lower = query_to_execute.to_lowercase();
                                                let is_catalog_mutation = lower.contains("attach ")
                                                    || lower.contains("detach ")
                                                    || lower.contains("create ")
                                                    || lower.contains("drop ")
                                                    || lower.contains("alter ")
                                                    || lower.contains("use ");
                                                if is_catalog_mutation {
                                                    refresh_attached_catalog(
                                                        conn,
                                                        db_path,
                                                        quack_disable_ssl,
                                                    );
                                                    trigger_explorer_refresh = true;
                                                }
                                                if count > 0 {
                                                    status_msg = format!(
                                                        "Remote statement executed successfully. Returned {} rows in {:.2?}.",
                                                        count, elapsed
                                                    );
                                                } else {
                                                    status_msg = format!(
                                                        "Remote statement executed successfully in {:.2?}.",
                                                        elapsed
                                                    );
                                                }
                                                executed_remote_fallback = true;

                                                let history_path = queries_dir.join(".history.sql");
                                                let timestamp = chrono::Local::now()
                                                    .format("%Y-%m-%d %H:%M:%S")
                                                    .to_string();
                                                let history_entry = format!(
                                                    "-- Executed at {} (Took {:.2?})\n{};\n\n",
                                                    timestamp,
                                                    elapsed,
                                                    query_to_execute.trim()
                                                );
                                                use std::io::Write;
                                                if let Ok(mut f) = std::fs::OpenOptions::new()
                                                    .create(true)
                                                    .append(true)
                                                    .open(history_path)
                                                {
                                                    let _ = f.write_all(history_entry.as_bytes());
                                                }
                                            }
                                            Err(query_err) => {
                                                let err_str = query_err.to_string();
                                                let diag = diagnose_remote_credential_error(
                                                    &err_str,
                                                    is_remote_db,
                                                );
                                                status_msg = format!(
                                                    "Execution Error: {}{}",
                                                    query_err, diag
                                                );
                                                executed_remote_fallback = true;
                                            }
                                        }
                                    }
                                    Err(prep_err) => {
                                        let err_str = prep_err.to_string();
                                        let diag = diagnose_remote_credential_error(
                                            &err_str,
                                            is_remote_db,
                                        );
                                        status_msg = format!("Prepare Error: {}{}", prep_err, diag);
                                        executed_remote_fallback = true;
                                    }
                                }
                            }

                            if !executed_remote_fallback {
                                let err_str = e.to_string();
                                handle_sql_error_position(
                                    &mut tabs[active_tab_index].textarea,
                                    &err_str,
                                );
                                let diag = diagnose_remote_credential_error(&err_str, is_remote_db);
                                status_msg = format!("Prepare Error: {}{}", e, diag);
                            }
                        }
                    }
                }
            }
        }

        if trigger_explorer_refresh {
            let prev_selected = explorer_state.selected();
            if explorer_mode == ExplorerMode::Project {
                let (b, st) = get_git_branch_and_status(project_dir);
                git_branch = b;
                git_status = st;
                explorer_items =
                    rebuild_project_nodes(project_dir, &expanded_project_groups, &git_status);
            } else {
                meta = load_explorer_metadata(conn, project_dir, is_quack);
                dictionary = meta.dictionary.clone();
                autocomplete_matches.clear();
                if !meta.queries.is_empty() {
                    expanded_groups.insert("Queries".to_string());
                }
                if meta.databases.len() > 1 {
                    for db in &meta.databases {
                        if db.is_active {
                            expanded_groups.insert(format!("db:{}", db.name));
                            expanded_groups.insert(format!("{}:Tables", db.name));
                        }
                    }
                } else {
                    expanded_groups.insert("Tables".to_string());
                }
                explorer_items = rebuild_explorer_nodes(
                    &meta,
                    &expanded_groups,
                    &expanded_tables,
                    conn,
                    is_quack,
                );
            }
            if let Some(sel) = prev_selected {
                explorer_state.select(Some(sel.min(explorer_items.len().saturating_sub(1))));
            } else if !explorer_items.is_empty() {
                explorer_state.select(Some(0));
            }
        }

        if trigger_explorer_action {
            if let Some(selected) = explorer_state.selected() {
                if selected < explorer_items.len() {
                    let item_type = explorer_items[selected].node_type.clone();
                    let item_name = explorer_items[selected].name.clone();

                    match item_type {
                        NodeType::Database { .. } => {
                            let clean_db_name =
                                if let Some(stripped) = item_name.strip_prefix("db:") {
                                    stripped.to_string()
                                } else {
                                    item_name.clone()
                                };
                            tabs[active_tab_index].target_database = clean_db_name.clone();
                            let _ = conn.execute(
                                &format!("USE \"{}\";", clean_db_name.replace('"', "\"\"")),
                                [],
                            );
                            meta.active_database = clean_db_name.clone();
                            for db in &mut meta.databases {
                                db.is_active = db.name == clean_db_name;
                            }
                            status_msg = format!(
                                "Target database switched to '{}' for active tab.",
                                clean_db_name
                            );

                            if expanded_groups.contains(&item_name) {
                                expanded_groups.remove(&item_name);
                            } else {
                                expanded_groups.insert(item_name);
                            }
                            explorer_items = rebuild_explorer_nodes(
                                &meta,
                                &expanded_groups,
                                &expanded_tables,
                                conn,
                                is_quack,
                            );
                            explorer_state
                                .select(Some(selected.min(explorer_items.len().saturating_sub(1))));
                        }
                        NodeType::Group => {
                            if explorer_mode == ExplorerMode::Project {
                                if item_name.ends_with(":empty") {
                                    new_asset_modal.reset();
                                    if item_name.contains("tables") {
                                        new_asset_modal.category_index = 0;
                                    } else if item_name.contains("views") {
                                        new_asset_modal.category_index = 1;
                                    } else if item_name.contains("macros") {
                                        new_asset_modal.category_index = 2;
                                    } else if item_name.contains("triggers") {
                                        new_asset_modal.category_index = 3;
                                    } else if item_name.contains("schemas") {
                                        new_asset_modal.category_index = 4;
                                    } else if item_name.contains("queries") {
                                        new_asset_modal.category_index = 5;
                                    } else if item_name.contains("seeds") {
                                        new_asset_modal.category_index = 6;
                                    }
                                } else {
                                    if expanded_project_groups.contains(&item_name) {
                                        expanded_project_groups.remove(&item_name);
                                    } else {
                                        expanded_project_groups.insert(item_name);
                                    }
                                    explorer_items = rebuild_project_nodes(
                                        project_dir,
                                        &expanded_project_groups,
                                        &git_status,
                                    );
                                    explorer_state.select(Some(
                                        selected.min(explorer_items.len().saturating_sub(1)),
                                    ));
                                }
                            } else if item_name.contains(":error:") {
                                if let Some((_, err_text)) = item_name.split_once(":error:") {
                                    status_msg = format!("⚠️ Cloud Bucket Error: {}", err_text);
                                }
                            } else if item_name.ends_with(":empty") {
                                status_msg =
                                    "Bucket or directory is empty (0 matching objects found)."
                                        .to_string();
                            } else {
                                if expanded_groups.contains(&item_name) {
                                    expanded_groups.remove(&item_name);
                                } else {
                                    expanded_groups.insert(item_name);
                                }
                                explorer_items = rebuild_explorer_nodes(
                                    &meta,
                                    &expanded_groups,
                                    &expanded_tables,
                                    conn,
                                    is_quack,
                                );
                                explorer_state.select(Some(
                                    selected.min(explorer_items.len().saturating_sub(1)),
                                ));
                            }
                        }
                        NodeType::Table {
                            database,
                            schema,
                            is_temporary: _,
                        }
                        | NodeType::View {
                            database,
                            schema,
                            is_temporary: _,
                        } => {
                            let key = match (&database, &schema) {
                                (Some(db), Some(sch)) if meta.databases.len() > 1 => {
                                    format!("{}.{}.{}", db, sch, item_name)
                                }
                                _ => item_name.clone(),
                            };
                            if expanded_tables.contains(&key)
                                || expanded_tables.contains(&item_name)
                            {
                                expanded_tables.remove(&key);
                                expanded_tables.remove(&item_name);
                            } else {
                                expanded_tables.insert(key);
                            }
                            explorer_items = rebuild_explorer_nodes(
                                &meta,
                                &expanded_groups,
                                &expanded_tables,
                                conn,
                                is_quack,
                            );
                            explorer_state
                                .select(Some(selected.min(explorer_items.len().saturating_sub(1))));
                        }
                        NodeType::Query => {
                            let path = queries_dir.join(&item_name);
                            if open_or_focus_file_in_tabs(&mut tabs, &mut active_tab_index, path) {
                                focus = FocusPane::Editor;
                                status_msg = format!("Loaded {}", item_name);
                            }
                        }
                        NodeType::ProjectFile { path, .. } => {
                            let fname = path
                                .file_name()
                                .unwrap_or_default()
                                .to_string_lossy()
                                .to_string();
                            if open_or_focus_file_in_tabs(&mut tabs, &mut active_tab_index, path) {
                                focus = FocusPane::Editor;
                                status_msg = format!("Loaded {}", fname);
                            }
                        }
                        NodeType::Column { .. } => {}
                        NodeType::Macro => {}
                        NodeType::Trigger { .. } => {}
                        NodeType::AddSecretAction => {
                            secret_modal.reset();
                        }
                        NodeType::AddBucketAction => {
                            add_bucket_modal.reset();
                        }
                        NodeType::CloudSecret { name, .. } => {
                            let query =
                                format!("SELECT * FROM duckdb_secrets() WHERE name = '{}';", name);
                            open_query_in_tabs(&mut tabs, &mut active_tab_index, query, None);
                            focus = FocusPane::Editor;
                            status_msg = format!("Inspecting secret '{}'", name);
                        }
                        NodeType::CloudBucket { bucket } => {
                            let key = format!("bucket:{}", bucket);
                            if expanded_groups.contains(&key) {
                                expanded_groups.remove(&key);
                            } else {
                                expanded_groups.insert(key);
                            }
                            explorer_items = rebuild_explorer_nodes(
                                &meta,
                                &expanded_groups,
                                &expanded_tables,
                                conn,
                                is_quack,
                            );
                            explorer_state
                                .select(Some(selected.min(explorer_items.len().saturating_sub(1))));
                        }
                        NodeType::CloudFile { path, is_dir } => {
                            if is_dir {
                                let key = format!("bucket_dir:{}", path);
                                if expanded_groups.contains(&key) {
                                    expanded_groups.remove(&key);
                                } else {
                                    expanded_groups.insert(key);
                                }
                                explorer_items = rebuild_explorer_nodes(
                                    &meta,
                                    &expanded_groups,
                                    &expanded_tables,
                                    conn,
                                    is_quack,
                                );
                                explorer_state.select(Some(
                                    selected.min(explorer_items.len().saturating_sub(1)),
                                ));
                            } else {
                                let ext = path.rsplit('.').next().unwrap_or("").to_lowercase();
                                let read_fn = match ext.as_str() {
                                    "parquet" => "read_parquet",
                                    "csv" => "read_csv",
                                    "json" | "jsonl" | "ndjson" => "read_json",
                                    _ => "read_parquet",
                                };
                                let query =
                                    format!("SELECT * FROM {}('{}');", read_fn, path);
                                let fname = path.split('/').last().unwrap_or(&path).to_string();
                                open_query_in_tabs(&mut tabs, &mut active_tab_index, query, None);
                                focus = FocusPane::Editor;
                                status_msg = format!("Queried cloud file '{}'", fname);
                            }
                        }
                    }
                }
            }
        }
    }

    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    use duckdb::types::Value;
    use std::collections::HashMap;

    #[test]
    fn test_format_duckdb_value() {
        assert_eq!(format_duckdb_value(Value::Null), "NULL");
        assert_eq!(format_duckdb_value(Value::Boolean(true)), "true");
        assert_eq!(format_duckdb_value(Value::Int(42)), "42");
        assert_eq!(
            format_duckdb_value(Value::Text("hello".to_string())),
            "hello"
        );
    }

    #[test]
    fn test_wide_table_column_windowing() {
        let cols: Vec<String> = (0..217).map(|i| format!("col_{}", i)).collect();
        let rows: Vec<Vec<String>> = vec![(0..217).map(|i| format!("val_{}", i)).collect()];

        let avail_width: usize = 80;
        let mut visible = Vec::new();
        let mut used = 0;
        for i in 0..cols.len() {
            let col_w = (cols[i].len().max(rows[0][i].len()) + 2).clamp(6, 30);
            if visible.is_empty() || used + col_w <= avail_width {
                used += col_w;
                visible.push(i);
            } else {
                break;
            }
        }
        assert!(!visible.is_empty());
        assert!(visible.len() < cols.len());
        assert_eq!(visible[0], 0);
        assert!(used <= avail_width);
    }

    #[test]
    fn test_duckdb_detach() {
        let conn = duckdb::Connection::open_in_memory().unwrap();
        conn.execute("ATTACH ':memory:' AS target_db;", []).unwrap();
        conn.execute("USE target_db;", []).unwrap();
        // Switch to memory first, then detach
        conn.execute("USE memory;", []).unwrap();
        conn.execute("DETACH target_db;", []).unwrap();
        conn.execute("ATTACH ':memory:' AS target_db;", []).unwrap();
        conn.execute("USE target_db;", []).unwrap();
    }

    #[test]
    fn test_calculate_status_height() {
        // Single line short status
        assert_eq!(calculate_status_height("Ready.", 80, 30), 3);

        // Long status that wraps to 2 lines
        let long_msg = "A".repeat(100);
        assert_eq!(calculate_status_height(&long_msg, 80, 30), 4);

        // Multi-line status with newlines
        let multi_line = "Error: Line 1\nLine 2\nLine 3";
        assert_eq!(calculate_status_height(multi_line, 80, 30), 5);

        // Respects maximum limit clamped to total_height / 3 or 8
        let huge_msg = "Error:\n".repeat(20);
        assert_eq!(calculate_status_height(&huge_msg, 80, 30), 8);
    }

    #[test]
    fn test_clipboard_roundtrip() {
        let mut cb: Option<arboard::Clipboard> = None;
        let test_str = "duckpack_test_clipboard_12345";
        assert!(set_clipboard_text(test_str, &mut cb));
        let retrieved = get_clipboard_text(&mut cb);
        if let Some(ref text) = retrieved {
            assert_eq!(text, test_str);
        }
    }

    #[test]
    fn test_load_explorer_metadata_single_and_attached() {
        let conn = duckdb::Connection::open_in_memory().unwrap();
        conn.execute("CREATE TABLE users (id INTEGER, name VARCHAR);", [])
            .unwrap();
        conn.execute("CREATE VIEW active_users AS SELECT * FROM users;", [])
            .unwrap();

        let temp_dir = std::env::temp_dir().join(format!(
            "dp_test_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&temp_dir).unwrap();

        // 1. Single DB mode
        let meta_single = load_explorer_metadata(&conn, &temp_dir, false);
        assert_eq!(meta_single.databases.len(), 1);
        assert_eq!(meta_single.tables, vec!["users".to_string()]);
        assert_eq!(meta_single.views, vec!["active_users".to_string()]);

        // Rebuild single DB nodes: flat layout
        let mut expanded_groups = std::collections::HashSet::new();
        let mut expanded_tables = std::collections::HashSet::new();
        expanded_groups.insert("Tables".to_string());
        let nodes_single = rebuild_explorer_nodes(
            &meta_single,
            &expanded_groups,
            &expanded_tables,
            &conn,
            false,
        );
        assert!(
            nodes_single
                .iter()
                .any(|n| n.display.contains("Tables (1)"))
        );
        assert!(nodes_single.iter().any(|n| n.name == "users"));

        // 2. Attach a second database
        let attached_db_path = temp_dir.join("analytics.duckdb");
        conn.execute(
            &format!("ATTACH '{}' AS analytics;", attached_db_path.display()),
            [],
        )
        .unwrap();
        conn.execute(
            "CREATE TABLE analytics.events (event_id BIGINT, event_name VARCHAR);",
            [],
        )
        .unwrap();

        let meta_multi = load_explorer_metadata(&conn, &temp_dir, false);
        assert_eq!(meta_multi.databases.len(), 2);
        assert!(
            meta_multi
                .databases
                .iter()
                .any(|d| d.name == "memory" && d.is_active)
        );
        assert!(
            meta_multi
                .databases
                .iter()
                .any(|d| d.name == "analytics" && !d.is_active)
        );

        // Rebuild multi-DB nodes: catalog-aware hierarchy
        expanded_groups.clear();
        expanded_tables.clear();
        expanded_groups.insert("db:analytics".to_string());
        expanded_groups.insert("analytics:Tables".to_string());
        expanded_tables.insert("analytics.main.events".to_string());

        let nodes_multi = rebuild_explorer_nodes(
            &meta_multi,
            &expanded_groups,
            &expanded_tables,
            &conn,
            false,
        );
        assert!(
            nodes_multi
                .iter()
                .any(|n| n.name == "db:analytics" && n.display.contains("analytics"))
        );
        assert!(nodes_multi.iter().any(|n| n.name == "analytics:Tables"));
        assert!(nodes_multi.iter().any(|n| n.name == "events"));
        assert!(nodes_multi.iter().any(|n| n.name == "event_id"));

        // 3. Schema visualizer with attached database
        let mut cb = None;
        let visualizer_result = generate_schema_visualizer(&conn, &mut cb, false);
        assert!(visualizer_result.is_ok());
        let tree_text = visualizer_result.unwrap();
        assert!(tree_text.contains("📁 memory"));
        assert!(tree_text.contains("📁 analytics"));
        assert!(tree_text.contains("events"));

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_quack_attach_preserves_target_db_and_memory() {
        let conn = duckdb::Connection::open_in_memory().unwrap();
        conn.execute("ATTACH ':memory:' AS target_db;", []).unwrap();
        conn.execute(
            "CREATE TABLE target_db.remote_tbl (col1 INTEGER, col2 VARCHAR);",
            [],
        )
        .unwrap();
        conn.execute("USE target_db;", []).unwrap();

        let temp_dir = std::env::temp_dir().join(format!(
            "dp_test_quack_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&temp_dir).unwrap();

        // When target_db is attached (e.g. Quack attach), both target_db and empty memory are preserved
        let meta = load_explorer_metadata(&conn, &temp_dir, false);
        assert_eq!(meta.databases.len(), 2);
        assert!(
            meta.databases
                .iter()
                .any(|d| d.name == "target_db" && d.is_active)
        );
        assert!(
            meta.databases
                .iter()
                .any(|d| d.name == "memory" && !d.is_active)
        );

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_dot_autocomplete_word_extraction() {
        // Multi-part dot-qualified identifiers extraction
        let line = "SELECT analytics.main.events.event_id FROM t";
        let cursor_char_idx = 37; // right at the end of "event_id" (len of "SELECT analytics.main.events.event_id")
        let prefix: String = line.chars().take(cursor_char_idx).collect();
        let word_start = prefix
            .rfind(|c: char| !c.is_alphanumeric() && c != '_' && c != '.')
            .map(|i| i + 1)
            .unwrap_or(0);
        let current_word = &prefix[word_start..];
        assert_eq!(current_word, "analytics.main.events.event_id");

        let line2 = "SELECT db.";
        let prefix2: String = line2.chars().collect();
        let word_start2 = prefix2
            .rfind(|c: char| !c.is_alphanumeric() && c != '_' && c != '.')
            .map(|i| i + 1)
            .unwrap_or(0);
        let current_word2 = &prefix2[word_start2..];
        assert_eq!(current_word2, "db.");
    }

    #[test]
    fn test_live_quack_metadata_loading() {
        let db_uri = "quack:0.0.0.0:9494";
        let token_var = std::env::var("QUACK_TOKEN").unwrap_or_default();
        if token_var.is_empty() {
            return;
        }
        // Connect to live quack container if running
        let conn_res = crate::engine::connect_target_db(db_uri, Some(&token_var), true);
        if let Ok(conn) = conn_res {
            let temp_dir = std::env::temp_dir().join(format!(
                "dp_live_quack_{}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            let _ = std::fs::create_dir_all(&temp_dir);

            conn.execute("CREATE TEMP TABLE t_test AS SELECT 1;", [])
                .unwrap();
            let meta = load_explorer_metadata(&conn, &temp_dir, true);
            println!(
                "Databases found: {:?}",
                meta.databases
                    .iter()
                    .map(|d| (&d.name, &d.db_type, d.is_active, d.tables.len()))
                    .collect::<Vec<_>>()
            );
            assert_eq!(meta.databases.len(), 2, "Expected target_db and memory");
            assert!(
                meta.databases
                    .iter()
                    .any(|d| d.name == "target_db" && d.is_active)
            );
            assert!(
                meta.databases
                    .iter()
                    .any(|d| d.name == "memory" && !d.is_active)
            );

            let target_db = meta
                .databases
                .iter()
                .find(|d| d.name == "target_db")
                .unwrap();
            assert!(
                target_db.tables.iter().any(|t| t.name == "test_tbl"),
                "Expected test_tbl in target_db"
            );

            let mut expanded_groups = std::collections::HashSet::new();
            let expanded_tables = std::collections::HashSet::new();
            expanded_groups.insert("db:target_db".to_string());
            expanded_groups.insert("target_db:Tables".to_string());
            expanded_groups.insert("db:memory".to_string());

            let nodes =
                rebuild_explorer_nodes(&meta, &expanded_groups, &expanded_tables, &conn, true);
            let node_displays: Vec<String> = nodes.iter().map(|n| n.display.clone()).collect();
            println!("Nodes rendered:\n{}", node_displays.join("\n"));

            assert!(
                nodes
                    .iter()
                    .any(|n| n.name == "db:target_db" && n.display.contains("target_db"))
            );
            assert!(
                nodes
                    .iter()
                    .any(|n| n.name == "db:memory" && n.display.contains("memory"))
            );
            assert!(nodes.iter().any(|n| n.name == "test_tbl"));

            let cols = load_table_columns_for("target_db", "main", "test_tbl", &conn, true);
            println!("Columns for test_tbl: {:?}", cols);
            assert!(cols.iter().any(|c| c.name == "column_a"));

            // Verify PK detection on live Quack for products table
            let prod_cols = load_table_columns_for("target_db", "main", "products", &conn, true);
            let pk_col = prod_cols
                .iter()
                .find(|c| c.name == "product_id")
                .expect("product_id not found in products");
            assert!(
                pk_col.is_pk,
                "product_id should be identified as primary key"
            );

            let mut expanded_tables = std::collections::HashSet::new();
            expanded_tables.insert("products".to_string());
            let nodes_with_cols =
                rebuild_explorer_nodes(&meta, &expanded_groups, &expanded_tables, &conn, true);
            let pk_node = nodes_with_cols
                .iter()
                .find(|n| n.name == "product_id")
                .expect("product_id node not found");
            assert!(
                pk_node.display.contains("🔑 product_id") && pk_node.display.contains("[PK]"),
                "Expected key icon and [PK] tag, got: {}",
                pk_node.display
            );
            println!(
                "Verified live Quack Explorer PK rendering: {}",
                pk_node.display
            );

            // Verify FK detection and rendering on live Quack for orders table
            let order_cols = load_table_columns_for("target_db", "main", "orders", &conn, true);
            let user_id_col = order_cols
                .iter()
                .find(|c| c.name == "user_id")
                .expect("user_id not found in orders");
            assert!(
                user_id_col.is_fk,
                "user_id should be identified as foreign key"
            );
            assert_eq!(user_id_col.fk_target.as_deref(), Some("users.user_id"));

            expanded_tables.insert("orders".to_string());
            let nodes_with_orders =
                rebuild_explorer_nodes(&meta, &expanded_groups, &expanded_tables, &conn, true);
            let fk_node = nodes_with_orders
                .iter()
                .find(|n| n.name == "user_id" && n.display.contains("[FK"))
                .expect("user_id FK node not found");
            assert!(
                fk_node.display.contains("🔗 user_id")
                    && fk_node.display.contains("[FK -> users.user_id]"),
                "Expected FK icon and target, got: {}",
                fk_node.display
            );
            println!(
                "Verified live Quack Explorer FK rendering: {}",
                fk_node.display
            );

            let _ = std::fs::remove_dir_all(&temp_dir);
        }
    }

    #[test]
    fn test_live_insert_users() {
        let db_uri = "quack:0.0.0.0:9494";
        let token_var = std::env::var("QUACK_TOKEN").unwrap_or_default();
        if token_var.is_empty() {
            return;
        }
        if let Ok(conn) = crate::engine::connect_target_db(db_uri, Some(&token_var), true) {
            let temp_dir = std::env::temp_dir().join(format!(
                "dp_live_test_{}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            let _ = std::fs::create_dir_all(&temp_dir);

            let _ = conn.execute_batch("DELETE FROM target_db.users WHERE user_id = 9999;");
            let q = "insert into users values (9999, 'Test', 'User', '1900-01-01', 'test_user@example.com', now(), now(), True);";

            // Execute mutation via execute_batch
            let res = conn.execute_batch(q);
            assert!(res.is_ok(), "execute_batch failed: {:?}", res);

            let count: i64 = conn
                .query_row(
                    "SELECT count(*) FROM target_db.users WHERE user_id = 9999;",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(count, 1, "Expected exactly 1 row inserted, got {}", count);
            println!("Verified: Exactly 1 row inserted via execute_batch for users!");

            // Verify load_explorer_metadata returns the EXACT live count for users
            let meta = load_explorer_metadata(&conn, &temp_dir, true);
            let target_db = meta
                .databases
                .iter()
                .find(|d| d.name == "target_db")
                .unwrap();
            let users_table = target_db.tables.iter().find(|t| t.name == "users").unwrap();
            let actual_db_count: i64 = conn
                .query_row("SELECT count(*) FROM target_db.users;", [], |r| r.get(0))
                .unwrap();
            assert_eq!(
                users_table.row_count,
                Some(actual_db_count),
                "Explorer metadata row_count must match actual count(*)"
            );
            println!(
                "Verified: Explorer metadata row count {} matches actual DB count {}",
                users_table.row_count.unwrap(),
                actual_db_count
            );

            let _ = conn.execute_batch("DELETE FROM target_db.users WHERE user_id = 9999;");
            let _ = std::fs::remove_dir_all(&temp_dir);
        }
    }

    #[test]
    fn test_temporary_tables_and_views_in_explorer() {
        let temp_dir = std::env::temp_dir().join(format!(
            "dp_temp_tbl_test_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&temp_dir).unwrap();

        let conn = Connection::open_in_memory().unwrap();
        conn.execute("CREATE TEMP TABLE staging_tbl (id INT, label VARCHAR);", [])
            .unwrap();
        conn.execute(
            "INSERT INTO staging_tbl VALUES (1, 'alpha'), (2, 'beta');",
            [],
        )
        .unwrap();
        conn.execute(
            "CREATE TEMP VIEW staging_view AS SELECT id, label FROM staging_tbl;",
            [],
        )
        .unwrap();

        let meta = load_explorer_metadata(&conn, &temp_dir, false);
        assert!(meta.temp_tables.contains("staging_tbl"));
        assert!(meta.temp_views.contains("staging_view"));
        assert!(meta.tables.contains(&"staging_tbl".to_string()));
        assert!(meta.views.contains(&"staging_view".to_string()));

        let mut expanded_groups = HashSet::new();
        expanded_groups.insert("Tables".to_string());
        expanded_groups.insert("Views".to_string());
        let expanded_tables = HashSet::new();

        let nodes = rebuild_explorer_nodes(&meta, &expanded_groups, &expanded_tables, &conn, false);
        let tbl_node = nodes
            .iter()
            .find(|n| n.name == "staging_tbl")
            .expect("staging_tbl node not found");
        assert!(
            tbl_node.display.contains("[temp]"),
            "Expected [temp] in display: {}",
            tbl_node.display
        );
        match &tbl_node.node_type {
            NodeType::Table { is_temporary, .. } => assert!(*is_temporary),
            _ => panic!("Expected NodeType::Table"),
        }

        let view_node = nodes
            .iter()
            .find(|n| n.name == "staging_view")
            .expect("staging_view node not found");
        assert!(
            view_node.display.contains("[temp]"),
            "Expected [temp] in display: {}",
            view_node.display
        );
        match &view_node.node_type {
            NodeType::View { is_temporary, .. } => assert!(*is_temporary),
            _ => panic!("Expected NodeType::View"),
        }

        let cols =
            load_table_columns_for(&meta.active_database, "main", "staging_tbl", &conn, false);
        assert_eq!(cols.len(), 2);
        assert_eq!(cols[0].name, "id");
        assert_eq!(cols[1].name, "label");

        let view_cols =
            load_table_columns_for(&meta.active_database, "main", "staging_view", &conn, false);
        assert_eq!(view_cols.len(), 2);
        assert_eq!(view_cols[0].name, "id");
        assert_eq!(view_cols[1].name, "label");

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_handle_sql_error_position_peg() {
        let mut textarea = TextArea::from(vec![
            "SELECT *".to_string(),
            "FROM non_existent".to_string(),
            "WHEREE x = 1".to_string(),
        ]);
        let err = "Parser Error: syntax error at line 3, column 1";
        handle_sql_error_position(&mut textarea, err);
        let (row, col) = textarea.cursor();
        assert_eq!(row, 2);
        assert_eq!(col, 0);
    }

    #[test]
    fn test_handle_sql_error_position_legacy() {
        let mut textarea = TextArea::from(vec!["SELCT 1;".to_string()]);
        let err = "Parser Error: syntax error at or near \"SELCT\"";
        handle_sql_error_position(&mut textarea, err);
    }

    #[test]
    fn test_triggers_in_explorer() {
        let temp_dir = std::env::temp_dir().join(format!(
            "dp_trg_exp_test_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&temp_dir).unwrap();

        let conn = Connection::open_in_memory().unwrap();
        conn.execute("CREATE TABLE orders (id INT, total DOUBLE);", [])
            .unwrap();
        conn.execute("CREATE TABLE order_audit (id INT);", [])
            .unwrap();
        conn.execute("CREATE TRIGGER trg_audit AFTER INSERT ON orders FOR EACH ROW INSERT INTO order_audit VALUES (NEW.id);", []).unwrap();

        let meta = load_explorer_metadata(&conn, &temp_dir, false);
        assert_eq!(meta.triggers.len(), 1);
        assert!(meta.triggers[0].contains("trg_audit"));

        let mut expanded_groups = HashSet::new();
        expanded_groups.insert("Triggers".to_string());
        let expanded_tables = HashSet::new();

        let nodes = rebuild_explorer_nodes(&meta, &expanded_groups, &expanded_tables, &conn, false);
        let trg_node = nodes
            .iter()
            .find(|n| n.name.contains("trg_audit"))
            .expect("trg_audit node not found");
        assert!(trg_node.display.contains("trg_audit"));
        match &trg_node.node_type {
            NodeType::Trigger { .. } => {}
            _ => panic!("Expected NodeType::Trigger"),
        }

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_explorer_fk_and_pk_column_rendering() {
        // Direct test of format_column_display helper
        assert_eq!(
            format_column_display("  ", "id", "INTEGER", true, false, None),
            "  └─ 🔑 id (INTEGER) [PK]"
        );
        assert_eq!(
            format_column_display("  ", "parent_id", "INTEGER", false, true, Some("parent.id")),
            "  └─ 🔗 parent_id (INTEGER) [FK -> parent.id]"
        );
        assert_eq!(
            format_column_display("  ", "parent_id", "INTEGER", true, true, Some("parent.id")),
            "  └─ 🔑🔗 parent_id (INTEGER) [PK, FK -> parent.id]"
        );
        assert_eq!(
            format_column_display("  ", "name", "VARCHAR", false, false, None),
            "  └─ name (VARCHAR)"
        );

        // Integration test with database catalog and rebuild_explorer_nodes
        let temp_dir = std::env::temp_dir().join(format!(
            "dp_fk_test_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&temp_dir).unwrap();

        let conn = Connection::open_in_memory().unwrap();
        conn.execute(
            "CREATE TABLE parent (id INT PRIMARY KEY, name VARCHAR);",
            [],
        )
        .unwrap();
        conn.execute("CREATE TABLE child (child_id INT PRIMARY KEY, parent_id INT REFERENCES parent(id), data VARCHAR);", []).unwrap();
        conn.execute("CREATE TABLE composite_child (parent_id INT REFERENCES parent(id), sub_id INT, PRIMARY KEY (parent_id, sub_id));", []).unwrap();

        let meta = load_explorer_metadata(&conn, &temp_dir, false);
        let mut expanded_groups = HashSet::new();
        expanded_groups.insert("Tables".to_string());
        let mut expanded_tables = HashSet::new();
        expanded_tables.insert("child".to_string());
        expanded_tables.insert("composite_child".to_string());

        let nodes = rebuild_explorer_nodes(&meta, &expanded_groups, &expanded_tables, &conn, false);

        // Check child table columns
        let child_pk = nodes
            .iter()
            .find(|n| n.name == "child_id")
            .expect("child_id not found");
        assert!(
            child_pk.display.contains("🔑 child_id"),
            "Expected 🔑 for child_id: {}",
            child_pk.display
        );
        assert!(
            child_pk.display.contains("[PK]"),
            "Expected [PK] for child_id: {}",
            child_pk.display
        );
        assert!(
            !child_pk.display.contains("🔗"),
            "child_id should not have FK icon"
        );

        let parent_id_nodes: Vec<_> = nodes.iter().filter(|n| n.name == "parent_id").collect();
        assert_eq!(
            parent_id_nodes.len(),
            2,
            "Expected 2 parent_id columns (child and composite_child)"
        );

        let child_fk = parent_id_nodes[0];
        assert!(
            child_fk.display.contains("🔗 parent_id"),
            "Expected 🔗 for parent_id in child: {}",
            child_fk.display
        );
        assert!(
            child_fk.display.contains("[FK -> parent.id]"),
            "Expected [FK -> parent.id]: {}",
            child_fk.display
        );
        assert!(
            !child_fk.display.contains("🔑"),
            "parent_id in child should not have PK icon"
        );

        // Check composite PK+FK table
        let composite_fk = parent_id_nodes[1];
        assert!(
            composite_fk.display.contains("🔑🔗 parent_id"),
            "Expected 🔑🔗 for composite parent_id: {}",
            composite_fk.display
        );
        assert!(
            composite_fk.display.contains("[PK, FK -> parent.id]"),
            "Expected [PK, FK -> parent.id]: {}",
            composite_fk.display
        );

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_new_asset_modal_state() {
        let mut modal = NewAssetModalState::new();
        assert!(!modal.is_open);
        assert_eq!(modal.category_index, 0);
        assert_eq!(modal.current_category(), ProjectCategory::Table);
        assert_eq!(modal.category_label(), "Table");

        modal.reset();
        assert!(modal.is_open);
        assert_eq!(modal.active_field, 1);
        assert_eq!(modal.name, "");

        modal.category_index = 1;
        assert_eq!(modal.current_category(), ProjectCategory::View);
        assert_eq!(modal.category_label(), "View");

        modal.category_index = 2;
        assert_eq!(modal.current_category(), ProjectCategory::Macro);
        assert_eq!(modal.category_label(), "Macro");

        modal.category_index = 3;
        assert_eq!(modal.current_category(), ProjectCategory::Trigger);
        assert_eq!(modal.category_label(), "Trigger");

        modal.category_index = 4;
        assert_eq!(modal.current_category(), ProjectCategory::Schema);
        assert_eq!(modal.category_label(), "Schema");

        modal.category_index = 5;
        assert_eq!(modal.current_category(), ProjectCategory::Query);
        assert_eq!(modal.category_label(), "Query");

        modal.category_index = 6;
        assert_eq!(modal.current_category(), ProjectCategory::Seed);
        assert_eq!(modal.category_label(), "Seed");
    }

    #[test]
    fn test_generate_asset_boilerplate() {
        let table_sql = generate_asset_boilerplate(ProjectCategory::Table, "customers");
        assert!(table_sql.contains("CREATE TABLE customers"));
        assert!(table_sql.contains("id INTEGER PRIMARY KEY"));

        let view_sql = generate_asset_boilerplate(ProjectCategory::View, "active_users.sql");
        assert!(view_sql.contains("CREATE VIEW active_users AS"));

        let macro_sql = generate_asset_boilerplate(ProjectCategory::Macro, "to_cents");
        assert!(macro_sql.contains("CREATE MACRO to_cents(val)"));

        let trigger_sql = generate_asset_boilerplate(ProjectCategory::Trigger, "trg_audit");
        assert!(trigger_sql.contains("CREATE OR REPLACE TRIGGER trg_audit"));
        assert!(trigger_sql.contains("FOR EACH ROW"));

        let schema_sql = generate_asset_boilerplate(ProjectCategory::Schema, "analytics");
        assert!(schema_sql.contains("CREATE SCHEMA IF NOT EXISTS analytics"));

        let query_sql = generate_asset_boilerplate(ProjectCategory::Query, "adhoc_report");
        assert!(query_sql.contains("SELECT 1;"));

        let seed_data = generate_asset_boilerplate(ProjectCategory::Seed, "sample_data");
        assert!(seed_data.contains("id,name,value"));
    }

    #[test]
    fn test_get_asset_file_path() {
        let root = PathBuf::from("/test/project");
        assert_eq!(
            get_asset_file_path(&root, ProjectCategory::Schema, "staging"),
            root.join("schemas").join("staging.sql")
        );
        assert_eq!(
            get_asset_file_path(&root, ProjectCategory::Table, "orders.sql"),
            root.join("tables").join("orders.sql")
        );
        assert_eq!(
            get_asset_file_path(&root, ProjectCategory::View, "v_orders"),
            root.join("views").join("v_orders.sql")
        );
        assert_eq!(
            get_asset_file_path(&root, ProjectCategory::Macro, "my_macro"),
            root.join("macros").join("my_macro.sql")
        );
        assert_eq!(
            get_asset_file_path(&root, ProjectCategory::Trigger, "orders_audit"),
            root.join("triggers").join("orders_audit.sql")
        );
        assert_eq!(
            get_asset_file_path(&root, ProjectCategory::Query, "analysis"),
            root.join("queries").join("analysis.sql")
        );
        assert_eq!(
            get_asset_file_path(&root, ProjectCategory::Seed, "countries"),
            root.join("seeds").join("countries.csv")
        );
    }

    #[test]
    fn test_rebuild_project_nodes_and_git_badges() {
        let temp_dir = std::env::temp_dir().join(format!(
            "dp_proj_nodes_test_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(temp_dir.join("tables")).unwrap();
        std::fs::create_dir_all(temp_dir.join("views")).unwrap();
        std::fs::create_dir_all(temp_dir.join("macros")).unwrap();

        let table_file = temp_dir.join("tables").join("users.sql");
        std::fs::write(&table_file, "CREATE TABLE users (id INT PRIMARY KEY);").unwrap();

        let view_file = temp_dir.join("views").join("v_users.sql");
        std::fs::write(&view_file, "CREATE VIEW v_users AS SELECT * FROM users;").unwrap();

        std::fs::write(
            temp_dir.join("duckpack.toml"),
            "[project]\nname = \"test\"\n",
        )
        .unwrap();

        let mut git_status = HashMap::new();
        git_status.insert(table_file.clone(), "[M]".to_string());
        git_status.insert(view_file.clone(), "[+]".to_string());

        let mut expanded = HashSet::new();
        expanded.insert("proj:tables".to_string());
        expanded.insert("proj:views".to_string());
        expanded.insert("proj:triggers".to_string()); // triggers directory doesn't exist on disk

        let nodes = rebuild_project_nodes(&temp_dir, &expanded, &git_status);

        // Verify tables group
        let table_group = nodes
            .iter()
            .find(|n| n.name == "proj:tables")
            .expect("proj:tables group not found");
        assert!(table_group.display.contains("Tables (1)"));

        // Verify users.sql with [M] badge
        let user_node = nodes
            .iter()
            .find(|n| n.name == "users.sql")
            .expect("users.sql node not found");
        assert!(user_node.display.contains("[M]"));

        // Verify views group
        let view_group = nodes
            .iter()
            .find(|n| n.name == "proj:views")
            .expect("proj:views group not found");
        assert!(view_group.display.contains("Views (1)"));

        // Verify v_users.sql with [+] badge
        let view_node = nodes
            .iter()
            .find(|n| n.name == "v_users.sql")
            .expect("v_users.sql node not found");
        assert!(view_node.display.contains("[+]"));

        // Verify triggers group is present even though triggers/ dir did not exist
        let trigger_group = nodes
            .iter()
            .find(|n| n.name == "proj:triggers")
            .expect("proj:triggers group not found");
        assert!(trigger_group.display.contains("Triggers (0)"));

        // Verify triggers empty placeholder node is present
        let empty_trigger_node = nodes
            .iter()
            .find(|n| n.name == "proj:triggers:empty")
            .expect("proj:triggers:empty placeholder not found");
        assert!(
            empty_trigger_node
                .display
                .contains("empty - press 'a' to add")
        );

        // Verify duckpack.toml config node
        let config_node = nodes
            .iter()
            .find(|n| n.name == "duckpack.toml")
            .expect("duckpack.toml node not found");
        assert!(config_node.display.contains("duckpack.toml"));

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_compute_and_apply_ide_diff() {
        let temp_dir = std::env::temp_dir().join(format!(
            "dp_ide_diff_test_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(temp_dir.join("tables")).unwrap();

        let table_file = temp_dir.join("tables").join("items.sql");
        std::fs::write(
            &table_file,
            "CREATE TABLE items (id INTEGER PRIMARY KEY, title VARCHAR);",
        )
        .unwrap();

        let target_conn = Connection::open_in_memory().unwrap();

        // 1. Compute diff (should detect items table needs to be created)
        let diff_result =
            compute_ide_diff(&temp_dir, &target_conn).expect("compute_ide_diff failed");
        assert_eq!(diff_result.items.len(), 1);
        assert_eq!(diff_result.items[0].obj_name, "items");
        assert_eq!(
            diff_result.items[0].item_type,
            crate::diff::DiffItemType::CreateTable
        );

        // 2. Apply diff in target_conn
        apply_ide_diff(&temp_dir, &target_conn, &diff_result).expect("apply_ide_diff failed");

        // 3. Recompute diff (should be 0 changes now)
        let diff_after = compute_ide_diff(&temp_dir, &target_conn).expect("recompute failed");
        assert_eq!(diff_after.items.len(), 0);

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_live_quack_compute_and_apply_ide_diff() {
        let db_uri = "quack:0.0.0.0:9494";
        let token_var = std::env::var("QUACK_TOKEN").unwrap_or_default();
        if token_var.is_empty() {
            return;
        }
        if let Ok(conn) = crate::engine::connect_target_db(db_uri, Some(&token_var), true) {
            let temp_dir = std::env::temp_dir().join(format!(
                "dp_quack_diff_test_{}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            let _ = std::fs::create_dir_all(temp_dir.join("tables"));

            // Ensure test table is dropped on remote
            let _ = conn.execute_batch(
                "CALL target_db.query('DROP TABLE IF EXISTS __duckpack_ide_quack_test_tbl;');",
            );

            let table_file = temp_dir
                .join("tables")
                .join("__duckpack_ide_quack_test_tbl.sql");
            std::fs::write(&table_file, "CREATE TABLE __duckpack_ide_quack_test_tbl (id INTEGER PRIMARY KEY, note VARCHAR);").unwrap();

            // 1. Compute diff against live Quack connection
            let diff_result =
                compute_ide_diff(&temp_dir, &conn).expect("compute_ide_diff failed on Quack");
            let pending_item = diff_result
                .items
                .iter()
                .find(|i| i.obj_name == "__duckpack_ide_quack_test_tbl");
            assert!(
                pending_item.is_some(),
                "Expected pending create table for __duckpack_ide_quack_test_tbl"
            );

            // 2. Filter diff result to only apply our test table
            let mut test_diff = diff_result.clone();
            for item in &mut test_diff.items {
                if item.obj_name != "__duckpack_ide_quack_test_tbl" {
                    item.selected = false;
                }
            }

            // 3. Apply diff via apply_ide_diff - this tests catalog resolution, transactions, and Quack DDL execution
            let apply_res = apply_ide_diff(&temp_dir, &conn, &test_diff);
            assert!(
                apply_res.is_ok(),
                "apply_ide_diff on Quack failed: {:?}",
                apply_res.err()
            );

            // 4. Verify table exists on remote Quack database
            let count: i64 = conn.query_row(
                "SELECT * FROM target_db.query('SELECT count(*) FROM __duckpack_ide_quack_test_tbl')",
                [],
                |r| r.get(0),
            ).expect("Failed to query created test table on remote Quack");
            assert_eq!(count, 0);

            // Cleanup remote table
            let _ = conn.execute_batch(
                "CALL target_db.query('DROP TABLE IF EXISTS __duckpack_ide_quack_test_tbl;');",
            );
            let _ = std::fs::remove_dir_all(&temp_dir);
        }
    }

    #[test]
    fn test_diff_space_toggle_and_lock_recomputation() {
        let mut diff = crate::diff::DiffResult {
            items: vec![
                crate::diff::DiffItem {
                    item_type: crate::diff::DiffItemType::CreateTable,
                    obj_name: "customers".to_string(),
                    sql: "CREATE TABLE customers (id INT PRIMARY KEY);".to_string(),
                    old_def: None,
                    new_def: None,
                    selected: true,
                    locked: false,
                    ignored: false,
                },
                crate::diff::DiffItem {
                    item_type: crate::diff::DiffItemType::CreateView,
                    obj_name: "v_customers".to_string(),
                    sql: "CREATE VIEW v_customers AS SELECT * FROM customers;".to_string(),
                    old_def: None,
                    new_def: None,
                    selected: true,
                    locked: false,
                    ignored: false,
                },
            ],
        };

        // Initially both are selected and unlocked
        assert!(diff.items[0].selected);
        assert!(!diff.items[0].locked);
        assert!(diff.items[1].selected);
        assert!(!diff.items[1].locked);

        // Toggle customers off with Space
        diff.items[0].selected = false;
        crate::tui::recompute_locks(&mut diff.items);

        // customers is unselected
        assert!(!diff.items[0].selected);
        assert!(!diff.items[0].locked);

        // Dependent view v_customers is automatically locked and deselected
        assert!(!diff.items[1].selected);
        assert!(diff.items[1].locked);

        // Re-selecting customers unlocks the view
        diff.items[0].selected = true;
        crate::tui::recompute_locks(&mut diff.items);
        assert!(diff.items[0].selected);
        assert!(!diff.items[1].locked);
    }

    #[test]
    fn test_nested_schemas_in_ide_explorer() {
        let temp_dir = std::env::temp_dir().join(format!(
            "dp_ide_nested_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&temp_dir).unwrap();

        let conn = Connection::open_in_memory().unwrap();
        conn.execute("ATTACH ':memory:' AS target_db;", []).unwrap();
        conn.execute("USE target_db;", []).unwrap();
        conn.execute("CREATE SCHEMA target_db.region;", []).unwrap();
        conn.execute("CREATE SCHEMA target_db.region.sales;", [])
            .unwrap();
        conn.execute(
            "CREATE TABLE target_db.region.products (id INT PRIMARY KEY);",
            [],
        )
        .unwrap();
        conn.execute(
            "CREATE TABLE target_db.region.sales.orders (order_id INT PRIMARY KEY, qty INT);",
            [],
        )
        .unwrap();

        let meta = load_explorer_metadata(&conn, &temp_dir, false);
        let mut expanded_groups = HashSet::new();
        expanded_groups.insert("db:target_db".to_string());
        expanded_groups.insert("target_db:Tables".to_string());
        let expanded_tables = HashSet::new();

        let nodes = rebuild_explorer_nodes(&meta, &expanded_groups, &expanded_tables, &conn, false);

        // Verify products node display
        let prod_node = nodes
            .iter()
            .find(|n| n.name == "products")
            .expect("products node");
        assert!(prod_node.display.contains("region.products"));

        // Verify orders node display with nested schema
        let orders_node = nodes
            .iter()
            .find(|n| n.name == "orders")
            .expect("orders node");
        assert!(orders_node.display.contains("region.sales.orders"));

        match &orders_node.node_type {
            NodeType::Table {
                database, schema, ..
            } => {
                assert_eq!(database.as_deref(), Some("target_db"));
                assert_eq!(schema.as_deref(), Some("region.sales"));
            }
            _ => panic!("Expected NodeType::Table"),
        }

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_attach_modal_state_providers() {
        let mut modal = AttachModalState::new();
        assert_eq!(modal.provider(), AttachProvider::DuckDB);
        assert!(!modal.is_open);

        modal.reset();
        assert!(modal.is_open);
        assert_eq!(modal.active_field, 1);

        modal.set_provider(1);
        assert_eq!(modal.provider(), AttachProvider::Quack);
        assert_eq!(modal.host, "localhost");
        assert_eq!(modal.port, "9494");
        assert_eq!(modal.alias, "quack_remote");

        modal.set_provider(2);
        assert_eq!(modal.provider(), AttachProvider::MotherDuck);

        modal.set_provider(3);
        assert_eq!(modal.provider(), AttachProvider::SQLite);

        modal.set_provider(4);
        assert_eq!(modal.provider(), AttachProvider::PostgreSQL);
        assert_eq!(modal.host, "localhost");
        assert_eq!(modal.port, "5432");
        assert_eq!(modal.username, "postgres");

        modal.set_provider(5);
        assert_eq!(modal.provider(), AttachProvider::MySQL);
        assert_eq!(modal.host, "localhost");
        assert_eq!(modal.port, "3306");
        assert_eq!(modal.username, "root");

        modal.set_provider(6);
        assert_eq!(modal.provider(), AttachProvider::MSSQL);
        assert_eq!(modal.host, "localhost");
        assert_eq!(modal.port, "1433");
        assert_eq!(modal.username, "sa");

        // Test MSSQL Connection String building
        modal.database = "mydb".to_string();
        modal.alias = "mssql_db".to_string();
        modal.password = "secretpass".to_string();
        let (conn_str, alias) = modal.build_connection_target().unwrap();
        assert_eq!(alias, "mssql_db");
        assert!(conn_str.contains("Server=localhost,1433"));
        assert!(conn_str.contains("Database=mydb"));
        assert!(conn_str.contains("User Id=sa"));
        assert!(conn_str.contains("Password=secretpass"));
        assert!(conn_str.contains("TrustServerCertificate=yes"));

        modal.disable_ssl = true;
        let (conn_str_no_ssl, _) = modal.build_connection_target().unwrap();
        assert!(conn_str_no_ssl.contains("Encrypt=no"));

        // Test Postgres URI building
        modal.reset();
        modal.set_provider(4);
        modal.database = "analytics".to_string();
        modal.alias = "analytics".to_string();
        modal.password = "pgpass".to_string();
        modal.disable_ssl = true;
        let (pg_uri, pg_alias) = modal.build_connection_target().unwrap();
        assert_eq!(pg_alias, "analytics");
        assert_eq!(
            pg_uri,
            "postgres://postgres:pgpass@localhost:5432/analytics?sslmode=disable"
        );

        // Test MySQL URI building
        modal.reset();
        modal.set_provider(5);
        modal.database = "store".to_string();
        modal.alias = "store".to_string();
        modal.password = "mysqlpass".to_string();
        modal.disable_ssl = true;
        let (mysql_uri, mysql_alias) = modal.build_connection_target().unwrap();
        assert_eq!(mysql_alias, "store");
        assert_eq!(
            mysql_uri,
            "mysql://root:mysqlpass@localhost:3306/store?ssl-mode=DISABLED"
        );

        // Test Quack Target building
        modal.reset();
        modal.set_provider(1);
        let (quack_target, quack_alias) = modal.build_connection_target().unwrap();
        assert_eq!(quack_alias, "quack_remote");
        assert_eq!(quack_target, "quack:localhost:9494");

        // Test Typing and Backspace with auto-sync alias
        modal.reset();
        modal.set_provider(6);
        modal.active_field = 3; // Database Name field
        modal.handle_char('p');
        modal.handle_char('r');
        modal.handle_char('o');
        modal.handle_char('d');
        assert_eq!(modal.database, "prod");
        assert_eq!(modal.alias, "prod"); // Alias auto-populated
        modal.handle_backspace();
        assert_eq!(modal.database, "pro");
        assert_eq!(modal.alias, "pro");

        modal.active_field = 1; // Host field
        modal.host.clear();
        modal.handle_char('1');
        modal.handle_char('0');
        modal.handle_char('.');
        modal.handle_char('0');
        assert_eq!(modal.host, "10.0");
        modal.handle_backspace();
        assert_eq!(modal.host, "10.");

        // Test Navigation
        modal.active_field = 1;
        modal.nav_right();
        assert_eq!(modal.active_field, 2); // To Port
        modal.nav_left();
        assert_eq!(modal.active_field, 1); // Back to Host
        modal.nav_down();
        assert_eq!(modal.active_field, 3); // Down to Database Name

        modal.reset();
        assert!(!modal.disable_ssl);
        assert!(!modal.insecure);
        assert!(!modal.read_only);
    }

    #[test]
    fn test_secret_modal_state_and_providers() {
        let mut modal = SecretModalState::new();
        assert_eq!(modal.provider(), CloudProvider::S3);
        assert_eq!(modal.region, "us-east-1");
        assert!(modal.use_ssl);
        assert!(modal.save_to_env);

        modal.set_provider(1); // Cloudflare R2
        assert_eq!(modal.provider(), CloudProvider::CloudflareR2);
        assert_eq!(modal.region, "auto");
        assert!(modal.endpoint.contains("r2.cloudflarestorage.com"));

        modal.set_provider(2); // MinIO
        assert_eq!(modal.provider(), CloudProvider::MinIO);
        assert_eq!(modal.endpoint, "localhost:9000");
        assert!(!modal.use_ssl);

        modal.reset();
        assert_eq!(modal.provider(), CloudProvider::S3);
        assert!(modal.is_open);
    }

    #[test]
    fn test_explorer_nodes_secrets_and_cloud_buckets() {
        let conn = Connection::open_in_memory().unwrap();
        let meta = ExplorerMetadata {
            databases: vec![],
            active_database: "memory".to_string(),
            tables: vec![],
            table_counts: HashMap::new(),
            views: vec![],
            temp_tables: HashSet::new(),
            temp_views: HashSet::new(),
            macros: vec![],
            triggers: vec![],
            queries: vec![],
            dictionary: vec![],
            secrets: vec![("my_s3_secret".to_string(), "s3".to_string())],
            cloud_buckets: vec!["s3://analytics-lake".to_string()],
        };

        let mut expanded_groups = HashSet::new();
        let expanded_tables = HashSet::new();

        // Not expanded
        let nodes = rebuild_explorer_nodes(&meta, &expanded_groups, &expanded_tables, &conn, false);
        let sec_group = nodes
            .iter()
            .find(|n| n.name == "Secrets & Cloud Buckets")
            .expect("Secrets & Cloud Buckets group");
        assert!(sec_group.display.contains("Secrets & Cloud (2)"));
        assert!(!nodes.iter().any(|n| n.name == "my_s3_secret"));

        // Expanded
        expanded_groups.insert("Secrets & Cloud Buckets".to_string());
        let nodes_expanded =
            rebuild_explorer_nodes(&meta, &expanded_groups, &expanded_tables, &conn, false);
        assert!(nodes_expanded.iter().any(|n| n.name == "add_cloud_secret"));
        assert!(nodes_expanded.iter().any(|n| n.name == "add_cloud_bucket"));
        assert!(nodes_expanded.iter().any(|n| n.name == "my_s3_secret"));
        assert!(
            nodes_expanded
                .iter()
                .any(|n| n.name == "bucket:s3://analytics-lake")
        );
    }

    #[test]
    fn test_populate_secret_modal_for_edit() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute(
            "CREATE TEMPORARY TABLE duckdb_secrets (name VARCHAR, secret_string VARCHAR);",
            [],
        )
        .unwrap();
        conn.execute("INSERT INTO duckdb_secrets VALUES ('test_edit', 'name=test_edit;endpoint=localhost:9000;key_id=my_key;use_ssl=false');", []).unwrap();

        let temp_dir = std::env::temp_dir().join(format!(
            "dp_ide_edit_sec_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::create_dir_all(&temp_dir);
        let _ = std::fs::write(
            temp_dir.join(".env"),
            "AWS_SECRET_ACCESS_KEY=my_secret_val\nDUCKPACK_S3_BUCKET=s3://test-bucket\n",
        );

        let mut modal = SecretModalState::new();
        populate_secret_modal_for_edit(&conn, &temp_dir, "test_edit", &mut modal);

        assert!(modal.is_open);
        assert_eq!(modal.name, "test_edit");
        assert_eq!(modal.key_id, "my_key");
        assert_eq!(modal.secret_key, "my_secret_val");
        assert_eq!(modal.endpoint, "localhost:9000");
        assert_eq!(modal.bucket, "s3://test-bucket");
        assert_eq!(modal.provider(), CloudProvider::MinIO);
        assert!(!modal.use_ssl);

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_database_picker_modal_state() {
        let mut modal = DatabasePickerModalState::new();
        assert!(!modal.is_open);
        assert_eq!(modal.selected_index, 0);

        let dbs = vec![
            crate::catalog::DatabaseInfo {
                name: "memory".to_string(),
                db_type: "duckdb".to_string(),
                path: None,
                is_active: true,
                readonly: false,
                tables: vec![],
                views: vec![],
                triggers: vec![],
            },
            crate::catalog::DatabaseInfo {
                name: "analytics_db".to_string(),
                db_type: "postgres".to_string(),
                path: Some("postgres://...".to_string()),
                is_active: false,
                readonly: false,
                tables: vec![],
                views: vec![],
                triggers: vec![],
            },
            crate::catalog::DatabaseInfo {
                name: "mssql_db".to_string(),
                db_type: "mssql".to_string(),
                path: None,
                is_active: false,
                readonly: false,
                tables: vec![],
                views: vec![],
                triggers: vec![],
            },
        ];

        modal.open_for_current("analytics_db", &dbs);
        assert!(modal.is_open);
        assert_eq!(modal.selected_index, 1);

        modal.next(dbs.len());
        assert_eq!(modal.selected_index, 2);

        // Next at end should clamp to last element
        modal.next(dbs.len());
        assert_eq!(modal.selected_index, 2);

        modal.prev();
        assert_eq!(modal.selected_index, 1);

        modal.prev();
        assert_eq!(modal.selected_index, 0);

        // Prev at beginning should clamp to 0
        modal.prev();
        assert_eq!(modal.selected_index, 0);

        // Opening for unknown db falls back to index 0
        modal.open_for_current("unknown_catalog", &dbs);
        assert_eq!(modal.selected_index, 0);
    }

    #[test]
    fn test_editor_tab_target_database_and_db_icon() {
        let tab = EditorTab::new("mssql_db".to_string());
        assert_eq!(tab.target_database, "mssql_db");
        assert!(!tab.is_dirty);
        assert!(tab.active_file.is_none());

        assert_eq!(db_icon("sqlite"), "🗃️");
        assert_eq!(db_icon("postgres"), "🔌");
        assert_eq!(db_icon("postgresql"), "🔌");
        assert_eq!(db_icon("mysql"), "🔌");
        assert_eq!(db_icon("mssql"), "🔌");
        assert_eq!(db_icon("duckdb"), "📁");
        assert_eq!(db_icon("unknown"), "📁");
    }

    #[test]
    fn test_exit_confirmation_modal_state() {
        let mut modal = ExitConfirmationModalState::new();
        assert!(!modal.is_open);
        assert!(!modal.confirm_exit);

        modal.open();
        assert!(modal.is_open);
        assert!(!modal.confirm_exit); // Safe default: Cancel

        modal.toggle();
        assert!(modal.confirm_exit); // Toggled to Exit

        modal.toggle();
        assert!(!modal.confirm_exit); // Toggled back to Cancel

        modal.close();
        assert!(!modal.is_open);
        assert!(!modal.confirm_exit);

        use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        modal.open();
        assert_eq!(
            modal.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
            None
        );
        assert!(modal.confirm_exit);
        assert_eq!(
            modal.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
            Some(true)
        );

        modal.open();
        assert_eq!(
            modal.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
            Some(false)
        );
        assert!(!modal.is_open);
    }

    #[test]
    fn test_new_asset_modal_handle_key_and_editor_tab_load_content() {
        use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let temp_dir = std::env::temp_dir().join(format!(
            "duckpack_test_ide_modal_key_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();

        let mut modal = NewAssetModalState::new();
        modal.reset();
        for ch in "orders".chars() {
            assert_eq!(
                modal.handle_key(
                    KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE),
                    &temp_dir
                ),
                None
            );
        }
        let created = modal
            .handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &temp_dir)
            .expect("Expected Enter to create orders.sql");
        assert!(created.exists());
        assert!(!modal.is_open);

        let mut tabs = vec![EditorTab::new("memory".to_string())];
        let mut active_idx = 0;
        assert!(tabs[0].is_untitled_empty());
        assert!(open_or_focus_file_in_tabs(
            &mut tabs,
            &mut active_idx,
            created.clone()
        ));
        assert_eq!(tabs.len(), 1);
        assert_eq!(tabs[0].active_file.as_ref(), Some(&created));
        assert!(!tabs[0].is_untitled_empty());

        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_pagination_page_sizes_and_explicit_sql_limit() {
        let mut tab = EditorTab::new("memory".to_string());
        assert_eq!(tab.pagination_limit, 2_000);
        assert_eq!(tab.pagination_limit_label(), "2000");

        tab.pagination_offset = 4_000;
        assert_eq!(tab.cycle_pagination_limit(), 10_000);
        assert_eq!(tab.pagination_offset, 0);
        assert_eq!(tab.pagination_limit_label(), "10000");

        assert_eq!(tab.cycle_pagination_limit(), 50_000);
        assert_eq!(tab.pagination_limit_label(), "50000");

        assert_eq!(tab.cycle_pagination_limit(), 0);
        assert_eq!(tab.pagination_limit_label(), "All");

        assert_eq!(tab.cycle_pagination_limit(), 500);
        assert_eq!(tab.pagination_limit_label(), "500");

        assert_eq!(tab.cycle_pagination_limit(), 2_000);

        // Explicit SQL LIMIT detection
        assert_eq!(
            extract_explicit_sql_limit("SELECT * FROM range(5000) LIMIT 1500;"),
            Some(1500)
        );
        assert_eq!(
            extract_explicit_sql_limit("SELECT * FROM range(5000) limit 1200 offset 10"),
            Some(1200)
        );
        assert_eq!(extract_explicit_sql_limit("SELECT * FROM users;"), None);

        // build_paginated_sql honours explicit LIMIT > pagination_limit without truncating to 500
        let (sql_explicit, is_pag_explicit) =
            build_paginated_sql("SELECT * FROM range(5000) LIMIT 1500;", 500, 0);
        assert_eq!(sql_explicit, "SELECT * FROM range(5000) LIMIT 1500");
        assert!(!is_pag_explicit);

        // Standard query wraps with pagination_limit and pagination_offset
        let (sql_wrapped, is_pag_wrapped) = build_paginated_sql("SELECT * FROM users;", 2_000, 0);
        assert_eq!(
            sql_wrapped,
            "SELECT * FROM (SELECT * FROM users) LIMIT 2000 OFFSET 0"
        );
        assert!(is_pag_wrapped);

        // Unlimited page size (0) executes directly without LIMIT wrapper
        let (sql_all, is_pag_all) = build_paginated_sql("SELECT * FROM users;", 0, 0);
        assert_eq!(sql_all, "SELECT * FROM users");
        assert!(!is_pag_all);
    }

    #[test]
    fn test_explorer_column_data_type_colour_coding_and_scrolling() {
        assert_eq!(
            column_data_type_style("INTEGER").fg,
            Some(Color::LightGreen)
        );
        assert_eq!(
            column_data_type_style("DECIMAL(18, 2)").fg,
            Some(Color::LightGreen)
        );
        assert_eq!(column_data_type_style("VARCHAR").fg, Some(Color::Yellow));
        assert_eq!(column_data_type_style("UUID").fg, Some(Color::Yellow));
        assert_eq!(
            column_data_type_style("TIMESTAMP WITH TIME ZONE").fg,
            Some(Color::LightMagenta)
        );
        assert_eq!(column_data_type_style("BOOLEAN").fg, Some(Color::Magenta));
        let complex_style = column_data_type_style("STRUCT(a INT, b VARCHAR)");
        assert_eq!(complex_style.fg, Some(Color::LightBlue));
        assert!(complex_style.add_modifier.contains(Modifier::BOLD));

        let col_node = ExplorerNode {
            display: format_column_display(
                "  ",
                "user_id",
                "INTEGER",
                true,
                true,
                Some("users.id"),
            ),
            name: "user_id".to_string(),
            node_type: NodeType::Column {
                data_type: "INTEGER".to_string(),
                is_pk: true,
                is_fk: true,
                fk_target: Some("users.id".to_string()),
            },
            level: 2,
            is_expandable: false,
            is_expanded: false,
        };

        let line = render_explorer_node_line(&col_node, 0);
        let full_text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(
            full_text,
            "      └─ 🔑🔗 user_id (INTEGER) [PK, FK -> users.id]"
        );

        let dt_span = line
            .spans
            .iter()
            .find(|s| s.content.as_ref() == "INTEGER")
            .expect("Expected styled INTEGER span");
        assert_eq!(dt_span.style.fg, Some(Color::LightGreen));

        let pk_span = line
            .spans
            .iter()
            .find(|s| s.content.starts_with("[PK"))
            .expect("Expected styled [PK span");
        assert_eq!(pk_span.style.fg, Some(Color::Yellow));
        assert!(pk_span.style.add_modifier.contains(Modifier::BOLD));

        let fk_span = line
            .spans
            .iter()
            .find(|s| s.content.contains("FK -> users.id]"))
            .expect("Expected styled FK span");
        assert_eq!(fk_span.style.fg, Some(Color::Cyan));

        // Verify horizontal scroll preserves characters and span styles after skipping 6 leading spaces
        let scrolled_line = render_explorer_node_line(&col_node, 6);
        let scrolled_text: String = scrolled_line
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect();
        assert_eq!(
            scrolled_text,
            "└─ 🔑🔗 user_id (INTEGER) [PK, FK -> users.id]"
        );
    }

    #[test]
    fn test_exit_confirmation_modal_render_with_warning() {
        let backend = ratatui::backend::TestBackend::new(90, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut modal = ExitConfirmationModalState::new();
        modal.open();

        terminal
            .draw(|f| {
                let rect = modal.render_with_warning(
                    f,
                    " ⚠️ Exit DuckPack Diff Review ",
                    "Are you sure you want to exit Diff Review?",
                    Some("⚠️ 3 pending change(s) have not been applied!"),
                );
                assert_eq!(rect.height, 9);
            })
            .unwrap();
    }
}
