pub mod editor;
pub mod execution;
pub mod explorer;
pub mod modals;

pub use crate::catalog::*;
pub use crate::query::*;
pub(crate) use editor::{
    EditorTab, FocusPane, generate_schema_visualizer,
};
#[cfg(test)]
pub(crate) use editor::{build_paginated_sql, extract_explicit_sql_limit, format_duckdb_value};
pub use editor::{
    calculate_status_height, get_clipboard_text, handle_sql_error_position, set_clipboard_text,
};
pub(crate) use explorer::{
    ExplorerMode, NodeType, compute_ide_diff_for_catalog, db_icon,
    get_git_branch_and_status, insert_failed_workspace_nodes, rebuild_explorer_nodes,
    rebuild_project_nodes, render_explorer_node_line,
};
#[cfg(test)]
pub(crate) use explorer::{
    ExplorerNode, ProjectCategory, apply_ide_diff, column_data_type_style, compute_ide_diff, format_column_display,
    generate_asset_boilerplate, get_asset_file_path,
};
pub(crate) use modals::{
    AddBucketModalState, AttachModalState, DatabasePickerModalState, ExitConfirmationModalState,
    InIdeDiffModalState, NewAssetModalState, QuackAuthModalState, SecretModalState,
    TimeTravelModalState, populate_secret_modal_for_edit, render_help_modal,
    render_schema_visualizer_modal,
};
#[cfg(test)]
pub(crate) use modals::{
    AttachProvider, CloudProvider, DuckLakeBackend, QuackAuthTab, TimeTravelMode,
};

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

fn build_db_explorer_items(
    meta: &ExplorerMetadata,
    expanded_groups: &HashSet<String>,
    expanded_tables: &HashSet<String>,
    conn: &Connection,
    is_quack: bool,
    failed_attachments: &[crate::workspace::FailedAttachment],
) -> Vec<explorer::ExplorerNode> {
    let mut items = rebuild_explorer_nodes(meta, expanded_groups, expanded_tables, conn, is_quack);
    insert_failed_workspace_nodes(&mut items, failed_attachments);
    items
}

fn retry_failed_workspace_attachment(
    alias: &str,
    failed_attachments: &mut Vec<crate::workspace::FailedAttachment>,
    conn: &Connection,
    project_dir: &std::path::Path,
    env_profile: Option<&str>,
) -> (bool, String) {
    let Some(pos) = failed_attachments
        .iter()
        .position(|f| f.alias.eq_ignore_ascii_case(alias))
    else {
        return (false, format!("Attachment '{}' is not in the retry queue.", alias));
    };
    crate::engine::load_env_files(project_dir, env_profile);
    let _ = crate::workspace::apply_workspace_secrets(conn, project_dir, env_profile);
    let entry = failed_attachments[pos].entry.clone();
    match crate::workspace::attach_single_workspace_entry(conn, &entry, project_dir, env_profile) {
        Ok(()) => {
            failed_attachments.remove(pos);
            (
                true,
                format!("✓ Reconnected workspace catalog '{}' successfully!", alias),
            )
        }
        Err(kind) => {
            failed_attachments[pos].kind = kind;
            let detail = failed_attachments[pos].detail_message();
            (false, format!("⚠️ {}", detail))
        }
    }
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
    run_ide_loop_with_workspace(
        terminal,
        conn,
        project_dir,
        db_path,
        quack_disable_ssl,
        None,
        None,
    )
}

/// Workspace-aware IDE entrypoint supporting `--env <profile>` persistence and resilient
/// offline/missing-env placeholder nodes in Explorer.
pub fn run_ide_loop_with_workspace(
    terminal: &mut Terminal<ratatui::backend::CrosstermBackend<std::io::Stdout>>,
    conn: &Connection,
    project_dir: &PathBuf,
    db_path: &str,
    quack_disable_ssl: bool,
    env_profile: Option<&str>,
    workspace_report: Option<crate::workspace::WorkspaceApplyReport>,
) -> Result<()> {
    let is_quack = db_path.starts_with("quack:");
    let is_remote_db = is_quack || db_path.starts_with("md:") || db_path.starts_with("motherduck:") || db_path.starts_with("ducklake:");

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

    let mut failed_attachments: Vec<crate::workspace::FailedAttachment> = workspace_report
        .as_ref()
        .map(|r| r.failed_attachments.clone())
        .unwrap_or_default();
    let initial_ws_summary = workspace_report
        .as_ref()
        .and_then(|r| r.status_summary());

    let mut meta = load_explorer_metadata(conn, project_dir, is_quack);
    let mut tabs: Vec<EditorTab> = vec![EditorTab::new(meta.active_database.clone())];
    let mut active_tab_index: usize = 0;
    let mut db_picker_modal = DatabasePickerModalState::new();
    let mut exit_confirm_modal = ExitConfirmationModalState::new();

    let mut focus = FocusPane::Editor;
    let mut status_msg = initial_ws_summary
        .unwrap_or_else(|| String::from("Ready. Press 'Tab' to switch panes. 'Esc' to exit."));

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
    // Lazy-loaded when switching to Project mode (F4) to avoid spawning git subprocesses at startup
    let (mut git_branch, mut git_status): (Option<String>, std::collections::HashMap<PathBuf, String>) =
        (None, std::collections::HashMap::new());
    let mut expanded_project_groups: HashSet<String> = vec![
        "proj:schemas",
        "proj:sequences",
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
    let mut time_travel_modal = TimeTravelModalState::new();
    let mut quack_auth_modal = QuackAuthModalState::new();

    let mut explorer_state = ListState::default();
    let mut explorer_horizontal_scroll: usize = 0;
    let mut explorer_width_percent: u16 = 25;
    let mut explorer_items = build_db_explorer_items(
        &meta,
        &expanded_groups,
        &expanded_tables,
        conn,
        is_quack,
        &failed_attachments,
    );

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
    // Lazy-initialized on first copy/paste to avoid Wayland/X11 IPC handshake at startup
    let mut os_clipboard: Option<arboard::Clipboard> = None;

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
            } else if time_travel_modal.is_open {
                time_travel_modal.render(f);
            } else if quack_auth_modal.is_open {
                quack_auth_modal.render(f);
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
                            explorer_items = build_db_explorer_items(
                                &meta,
                                &expanded_groups,
                                &expanded_tables,
                                conn,
                                is_quack,
                                &failed_attachments,
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
            Event::Paste(pasted_text) => {
                if attach_modal.is_open {
                    attach_modal.paste_text(&pasted_text);
                } else if secret_modal.is_open {
                    secret_modal.paste_text(&pasted_text);
                } else if quack_auth_modal.is_open {
                    quack_auth_modal.paste_text(&pasted_text);
                } else if focus == FocusPane::Editor {
                    tabs[active_tab_index].textarea.insert_str(&pasted_text);
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
                    if key.code == KeyCode::Char('v')
                        && key.modifiers.contains(KeyModifiers::CONTROL)
                    {
                        if let Some(text) = get_clipboard_text(&mut os_clipboard) {
                            attach_modal.paste_text(&text);
                        }
                    } else if let Some(alias) = attach_modal.handle_key(key, conn) {
                        tabs[active_tab_index].target_database = alias.clone();
                        let save_note = if attach_modal.save_to_toml {
                            match attach_modal.persist_to_workspace(&alias, project_dir, env_profile) {
                                Ok(msg) => format!(" ({})", msg),
                                Err(e) => format!(" (⚠️ save to duckpack.toml failed: {})", e),
                            }
                        } else {
                            String::new()
                        };
                        status_msg = format!(
                            "Attached database '{}' successfully and switched active tab context.{}",
                            alias, save_note
                        );
                        trigger_explorer_refresh = true;
                    }
                } else if secret_modal.is_open {
                    if key.code == KeyCode::Char('v')
                        && key.modifiers.contains(KeyModifiers::CONTROL)
                    {
                        if let Some(text) = get_clipboard_text(&mut os_clipboard) {
                            secret_modal.paste_text(&text);
                        }
                    } else if let Some(msg) =
                        secret_modal.handle_key_with_env(key, conn, project_dir, env_profile)
                    {
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
                } else if time_travel_modal.is_open {
                    if let Some((db_name, sql)) = time_travel_modal.handle_key(key) {
                        open_query_in_tabs(
                            &mut tabs,
                            &mut active_tab_index,
                            sql,
                            Some(db_name.clone()),
                        );
                        focus = FocusPane::Editor;
                        execute_query = true;
                        status_msg =
                            format!("Executed DuckLake time-travel query on '{}'", db_name);
                    }
                } else if quack_auth_modal.is_open {
                    if key.code == KeyCode::Char('v')
                        && key.modifiers.contains(KeyModifiers::CONTROL)
                    {
                        if let Some(text) = get_clipboard_text(&mut os_clipboard) {
                            quack_auth_modal.paste_text(&text);
                        }
                    } else if let Some(msg) = quack_auth_modal.handle_key(key, conn, project_dir) {
                        status_msg = msg;
                        trigger_explorer_refresh = true;
                    }
                } else if key.code == KeyCode::Char('a')
                    && key.modifiers.contains(KeyModifiers::CONTROL)
                {
                    attach_modal.reset();
                } else if key.code == KeyCode::Char('k')
                    && key.modifiers.contains(KeyModifiers::CONTROL)
                {
                    secret_modal.reset();
                } else if key.code == KeyCode::Char('u')
                    && key.modifiers.contains(KeyModifiers::CONTROL)
                {
                    let tab_cat = tabs
                        .get(active_tab_index)
                        .map(|t| t.target_database.as_str())
                        .filter(|c| !c.is_empty() && *c != "memory");
                    quack_auth_modal.open_with_env(conn, project_dir, tab_cat, env_profile);
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
                        explorer_items = build_db_explorer_items(
                            &meta,
                            &expanded_groups,
                            &expanded_tables,
                            conn,
                            is_quack,
                            &failed_attachments,
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
                                explorer_items = build_db_explorer_items(
                                    &meta,
                                    &expanded_groups,
                                    &expanded_tables,
                                    conn,
                                    is_quack,
                                    &failed_attachments,
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
                                    let selected_node = explorer_state
                                        .selected()
                                        .and_then(|idx| explorer_items.get(idx));
                                    new_asset_modal.open_for_node(selected_node);
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
                                    let selected_failed_alias = explorer_state
                                        .selected()
                                        .and_then(|idx| explorer_items.get(idx))
                                        .and_then(|node| match &node.node_type {
                                            NodeType::FailedAttachment { alias, .. } => {
                                                Some(alias.clone())
                                            }
                                            _ => None,
                                        });
                                    if let Some(alias) = selected_failed_alias {
                                        let (ok, msg) = retry_failed_workspace_attachment(
                                            &alias,
                                            &mut failed_attachments,
                                            conn,
                                            project_dir,
                                            env_profile,
                                        );
                                        if ok {
                                            tabs[active_tab_index].target_database = alias;
                                        }
                                        status_msg = msg;
                                        trigger_explorer_refresh = true;
                                    } else {
                                        refresh_attached_catalog(conn, db_path, quack_disable_ssl);
                                        trigger_explorer_refresh = true;
                                        status_msg = "Explorer refreshed.".to_string();
                                    }
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
                                                let query = execution::build_cloud_file_select_sql(path);
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
                                                    name.replace('\'', "''")
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
                                            NodeType::Snapshot {
                                                database,
                                                snapshot_id,
                                                ..
                                            } => {
                                                let safe_db = database.replace('\'', "''");
                                                let query = format!(
                                                    "SELECT * FROM ducklake_snapshots('{}') WHERE snapshot_id = {};",
                                                    safe_db, snapshot_id
                                                );
                                                open_query_in_tabs(
                                                    &mut tabs,
                                                    &mut active_tab_index,
                                                    query,
                                                    Some(database.clone()),
                                                );
                                                focus = FocusPane::Editor;
                                                execute_query = true;
                                                status_msg = format!(
                                                    "Inspecting DuckLake '{}' snapshot v{}",
                                                    database, snapshot_id
                                                );
                                            }
                                            NodeType::Group
                                                if item_name == "Snapshots"
                                                    || item_name.ends_with(":Snapshots") =>
                                            {
                                                let db_name = item_name
                                                    .strip_suffix(":Snapshots")
                                                    .unwrap_or(&meta.active_database);
                                                let safe_db = db_name.replace('\'', "''");
                                                let query = format!(
                                                    "SELECT * FROM ducklake_snapshots('{}') ORDER BY snapshot_id DESC LIMIT 50;",
                                                    safe_db
                                                );
                                                open_query_in_tabs(
                                                    &mut tabs,
                                                    &mut active_tab_index,
                                                    query,
                                                    Some(db_name.to_string()),
                                                );
                                                focus = FocusPane::Editor;
                                                execute_query = true;
                                                status_msg = format!(
                                                    "Queried DuckLake snapshots for '{}'",
                                                    db_name
                                                );
                                            }
                                            _ => {}
                                        }
                                    }
                                }
                            } else if key.code == KeyCode::Char('t') {
                                if let Some(selected) = explorer_state.selected() {
                                    if selected < explorer_items.len() {
                                        let item_type = &explorer_items[selected].node_type;
                                        let item_name = &explorer_items[selected].name;
                                        match item_type {
                                            NodeType::Table {
                                                database,
                                                schema,
                                                is_temporary,
                                            } => {
                                                let db_name = database
                                                    .as_deref()
                                                    .unwrap_or(&meta.active_database);
                                                let schema_name =
                                                    schema.as_deref().unwrap_or("main");
                                                if *is_temporary {
                                                    status_msg = "⚠️ Time-Travel ('t') is not available for temporary tables.".to_string();
                                                } else if let Some(db_info) = meta
                                                    .databases
                                                    .iter()
                                                    .find(|d| d.name.eq_ignore_ascii_case(db_name))
                                                {
                                                    if db_info.db_type.eq_ignore_ascii_case("ducklake")
                                                        || !db_info.snapshots.is_empty()
                                                    {
                                                        time_travel_modal.open_for_ducklake(
                                                            db_info,
                                                            Some((schema_name, item_name)),
                                                            None,
                                                        );
                                                    } else {
                                                        status_msg = format!(
                                                            "⚠️ Time-Travel ('t') requires a DuckLake catalog ('{}' is '{}').",
                                                            db_info.name, db_info.db_type
                                                        );
                                                    }
                                                }
                                            }
                                            NodeType::Snapshot {
                                                database,
                                                snapshot_id,
                                                ..
                                            } => {
                                                if let Some(db_info) = meta
                                                    .databases
                                                    .iter()
                                                    .find(|d| d.name.eq_ignore_ascii_case(database))
                                                {
                                                    time_travel_modal.open_for_ducklake(
                                                        db_info,
                                                        None,
                                                        Some(*snapshot_id),
                                                    );
                                                }
                                            }
                                            NodeType::Group
                                                if item_name == "Snapshots"
                                                    || item_name.ends_with(":Snapshots") =>
                                            {
                                                let db_name = item_name
                                                    .strip_suffix(":Snapshots")
                                                    .unwrap_or(&meta.active_database);
                                                if let Some(db_info) = meta
                                                    .databases
                                                    .iter()
                                                    .find(|d| d.name.eq_ignore_ascii_case(db_name))
                                                {
                                                    time_travel_modal
                                                        .open_for_ducklake(db_info, None, None);
                                                }
                                            }
                                            _ => {
                                                status_msg = "⚠️ Select a DuckLake table or snapshot node and press 't' for Time-Travel.".to_string();
                                            }
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
                                                        &format!(
                                                            "DETACH {};",
                                                            crate::sql_utils::quote_ident(db_name)
                                                        ),
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
                                                                db_name,
                                                                crate::sql_utils::redact_connection_credentials(&e.to_string())
                                                            );
                                                        }
                                                    }
                                                }
                                            }
                                            NodeType::CloudSecret { name, .. } => {
                                                match conn.execute(
                                                    &format!(
                                                        "DROP SECRET IF EXISTS {};",
                                                        crate::sql_utils::quote_ident(name)
                                                    ),
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
                                                            name,
                                                            crate::sql_utils::redact_connection_credentials(&e.to_string())
                                                        );
                                                    }
                                                }
                                            }
                                            NodeType::CloudBucket { bucket } => {
                                                crate::env_store::remove_session_cloud_bucket(
                                                    bucket,
                                                );
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
            if let Some(outcome) = execution::execute_ide_query(
                conn,
                active_tab,
                &mut meta,
                &queries_dir,
                db_path,
                is_quack,
                is_remote_db,
                quack_disable_ssl,
                trigger_pagination,
            ) {
                status_msg = outcome.status_msg;
                if outcome.trigger_explorer_refresh {
                    trigger_explorer_refresh = true;
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
                explorer_items = build_db_explorer_items(
                    &meta,
                    &expanded_groups,
                    &expanded_tables,
                    conn,
                    is_quack,
                    &failed_attachments,
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
                            explorer_items = build_db_explorer_items(
                                &meta,
                                &expanded_groups,
                                &expanded_tables,
                                conn,
                                is_quack,
                                &failed_attachments,
                            );
                            explorer_state
                                .select(Some(selected.min(explorer_items.len().saturating_sub(1))));
                        }
                        NodeType::Group => {
                            if explorer_mode == ExplorerMode::Project {
                                if item_name.ends_with(":empty") {
                                    new_asset_modal.open_for_group_name(&item_name);
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
                                explorer_items = build_db_explorer_items(
                                    &meta,
                                    &expanded_groups,
                                    &expanded_tables,
                                    conn,
                                    is_quack,
                                    &failed_attachments,
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
                            explorer_items = build_db_explorer_items(
                                &meta,
                                &expanded_groups,
                                &expanded_tables,
                                conn,
                                is_quack,
                                &failed_attachments,
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
                        NodeType::Snapshot {
                            database,
                            snapshot_id,
                            ..
                        } => {
                            let safe_db = database.replace('\'', "''");
                            let query = format!(
                                "SELECT * FROM ducklake_snapshots('{}') WHERE snapshot_id = {};",
                                safe_db, snapshot_id
                            );
                            open_query_in_tabs(
                                &mut tabs,
                                &mut active_tab_index,
                                query,
                                Some(database.clone()),
                            );
                            focus = FocusPane::Editor;
                            status_msg = format!(
                                "Inspecting DuckLake '{}' snapshot v{} (press Ctrl+E/F5 to run, or 't' in Explorer for Time-Travel)",
                                database, snapshot_id
                            );
                        }
                        NodeType::AddSecretAction => {
                            secret_modal.reset();
                        }
                        NodeType::AddBucketAction => {
                            add_bucket_modal.reset();
                        }
                        NodeType::QuackRbacAction => {
                            let tab_cat = tabs
                                .get(active_tab_index)
                                .map(|t| t.target_database.as_str())
                                .filter(|c| !c.is_empty() && *c != "memory");
                            quack_auth_modal.open_with_env(conn, project_dir, tab_cat, env_profile);
                        }
                        NodeType::FailedAttachment { alias, .. } => {
                            let (ok, msg) = retry_failed_workspace_attachment(
                                &alias,
                                &mut failed_attachments,
                                conn,
                                project_dir,
                                env_profile,
                            );
                            if ok {
                                tabs[active_tab_index].target_database = alias.clone();
                                meta = load_explorer_metadata(conn, project_dir, is_quack);
                                dictionary = meta.dictionary.clone();
                                expanded_groups.insert(format!("db:{}", alias));
                                expanded_groups.insert(format!("{}:Tables", alias));
                            }
                            explorer_items = build_db_explorer_items(
                                &meta,
                                &expanded_groups,
                                &expanded_tables,
                                conn,
                                is_quack,
                                &failed_attachments,
                            );
                            explorer_state.select(Some(
                                selected.min(explorer_items.len().saturating_sub(1)),
                            ));
                            status_msg = msg;
                        }
                        NodeType::CloudSecret { name, .. } => {
                            let query = format!(
                                "SELECT * FROM duckdb_secrets() WHERE name = {};",
                                crate::sql_utils::quote_literal(&name)
                            );
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
                            explorer_items = build_db_explorer_items(
                                &meta,
                                &expanded_groups,
                                &expanded_tables,
                                conn,
                                is_quack,
                                &failed_attachments,
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
                                explorer_items = build_db_explorer_items(
                                    &meta,
                                    &expanded_groups,
                                    &expanded_tables,
                                    conn,
                                    is_quack,
                                    &failed_attachments,
                                );
                                explorer_state.select(Some(
                                    selected.min(explorer_items.len().saturating_sub(1)),
                                ));
                            } else {
                                let query = execution::build_cloud_file_select_sql(&path);
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
mod tests;
