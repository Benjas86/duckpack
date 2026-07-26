use anyhow::Result;
use duckdb::Connection;
use ratatui::{
    crossterm::event::{self, Event, KeyCode, KeyModifiers, MouseEventKind, MouseButton},
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, BorderType, Clear, List, ListItem, ListState, Paragraph, Row, Table, Tabs, Wrap},
    Terminal,
};
use std::path::PathBuf;
use tui_textarea::{Input, Key, TextArea};

/// Manages the currently focused UI pane inside the IDE.
/// Determines which component receives keyboard inputs.
#[derive(PartialEq)]
enum FocusPane {
    Explorer,
    Editor,
    Results,
}

/// Categorizes the type of entity currently being rendered in the Explorer tree.
#[derive(Clone, Debug, PartialEq)]
enum NodeType {
    Group,
    Table,
    View,
    Query,
    Macro,
    Column { data_type: String },
}

/// Represents a single renderable node in the Explorer sidebar.
/// Tracks its hierarchical depth (`level`) and expandability state for collapsing/expanding tables.
#[derive(Clone, Debug)]
struct ExplorerNode {
    name: String,
    display: String,
    level: usize,
    is_expandable: bool,
    is_expanded: bool,
    node_type: NodeType,
}

/// Represents an individual open Query Tab within the IDE.
/// Contains an isolated text area and its own dedicated query results and scrolling state.
struct EditorTab<'a> {
    textarea: TextArea<'a>,
    active_file: Option<PathBuf>,
    query_results: Vec<Vec<String>>,
    column_names: Vec<String>,
    horizontal_scroll: usize,
    results_state: ratatui::widgets::TableState,
    base_query: Option<String>,
    pagination_offset: usize,
    pagination_limit: usize,
    is_paginated: bool,
}

impl<'a> EditorTab<'a> {
    fn new() -> Self {
        let mut textarea = TextArea::default();
        textarea.set_search_pattern("(?i)\\b(SELECT|FROM|WHERE|INSERT|UPDATE|DELETE|JOIN|ON|GROUP BY|ORDER BY|LIMIT|CREATE|TABLE|DROP|ALTER|VALUES|AND|OR|NOT|AS|IN|IS|NULL|SET|INTO|VIEW|INDEX|SHOW|PRAGMA|DESCRIBE|ATTACH|USE|MACRO)\\b").unwrap_or(());
        textarea.set_search_style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD));
        textarea.set_line_number_style(Style::default().fg(Color::DarkGray));
        Self {
            textarea,
            active_file: None,
            query_results: Vec::new(),
            column_names: Vec::new(),
            horizontal_scroll: 0,
            results_state: ratatui::widgets::TableState::default(),
            base_query: None,
            pagination_offset: 0,
            pagination_limit: 500,
            is_paginated: false,
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
) -> Result<()> {
    let mut tabs: Vec<EditorTab> = vec![EditorTab::new()];
    let mut active_tab_index: usize = 0;

    let mut focus = FocusPane::Editor;
    let mut status_msg = String::from("Ready. Press 'Tab' to switch panes. 'Esc' to exit.");

    // Load tables/views
    let mut tables: Vec<String> = Vec::new();
    let mut views: Vec<String> = Vec::new();
    if let Ok(mut stmt) = conn.prepare("SELECT table_schema, table_name, table_type FROM information_schema.tables WHERE table_schema NOT IN ('information_schema', 'pg_catalog') ORDER BY table_type, table_schema, table_name") {
        if let Ok(mut rows) = stmt.query([]) {
            while let Ok(Some(row)) = rows.next() {
                if let (Ok(schema), Ok(name), Ok(typ)) = (row.get::<_, String>(0), row.get::<_, String>(1), row.get::<_, String>(2)) {
                    let display_name = if schema == "main" { name.clone() } else { format!("{}.{}", schema, name) };
                    if typ == "BASE TABLE" {
                        tables.push(display_name);
                    } else if typ == "VIEW" {
                        views.push(display_name);
                    }
                }
            }
        }
    }

    let mut macros: Vec<String> = Vec::new();
    if let Ok(mut stmt) = conn.prepare("SELECT CASE WHEN schema_name = 'main' THEN function_name ELSE schema_name || '.' || function_name END FROM duckdb_functions() WHERE function_type = 'macro' AND internal = false ORDER BY 1") {
        if let Ok(mut rows) = stmt.query([]) {
            while let Ok(Some(row)) = rows.next() {
                if let Ok(name) = row.get::<_, String>(0) {
                    macros.push(name);
                }
            }
        }
    }

    // Load autocomplete dictionary (tables + columns)
    let mut dictionary: Vec<String> = tables.clone();
    dictionary.extend(views.clone());
    if let Ok(mut stmt) = conn.prepare("SELECT column_name FROM information_schema.columns WHERE table_schema NOT IN ('information_schema', 'pg_catalog')") {
        if let Ok(mut rows) = stmt.query([]) {
            while let Ok(Some(row)) = rows.next() {
                if let Ok(col) = row.get::<_, String>(0) {
                    if !dictionary.contains(&col) {
                        dictionary.push(col);
                    }
                }
            }
        }
    }
    let mut autocomplete_matches: Vec<String> = Vec::new();
    let mut autocomplete_index = 0;
    let mut autocomplete_original_word = String::new();

    // Load queries
    let mut queries: Vec<String> = Vec::new();
    let queries_dir = project_dir.join("queries");
    if queries_dir.exists() {
        if let Ok(entries) = std::fs::read_dir(&queries_dir) {
            for entry in entries.flatten() {
                if let Some(ext) = entry.path().extension() {
                    if ext == "sql" {
                        if let Some(name) = entry.file_name().to_str() {
                            queries.push(name.to_string());
                        }
                    }
                }
            }
        }
    }
    queries.sort();

    let mut explorer_state = ListState::default();
    let mut explorer_items: Vec<ExplorerNode> = Vec::new();
    let mut explorer_horizontal_scroll: usize = 0;

    if !tables.is_empty() {
        explorer_items.push(ExplorerNode {
            name: "Tables".to_string(),
            display: format!("📦 Tables ({})", tables.len()),
            level: 0,
            is_expandable: true,
            is_expanded: true,
            node_type: NodeType::Group,
        });
        for t in &tables {
            explorer_items.push(ExplorerNode {
                name: t.clone(),
                display: format!("  ├─ {}", t),
                level: 1,
                is_expandable: true,
                is_expanded: false,
                node_type: NodeType::Table,
            });
        }
    }

    if !views.is_empty() {
        explorer_items.push(ExplorerNode {
            name: "Views".to_string(),
            display: format!("👁️ Views ({})", views.len()),
            level: 0,
            is_expandable: true,
            is_expanded: true,
            node_type: NodeType::Group,
        });
        for v in &views {
            explorer_items.push(ExplorerNode {
                name: v.clone(),
                display: format!("  ├─ {}", v),
                level: 1,
                is_expandable: true,
                is_expanded: false,
                node_type: NodeType::View,
            });
        }
    }

    if !macros.is_empty() {
        explorer_items.push(ExplorerNode {
            name: "Macros".to_string(),
            display: format!("🔧 Macros ({})", macros.len()),
            level: 0,
            is_expandable: true,
            is_expanded: true,
            node_type: NodeType::Group,
        });
        for m in &macros {
            explorer_items.push(ExplorerNode {
                name: m.clone(),
                display: format!("  ├─ {}", m),
                level: 1,
                is_expandable: false,
                is_expanded: false,
                node_type: NodeType::Macro,
            });
        }
    }

    if !queries.is_empty() {
        explorer_items.push(ExplorerNode {
            name: "Queries".to_string(),
            display: format!("📄 Queries ({})", queries.len()),
            level: 0,
            is_expandable: true,
            is_expanded: true,
            node_type: NodeType::Group,
        });
        for q in &queries {
            explorer_items.push(ExplorerNode {
                name: q.clone(),
                display: format!("  ├─ {}", q),
                level: 1,
                is_expandable: false,
                is_expanded: false,
                node_type: NodeType::Query,
            });
        }
    }

    if !explorer_items.is_empty() {
        explorer_state.select(Some(0));
    }

    let mut current_editor_rect = Rect::default();
    let mut current_explorer_rect = Rect::default();
    let mut current_tabs_rect = Rect::default();
    let mut current_results_rect = Rect::default();
    let mut show_help = false;
    let mut show_schema_visualizer = false;
    let mut schema_ascii_tree = String::new();
    let mut schema_scroll = 0;
    let mut max_schema_scroll = 0;

    loop {
        terminal.draw(|f| {
            let main_chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Length(3), Constraint::Min(0)].as_ref())
                .split(f.area());

            let ssh_prefix = if std::env::var("SSH_CLIENT").is_ok() { "[SSH Remote] " } else { "" };
            let top_header = Paragraph::new("")
                .block(Block::default().borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .title_top(Line::from(format!(" Target DB: {}{} ", ssh_prefix, db_path)).alignment(Alignment::Right).style(Style::default().fg(Color::Cyan)))
                    .title_top(Line::from(format!(" DuckPack v{} ", env!("CARGO_PKG_VERSION"))).alignment(Alignment::Center).style(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)))
                );
            f.render_widget(top_header, main_chunks[0]);

            let chunks = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(20), Constraint::Percentage(80)].as_ref())
                .split(main_chunks[1]);

            let right_chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Length(3), Constraint::Min(10), Constraint::Percentage(45), Constraint::Length(2)].as_ref())
                .split(chunks[1]);

            let max_item_width = explorer_items.iter().map(|item| {
                let indent = item.level * 2;
                let expand_indicator = if item.is_expandable { 2 } else { 0 };
                indent + expand_indicator + item.display.chars().count()
            }).max().unwrap_or(0);
            
            let explorer_inner_width = chunks[0].width.saturating_sub(2) as usize;
            let max_scroll = max_item_width.saturating_sub(explorer_inner_width);
            if explorer_horizontal_scroll > max_scroll {
                explorer_horizontal_scroll = max_scroll;
            }

            // Explorer Pane
            let mut list_items = Vec::new();
            for item in &explorer_items {
                let indent = "  ".repeat(item.level);
                let expand_indicator = if item.is_expandable {
                    if item.is_expanded { "▼ " } else { "▶ " }
                } else {
                    ""
                };
                let display_text = format!("{}{}{}", indent, expand_indicator, item.display)
                    .chars()
                    .skip(explorer_horizontal_scroll)
                    .collect::<String>();
                list_items.push(ListItem::new(display_text));
            }

            let explorer_border_style = if focus == FocusPane::Explorer {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default()
            };

            let explorer_list = List::new(list_items)
                .block(Block::default().borders(Borders::ALL).title(" Explorer ").border_style(explorer_border_style))
                .highlight_style(Style::default().bg(Color::DarkGray).add_modifier(Modifier::BOLD));
            f.render_stateful_widget(explorer_list, chunks[0], &mut explorer_state);
            current_explorer_rect = chunks[0];

            // Tabs Pane
            let tab_titles: Vec<Line> = tabs.iter().enumerate().map(|(i, tab)| {
                let name = if let Some(ref path) = tab.active_file {
                    path.file_name().unwrap_or(std::ffi::OsStr::new("unknown")).to_string_lossy().to_string()
                } else {
                    format!("New Query {}", i + 1)
                };
                let style = if i == active_tab_index {
                    Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::DarkGray)
                };
                Line::from(vec![Span::styled(format!(" {} ", name), style)])
            }).collect();
            
            let tabs_widget = Tabs::new(tab_titles)
                .block(Block::default().borders(Borders::ALL).title(" Open Tabs "))
                .select(active_tab_index)
                .highlight_style(Style::default().fg(Color::Yellow).add_modifier(Modifier::REVERSED));
            f.render_widget(tabs_widget, right_chunks[0]);
            current_tabs_rect = right_chunks[0];

            // Editor Pane
            let active_tab = &mut tabs[active_tab_index];
            let editor_title = if let Some(ref path) = active_tab.active_file {
                format!(" Editor [{}] ", path.file_name().unwrap_or(std::ffi::OsStr::new("unknown")).to_string_lossy())
            } else {
                " Editor [New Query] ".to_string()
            };

            let sql_keywords = "(?i)\\b(SELECT|FROM|WHERE|INSERT|UPDATE|DELETE|JOIN|ON|GROUP BY|ORDER BY|LIMIT|CREATE|TABLE|DROP|ALTER|VALUES|AND|OR|NOT|AS|IN|IS|NULL|SET|INTO|VIEW|INDEX|SHOW|PRAGMA|DESCRIBE|ATTACH|USE|MACRO)\\b";
            if active_tab.textarea.is_selecting() {
                active_tab.textarea.set_search_pattern("a^").unwrap_or(());
            } else {
                active_tab.textarea.set_search_pattern(sql_keywords).unwrap_or(());
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
                    .title_bottom(Line::from(" ➕ New (Ctrl+T)  |  ❌ Close (Ctrl+W)  |  💾 Save (Ctrl+S)  |  ✨ Format (Ctrl+F) ").alignment(Alignment::Left))
                    .title_bottom(Line::from(" ▶ Run (F5) ").alignment(Alignment::Right))
                    .border_style(editor_border_style)
            );
            f.render_widget(active_tab.textarea.widget(), right_chunks[1]);
            current_editor_rect = right_chunks[1];

            // Results Pane
            let results_border_style = if focus == FocusPane::Results {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default()
            };

            let title = if tabs[active_tab_index].is_paginated {
                let offset = tabs[active_tab_index].pagination_offset;
                let limit = tabs[active_tab_index].pagination_limit;
                format!(" Results (Rows {} - {}) ", offset + 1, offset + limit)
            } else {
                " Results ".to_string()
            };

            let results_block = Block::default()
                .borders(Borders::ALL)
                .title(title)
                .title_bottom(Line::from(" [?] Help ").alignment(Alignment::Right))
                .border_style(results_border_style);

            let active_tab = &mut tabs[active_tab_index];

            if active_tab.query_results.is_empty() && active_tab.column_names.is_empty() {
                f.render_widget(Paragraph::new("No results.").block(results_block), right_chunks[2]);
            } else {
                let display_cols = active_tab.column_names.len().saturating_sub(active_tab.horizontal_scroll);
                let display_names = if active_tab.horizontal_scroll < active_tab.column_names.len() {
                    &active_tab.column_names[active_tab.horizontal_scroll..]
                } else {
                    &[]
                };
                
                let mut widths = Vec::new();
                for (i, name) in display_names.iter().enumerate() {
                    let mut max_len = name.len();
                    for row in &active_tab.query_results {
                        if let Some(cell) = row.get(active_tab.horizontal_scroll + i) {
                            max_len = max_len.max(cell.len());
                        }
                    }
                    widths.push(Constraint::Length((max_len as u16 + 2).min(30)));
                }

                let header = Row::new(display_names.iter().map(|c| c.as_str())).style(Style::default().add_modifier(Modifier::BOLD));
                let rows: Vec<Row> = active_tab.query_results.iter().map(|r| {
                    let display_cells = if active_tab.horizontal_scroll < r.len() {
                        &r[active_tab.horizontal_scroll..]
                    } else {
                        &[]
                    };
                    Row::new(display_cells.iter().map(|c| c.as_str()))
                }).collect();
                
                let table = Table::new(rows, widths).header(header).block(results_block)
                    .row_highlight_style(Style::default().bg(Color::DarkGray).add_modifier(Modifier::BOLD));
                f.render_stateful_widget(table, right_chunks[2], &mut active_tab.results_state);
            }
            current_results_rect = right_chunks[2];

            if show_help {
                let popup_area = ratatui::layout::Rect {
                    x: f.area().width / 4,
                    y: f.area().height / 4,
                    width: f.area().width / 2,
                    height: f.area().height / 2,
                };
                f.render_widget(ratatui::widgets::Clear, popup_area);
                
                let help_lines = vec![
                    Line::from(vec![Span::styled(" [?]       ", Style::default().fg(Color::Cyan)), Span::raw("Toggle Help Menu")]),
                    Line::from(""),
                    Line::from(vec![Span::styled(" Ctrl+T    ", Style::default().fg(Color::Cyan)), Span::raw("New Tab")]),
                    Line::from(vec![Span::styled(" Ctrl+W    ", Style::default().fg(Color::Cyan)), Span::raw("Close Tab")]),
                    Line::from(vec![Span::styled(" Ctrl+N/P  ", Style::default().fg(Color::Cyan)), Span::raw("Next/Prev Tab")]),
                    Line::from(vec![Span::styled(" Ctrl+E/F5 ", Style::default().fg(Color::Cyan)), Span::raw("Execute Query (or selected text)")]),
                    Line::from(vec![Span::styled(" Ctrl+V    ", Style::default().fg(Color::Cyan)), Span::raw("Visualize Schema")]),
                    Line::from(vec![Span::styled(" Ctrl+S    ", Style::default().fg(Color::Cyan)), Span::raw("Save Query")]),
                    Line::from(vec![Span::styled(" Ctrl+F    ", Style::default().fg(Color::Cyan)), Span::raw("Format Query")]),
                    Line::from(vec![Span::styled(" Ctrl+Space", Style::default().fg(Color::Cyan)), Span::raw("Autocomplete")]),
                    Line::from(vec![Span::styled(" n / p     ", Style::default().fg(Color::Cyan)), Span::raw("Next/Prev Page (in Results pane)")]),
                    Line::from(""),
                    Line::from(vec![Span::styled(" Mouse     ", Style::default().fg(Color::Cyan)), Span::raw("Click tabs/buttons, scroll panes")]),
                ];

                let help_paragraph = Paragraph::new(help_lines)
                        .block(Block::default()
                        .borders(Borders::ALL)
                        .title(" Help Menu ")
                        .border_style(Style::default().fg(Color::Cyan)))
                        .alignment(Alignment::Left);
                f.render_widget(help_paragraph, popup_area);
            } else if show_schema_visualizer {
                let popup_area = ratatui::layout::Rect {
                    x: f.area().width / 8,
                    y: f.area().height / 8,
                    width: (f.area().width * 3) / 4,
                    height: (f.area().height * 3) / 4,
                };
                f.render_widget(ratatui::widgets::Clear, popup_area);
                
                let lines: Vec<Line> = schema_ascii_tree.lines().map(|l| Line::from(l.to_string())).collect();
                max_schema_scroll = lines.len().saturating_sub((popup_area.height as usize).saturating_sub(2)) as u16;

                let tree_paragraph = Paragraph::new(lines)
                        .block(Block::default()
                        .borders(Borders::ALL)
                        .title(format!(" Schema Visualizer (Scroll: {}/{}) ", schema_scroll, max_schema_scroll))
                        .border_style(Style::default().fg(Color::Cyan)))
                        .alignment(Alignment::Left)
                        .scroll((schema_scroll, 0));
                f.render_widget(tree_paragraph, popup_area);
            }

            // Status Bar
            let status = Paragraph::new(format!(" {}", status_msg))
                .style(Style::default().fg(Color::Black).bg(Color::Cyan));
            f.render_widget(status, right_chunks[3]);

        })?;

        let mut execute_query = false;
        let mut save_query = false;
        let mut export_query = false;
        let mut format_query = false;
        let mut new_tab = false;
        let mut close_tab = false;
        let mut trigger_explorer_action = false;
        let mut trigger_pagination: Option<i32> = None;
        let ev = event::read()?;

        match ev {
            Event::Mouse(mouse) => {
                if mouse.kind == MouseEventKind::Down(MouseButton::Left) {
                    if mouse.row == current_editor_rect.bottom().saturating_sub(1) {
                        if mouse.column >= current_editor_rect.right().saturating_sub(15) && mouse.column <= current_editor_rect.right() {
                            execute_query = true;
                        }
                        if mouse.column >= current_editor_rect.left() && mouse.column <= current_editor_rect.left() + 18 {
                            new_tab = true;
                        }
                        if mouse.column > current_editor_rect.left() + 18 && mouse.column <= current_editor_rect.left() + 40 {
                            close_tab = true;
                        }
                        if mouse.column > current_editor_rect.left() + 40 && mouse.column <= current_editor_rect.left() + 61 {
                            save_query = true;
                        }
                        if mouse.column > current_editor_rect.left() + 61 && mouse.column <= current_editor_rect.left() + 85 {
                            format_query = true;
                        }
                    } else if mouse.row == current_results_rect.bottom().saturating_sub(1) {
                        if mouse.column >= current_results_rect.right().saturating_sub(10) && mouse.column <= current_results_rect.right() {
                            show_help = !show_help;
                        }
                    } else if mouse.row >= current_editor_rect.top() && mouse.row < current_editor_rect.bottom() {
                        if mouse.column >= current_editor_rect.left() && mouse.column <= current_editor_rect.right() {
                            focus = FocusPane::Editor;
                        }
                    }

                    // Explorer Click
                    if mouse.row >= current_explorer_rect.top() + 1 && mouse.row < current_explorer_rect.bottom() - 1 {
                        if mouse.column >= current_explorer_rect.left() + 1 && mouse.column < current_explorer_rect.right() - 1 {
                            let list_offset = explorer_state.offset();
                            let clicked_idx = (mouse.row - current_explorer_rect.top() - 1) as usize + list_offset;
                            if clicked_idx < explorer_items.len() {
                                focus = FocusPane::Explorer;
                                explorer_state.select(Some(clicked_idx));
                                trigger_explorer_action = true;
                            }
                        }
                    }

                    // Tabs Click
                    if mouse.row == current_tabs_rect.top() + 1 && mouse.column >= current_tabs_rect.left() + 1 && mouse.column < current_tabs_rect.right() - 1 {
                        let mut current_x = current_tabs_rect.left() + 1;
                        for (i, tab) in tabs.iter().enumerate() {
                            let tab_name = if let Some(ref path) = tab.active_file {
                                path.file_name().unwrap_or(std::ffi::OsStr::new("unknown")).to_string_lossy().to_string()
                            } else {
                                format!("New Query {}", i + 1)
                            };
                            let tab_width = tab_name.chars().count() as u16 + 2; // " name "
                            let display_width = tab_width + 3; // Ratatui divider " | "
                            if mouse.column >= current_x && mouse.column < current_x + display_width {
                                active_tab_index = i;
                                focus = FocusPane::Editor;
                                break;
                            }
                            current_x += display_width;
                        }
                    }
                } else if mouse.kind == MouseEventKind::ScrollDown {
                    if mouse.modifiers.contains(KeyModifiers::SHIFT) {
                        if mouse.row >= current_explorer_rect.top() && mouse.row <= current_explorer_rect.bottom() && mouse.column >= current_explorer_rect.left() && mouse.column <= current_explorer_rect.right() {
                            explorer_horizontal_scroll += 1;
                        } else if mouse.row >= current_results_rect.top() && mouse.row <= current_results_rect.bottom() && mouse.column >= current_results_rect.left() && mouse.column <= current_results_rect.right() {
                            tabs[active_tab_index].horizontal_scroll += 1;
                        } else if mouse.row >= current_editor_rect.top() && mouse.row <= current_editor_rect.bottom() && mouse.column >= current_editor_rect.left() && mouse.column <= current_editor_rect.right() {
                            tabs[active_tab_index].textarea.scroll((0, 1));
                        }
                    } else {
                        if mouse.row >= current_explorer_rect.top() && mouse.row <= current_explorer_rect.bottom() && mouse.column >= current_explorer_rect.left() && mouse.column <= current_explorer_rect.right() {
                            if let Some(selected) = explorer_state.selected() {
                                if selected < explorer_items.len().saturating_sub(1) {
                                    explorer_state.select(Some(selected + 1));
                                }
                            } else if !explorer_items.is_empty() {
                                explorer_state.select(Some(0));
                            }
                        } else if mouse.row >= current_results_rect.top() && mouse.row <= current_results_rect.bottom() && mouse.column >= current_results_rect.left() && mouse.column <= current_results_rect.right() {
                            let active_tab = &mut tabs[active_tab_index];
                            if let Some(selected) = active_tab.results_state.selected() {
                                if selected < active_tab.query_results.len().saturating_sub(1) {
                                    active_tab.results_state.select(Some(selected + 1));
                                }
                            } else if !active_tab.query_results.is_empty() {
                                active_tab.results_state.select(Some(0));
                            }
                        } else if mouse.row >= current_editor_rect.top() && mouse.row <= current_editor_rect.bottom() && mouse.column >= current_editor_rect.left() && mouse.column <= current_editor_rect.right() {
                            tabs[active_tab_index].textarea.scroll((1, 0));
                        }
                    }
                } else if mouse.kind == MouseEventKind::ScrollUp {
                    if mouse.modifiers.contains(KeyModifiers::SHIFT) {
                        if mouse.row >= current_explorer_rect.top() && mouse.row <= current_explorer_rect.bottom() && mouse.column >= current_explorer_rect.left() && mouse.column <= current_explorer_rect.right() {
                            if explorer_horizontal_scroll > 0 { explorer_horizontal_scroll -= 1; }
                        } else if mouse.row >= current_results_rect.top() && mouse.row <= current_results_rect.bottom() && mouse.column >= current_results_rect.left() && mouse.column <= current_results_rect.right() {
                            let active_tab = &mut tabs[active_tab_index];
                            if active_tab.horizontal_scroll > 0 { active_tab.horizontal_scroll -= 1; }
                        } else if mouse.row >= current_editor_rect.top() && mouse.row <= current_editor_rect.bottom() && mouse.column >= current_editor_rect.left() && mouse.column <= current_editor_rect.right() {
                            tabs[active_tab_index].textarea.scroll((0, -1));
                        }
                    } else {
                        if mouse.row >= current_explorer_rect.top() && mouse.row <= current_explorer_rect.bottom() && mouse.column >= current_explorer_rect.left() && mouse.column <= current_explorer_rect.right() {
                            if let Some(selected) = explorer_state.selected() {
                                if selected > 0 {
                                    explorer_state.select(Some(selected - 1));
                                }
                            }
                        } else if mouse.row >= current_results_rect.top() && mouse.row <= current_results_rect.bottom() && mouse.column >= current_results_rect.left() && mouse.column <= current_results_rect.right() {
                            let active_tab = &mut tabs[active_tab_index];
                            if let Some(selected) = active_tab.results_state.selected() {
                                if selected > 0 {
                                    active_tab.results_state.select(Some(selected - 1));
                                }
                            }
                        } else if mouse.row >= current_editor_rect.top() && mouse.row <= current_editor_rect.bottom() && mouse.column >= current_editor_rect.left() && mouse.column <= current_editor_rect.right() {
                            tabs[active_tab_index].textarea.scroll((-1, 0));
                        }
                    }
                } else if mouse.kind == MouseEventKind::ScrollLeft {
                    if mouse.row >= current_explorer_rect.top() && mouse.row <= current_explorer_rect.bottom() && mouse.column >= current_explorer_rect.left() && mouse.column <= current_explorer_rect.right() {
                        if explorer_horizontal_scroll > 0 {
                            explorer_horizontal_scroll -= 1;
                        }
                    } else if mouse.row >= current_results_rect.top() && mouse.row <= current_results_rect.bottom() && mouse.column >= current_results_rect.left() && mouse.column <= current_results_rect.right() {
                        let active_tab = &mut tabs[active_tab_index];
                        if active_tab.horizontal_scroll > 0 {
                            active_tab.horizontal_scroll -= 1;
                        }
                    } else if mouse.row >= current_editor_rect.top() && mouse.row <= current_editor_rect.bottom() && mouse.column >= current_editor_rect.left() && mouse.column <= current_editor_rect.right() {
                        // For textarea, scroll((lines, columns))
                        tabs[active_tab_index].textarea.scroll((0, -1));
                    }
                } else if mouse.kind == MouseEventKind::ScrollRight {
                    if mouse.row >= current_explorer_rect.top() && mouse.row <= current_explorer_rect.bottom() && mouse.column >= current_explorer_rect.left() && mouse.column <= current_explorer_rect.right() {
                        explorer_horizontal_scroll += 1;
                    } else if mouse.row >= current_results_rect.top() && mouse.row <= current_results_rect.bottom() && mouse.column >= current_results_rect.left() && mouse.column <= current_results_rect.right() {
                        tabs[active_tab_index].horizontal_scroll += 1;
                    } else if mouse.row >= current_editor_rect.top() && mouse.row <= current_editor_rect.bottom() && mouse.column >= current_editor_rect.left() && mouse.column <= current_editor_rect.right() {
                        tabs[active_tab_index].textarea.scroll((0, 1));
                    }
                }

                // Forward mouse events to textarea if inside editor bounds
                if mouse.row >= current_editor_rect.top() && mouse.row < current_editor_rect.bottom() - 1 {
                    if mouse.column >= current_editor_rect.left() && mouse.column <= current_editor_rect.right() {
                        if focus == FocusPane::Editor {
                            tabs[active_tab_index].textarea.input(ev.clone());
                        }
                    }
                }
            }
            Event::Key(key) => {
                if show_help || show_schema_visualizer {
                    if key.code == KeyCode::Esc || key.code == KeyCode::Char('q') || key.code == KeyCode::Char('?') {
                        show_help = false;
                        show_schema_visualizer = false;
                    } else if show_schema_visualizer {
                        if key.code == KeyCode::Down || key.code == KeyCode::Char('j') {
                            schema_scroll = (schema_scroll + 1).min(max_schema_scroll);
                        } else if key.code == KeyCode::Up || key.code == KeyCode::Char('k') {
                            schema_scroll = schema_scroll.saturating_sub(1);
                        }
                    }
                    continue; // Block other inputs while help or visualizer is shown
                }

                if key.code == KeyCode::Char('?') {
                    show_help = true;
                    continue;
                }

                if key.code == KeyCode::Char('v') && key.modifiers.contains(KeyModifiers::CONTROL) {
                    show_schema_visualizer = true;
                    // Generate schema visually here
                    schema_ascii_tree = generate_schema_visualizer(&db_path).unwrap_or_else(|e| format!("Failed to generate schema visualizer: {}", e));
                    continue;
                }

                if key.code == KeyCode::F(5) || (key.code == KeyCode::Char('e') && key.modifiers.contains(KeyModifiers::CONTROL)) {
                    execute_query = true;
                }

                // Global quit
                if key.code == KeyCode::Esc {
                    break;
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

                // Save query
                if key.code == KeyCode::Char('s') && key.modifiers.contains(KeyModifiers::CONTROL) {
                    save_query = true;
                }

                // Export query
                if key.code == KeyCode::Char('o') && key.modifiers.contains(KeyModifiers::CONTROL) {
                    export_query = true;
                }

                // Format query
                if key.code == KeyCode::Char('f') && key.modifiers.contains(KeyModifiers::CONTROL) {
                    format_query = true;
                }

                // New Tab
                if key.code == KeyCode::Char('t') && key.modifiers.contains(KeyModifiers::CONTROL) {
                    new_tab = true;
                }
                
                // Close Tab
                if key.code == KeyCode::Char('w') && key.modifiers.contains(KeyModifiers::CONTROL) {
                    tabs.remove(active_tab_index);
                    if tabs.is_empty() {
                        tabs.push(EditorTab::new());
                    }
                    if active_tab_index >= tabs.len() {
                        active_tab_index = tabs.len() - 1;
                    }
                    status_msg = "Closed tab".to_string();
                }

                // Next Tab
                if key.code == KeyCode::Char('n') && key.modifiers.contains(KeyModifiers::CONTROL) {
                    active_tab_index = (active_tab_index + 1) % tabs.len();
                }

                // Prev Tab
                if key.code == KeyCode::Char('p') && key.modifiers.contains(KeyModifiers::CONTROL) {
                    if active_tab_index == 0 {
                        active_tab_index = tabs.len() - 1;
                    } else {
                        active_tab_index -= 1;
                    }
                }

                // Copy to OS Clipboard
                if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) && focus == FocusPane::Editor {
                    let active_tab = &mut tabs[active_tab_index];
                    active_tab.textarea.copy();
                    let yanked = active_tab.textarea.yank_text();
                    if let Ok(mut cb) = arboard::Clipboard::new() {
                        let _ = cb.set_text(yanked);
                        status_msg = "Copied to clipboard".to_string();
                    }
                    continue;
                }

                // Paste from OS Clipboard
                if key.code == KeyCode::Char('v') && key.modifiers.contains(KeyModifiers::CONTROL) && focus == FocusPane::Editor {
                    if let Ok(mut cb) = arboard::Clipboard::new() {
                        if let Ok(text) = cb.get_text() {
                            let active_tab = &mut tabs[active_tab_index];
                            active_tab.textarea.insert_str(&text);
                            status_msg = "Pasted from clipboard".to_string();
                        }
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
                        } else if key.code == KeyCode::Left {
                            if explorer_horizontal_scroll > 0 {
                                explorer_horizontal_scroll -= 1;
                            }
                        } else if key.code == KeyCode::Right {
                            explorer_horizontal_scroll += 1;
                        } else if key.code == KeyCode::Enter {
                            trigger_explorer_action = true;
                        }
                    }
                    FocusPane::Editor => {
                        if key.code == KeyCode::Char(' ') && key.modifiers.contains(KeyModifiers::CONTROL) {
                            let cursor = tabs[active_tab_index].textarea.cursor();
                            if cursor.0 < tabs[active_tab_index].textarea.lines().len() {
                                let line = &tabs[active_tab_index].textarea.lines()[cursor.0];
                                if cursor.1 <= line.chars().count() {
                                    // Need to convert char index to byte index for slice slicing, but line is a String.
                                    // tui-textarea's cursor.1 is a char index.
                                    let char_idx = cursor.1;
                                    let prefix: String = line.chars().take(char_idx).collect();
                                    
                                    let word_start = prefix.rfind(|c: char| !c.is_alphanumeric() && c != '_').map(|i| i + 1).unwrap_or(0);
                                    let current_word = &prefix[word_start..];

                                    if !current_word.is_empty() {
                                        let mut is_cycling = false;
                                        if !autocomplete_matches.is_empty() && current_word == autocomplete_matches[autocomplete_index] {
                                            is_cycling = true;
                                        }

                                        if is_cycling {
                                            autocomplete_index = (autocomplete_index + 1) % autocomplete_matches.len();
                                        } else {
                                            autocomplete_original_word = current_word.to_string();
                                            autocomplete_matches = dictionary.iter()
                                                .filter(|d| d.to_lowercase().starts_with(&autocomplete_original_word.to_lowercase()))
                                                .cloned().collect();
                                            autocomplete_index = 0;
                                        }

                                        if !autocomplete_matches.is_empty() {
                                            let match_str = &autocomplete_matches[autocomplete_index];
                                            for _ in 0..current_word.chars().count() {
                                                tabs[active_tab_index].textarea.delete_char();
                                            }
                                            tabs[active_tab_index].textarea.insert_str(match_str);
                                            status_msg = format!("Autocomplete: {} ({}/{})", match_str, autocomplete_index + 1, autocomplete_matches.len());
                                        } else {
                                            status_msg = format!("No autocomplete matches for '{}'", autocomplete_original_word);
                                        }
                                    }
                                }
                            }
                        } else {
                            tabs[active_tab_index].textarea.input(key);
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
                                if active_tab.query_results.len() == active_tab.pagination_limit {
                                    trigger_pagination = Some(1);
                                }
                            } else {
                                let len = active_tab.query_results.len();
                                if len > 0 {
                                    let selected = active_tab.results_state.selected().unwrap_or(0);
                                    let new_sel = (selected + 10).min(len.saturating_sub(1));
                                    active_tab.results_state.select(Some(new_sel));
                                }
                            }
                        } else if key.code == KeyCode::Char('n') || key.code == KeyCode::Char('N') {
                            if active_tab.query_results.len() == active_tab.pagination_limit {
                                trigger_pagination = Some(1);
                            }
                        } else if key.code == KeyCode::Char('p') || key.code == KeyCode::Char('P') {
                            trigger_pagination = Some(-1);
                        } else if key.code == KeyCode::PageUp {
                            if key.modifiers.contains(KeyModifiers::SHIFT) {
                                trigger_pagination = Some(-1);
                            } else {
                                if let Some(selected) = active_tab.results_state.selected() {
                                    let new_sel = selected.saturating_sub(10);
                                    active_tab.results_state.select(Some(new_sel));
                                }
                            }
                        } else if key.code == KeyCode::Left {
                            active_tab.horizontal_scroll = active_tab.horizontal_scroll.saturating_sub(1);
                        } else if key.code == KeyCode::Right {
                            if active_tab.horizontal_scroll < active_tab.column_names.len().saturating_sub(1) {
                                active_tab.horizontal_scroll += 1;
                            }
                        }
                    }
                }
            }
            _ => {}
        }

        if new_tab {
            tabs.push(EditorTab::new());
            active_tab_index = tabs.len() - 1;
            status_msg = "New tab opened".to_string();
        }

        if close_tab {
            tabs.remove(active_tab_index);
            if tabs.is_empty() {
                tabs.push(EditorTab::new());
            }
            if active_tab_index >= tabs.len() {
                active_tab_index = tabs.len() - 1;
            }
            status_msg = "Closed tab".to_string();
        }

        if format_query {
            let query = tabs[active_tab_index].textarea.lines().join("\n");
            let formatted = sqlformat::format(&query, &sqlformat::QueryParams::None, &sqlformat::FormatOptions::default());
            tabs[active_tab_index].textarea = TextArea::from(formatted.lines().map(|s| s.to_string()).collect::<Vec<_>>());
            tabs[active_tab_index].textarea.set_search_pattern("(?i)\\b(SELECT|FROM|WHERE|INSERT|UPDATE|DELETE|JOIN|ON|GROUP BY|ORDER BY|LIMIT|CREATE|TABLE|DROP|ALTER|VALUES|AND|OR|NOT|AS|IN|IS|NULL|SET|INTO|VIEW|INDEX|SHOW|PRAGMA|DESCRIBE|ATTACH|USE|MACRO)\\b").unwrap_or(());
            tabs[active_tab_index].textarea.set_search_style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD));
            tabs[active_tab_index].textarea.set_line_number_style(Style::default().fg(Color::DarkGray));
            status_msg = "Formatted SQL".to_string();
        }

        if save_query {
            let content = tabs[active_tab_index].textarea.lines().join("\n");
            if let Some(ref path) = tabs[active_tab_index].active_file {
                if let Err(e) = std::fs::write(path, content) {
                    status_msg = format!("Error saving file: {}", e);
                } else {
                    status_msg = format!("Saved {}", path.display());
                }
            } else {
                let name = format!("query_{}.sql", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs());
                let path = queries_dir.join(&name);
                if let Err(e) = std::fs::write(&path, content) {
                    status_msg = format!("Error saving new file: {}", e);
                } else {
                    tabs[active_tab_index].active_file = Some(path.clone());
                    explorer_items.push(ExplorerNode {
                        name: name.clone(),
                        display: format!("📄 {}", name),
                        level: 0,
                        is_expandable: false,
                        is_expanded: false,
                        node_type: NodeType::Query,
                    });
                    status_msg = format!("Saved as {}", name);
                }
            }
        }

        if export_query {
            let active_tab = &tabs[active_tab_index];
            if active_tab.query_results.is_empty() {
                status_msg = "No results to export!".to_string();
            } else {
                let name = format!("export_{}.csv", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs());
                let path = queries_dir.join(&name);
                let mut csv_content = String::new();
                csv_content.push_str(&active_tab.column_names.join(","));
                csv_content.push('\n');
                for row in &active_tab.query_results {
                    let escaped_row: Vec<String> = row.iter().map(|c| {
                        if c.contains(',') || c.contains('"') || c.contains('\n') {
                            format!("\"{}\"", c.replace("\"", "\"\""))
                        } else {
                            c.clone()
                        }
                    }).collect();
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
            
            let query_to_execute = if let Some(dir) = trigger_pagination {
                if let Some(bq) = &active_tab.base_query {
                    let new_offset = (active_tab.pagination_offset as i64 + (dir as i64 * active_tab.pagination_limit as i64)).max(0) as usize;
                    active_tab.pagination_offset = new_offset;
                    bq.clone()
                } else {
                    String::new()
                }
            } else {
                let mut query = if let Some(((start_row, start_col), (end_row, end_col))) = active_tab.textarea.selection_range() {
                    let lines = active_tab.textarea.lines();
                    let mut selected_text = Vec::new();
                    for row in start_row..=end_row {
                        if row >= lines.len() { continue; }
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
                
                // Reset syntax highlighting
                active_tab.textarea.set_search_pattern("(?i)\\b(SELECT|FROM|WHERE|INSERT|UPDATE|DELETE|JOIN|ON|GROUP BY|ORDER BY|LIMIT|CREATE|TABLE|DROP|ALTER|VALUES|AND|OR|NOT|AS|IN|IS|NULL|SET|INTO|VIEW|INDEX|SHOW|PRAGMA|DESCRIBE|ATTACH|USE|MACRO)\\b").unwrap_or(());
                active_tab.textarea.set_search_style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD));

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
                            clean_lower = clean_lower[idx+2..].trim_start().to_string();
                        } else {
                            clean_lower = String::new();
                        }
                    } else {
                        break;
                    }
                }
                
                let is_paginatable = clean_lower.starts_with("select") || clean_lower.starts_with("with") || clean_lower.starts_with("values") || clean_lower.starts_with("from");
                
                let final_query = if is_paginatable {
                    active_tab.is_paginated = true;
                    let clean_query = query_to_execute.trim().strip_suffix(';').unwrap_or(query_to_execute.trim());
                    format!("SELECT * FROM ({}) LIMIT {} OFFSET {}", clean_query, active_tab.pagination_limit, active_tab.pagination_offset)
                } else {
                    active_tab.is_paginated = false;
                    query_to_execute.clone()
                };

                let _ = std::fs::write("debug_query.txt", format!("query: {}\nclean: {}\npag: {}\nfinal: {}", query_to_execute, clean_lower, is_paginatable, final_query));

                match conn.prepare(&final_query) {
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
                                status_msg = format!("Query executed successfully. Returned {} rows in {:.2?}.", count, elapsed);

                                let history_path = queries_dir.join(".history.sql");
                                let timestamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
                                let history_entry = format!("-- Executed at {} (Took {:.2?})\n{};\n\n", timestamp, elapsed, query_to_execute.trim());
                                use std::io::Write;
                                if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(history_path) {
                                    let _ = f.write_all(history_entry.as_bytes());
                                }
                            }
                            Err(e) => {
                                let err_str = e.to_string();
                                if err_str.contains("syntax error at or near") {
                                    if let Some(pos) = err_str.find("\"") {
                                        if let Some(end_pos) = err_str[pos+1..].find("\"") {
                                            let broken_word = &err_str[pos+1..pos+1+end_pos];
                                            tabs[active_tab_index].textarea.set_search_pattern(&format!("(?i)\\b{}\\b", regex::escape(&broken_word))).unwrap_or(());
                                            tabs[active_tab_index].textarea.set_search_style(Style::default().fg(Color::Red).add_modifier(Modifier::BOLD));
                                        }
                                    }
                                }
                                status_msg = format!("Execution Error: {}", e);
                            }
                        }
                    }
                    Err(e) => {
                        let err_str = e.to_string();
                        if err_str.contains("syntax error at or near") {
                            if let Some(pos) = err_str.find("\"") {
                                if let Some(end_pos) = err_str[pos+1..].find("\"") {
                                    let broken_word = &err_str[pos+1..pos+1+end_pos];
                                    tabs[active_tab_index].textarea.set_search_pattern(&format!("(?i)\\b{}\\b", regex::escape(&broken_word))).unwrap_or(());
                                    tabs[active_tab_index].textarea.set_search_style(Style::default().fg(Color::Red).add_modifier(Modifier::BOLD));
                                }
                            }
                        }
                        status_msg = format!("Prepare Error: {}", e);
                    }
                }
            }
        }

        if trigger_explorer_action {
            if let Some(selected) = explorer_state.selected() {
                if selected < explorer_items.len() {
                    let item_type = explorer_items[selected].node_type.clone();
                    let item_name = explorer_items[selected].name.clone();
                    let item_expanded = explorer_items[selected].is_expanded;
                    let item_level = explorer_items[selected].level;

                    match item_type {
                        NodeType::Table | NodeType::View => {
                            if item_expanded {
                                explorer_items[selected].is_expanded = false;
                                let mut j = selected + 1;
                                while j < explorer_items.len() && explorer_items[j].level > item_level {
                                    explorer_items.remove(j);
                                }
                            } else {
                                explorer_items[selected].is_expanded = true;
                                let mut new_nodes = Vec::new();
                                let safe_name = item_name.replace("'", "''");
                                if let Ok(mut stmt) = conn.prepare(&format!("PRAGMA table_info('{}')", safe_name)) {
                                    if let Ok(mut rows) = stmt.query([]) {
                                        while let Ok(Some(row)) = rows.next() {
                                            if let Ok(col_name) = row.get::<_, String>(1) {
                                                let data_type: String = row.get(2).unwrap_or_else(|_| "UNKNOWN".to_string());
                                                new_nodes.push(ExplorerNode {
                                                    name: col_name.clone(),
                                                    display: format!("    └─ {} ({})", col_name, data_type),
                                                    level: item_level + 1,
                                                    is_expandable: false,
                                                    is_expanded: false,
                                                    node_type: NodeType::Column { data_type },
                                                });
                                            }
                                        }
                                    }
                                }
                                for (offset, node) in new_nodes.into_iter().enumerate() {
                                    explorer_items.insert(selected + 1 + offset, node);
                                }
                            }
                        }
                        NodeType::Group => {
                            if item_expanded {
                                explorer_items[selected].is_expanded = false;
                                let mut j = selected + 1;
                                while j < explorer_items.len() && explorer_items[j].level > item_level {
                                    explorer_items.remove(j);
                                }
                            } else {
                                explorer_items[selected].is_expanded = true;
                                let mut new_nodes = Vec::new();
                                if item_name == "Tables" {
                                    for t in &tables {
                                        new_nodes.push(ExplorerNode {
                                            name: t.clone(),
                                            display: format!("  ├─ {}", t),
                                            level: 1,
                                            is_expandable: true,
                                            is_expanded: false,
                                            node_type: NodeType::Table,
                                        });
                                    }
                                } else if item_name == "Views" {
                                    for v in &views {
                                        new_nodes.push(ExplorerNode {
                                            name: v.clone(),
                                            display: format!("  ├─ {}", v),
                                            level: 1,
                                            is_expandable: true,
                                            is_expanded: false,
                                            node_type: NodeType::View,
                                        });
                                    }
                                } else if item_name == "Macros" {
                                    for m in &macros {
                                        new_nodes.push(ExplorerNode {
                                            name: m.clone(),
                                            display: format!("  ├─ {}", m),
                                            level: 1,
                                            is_expandable: false,
                                            is_expanded: false,
                                            node_type: NodeType::Macro,
                                        });
                                    }
                                } else if item_name == "Queries" {
                                    for q in &queries {
                                        new_nodes.push(ExplorerNode {
                                            name: q.clone(),
                                            display: format!("  ├─ {}", q),
                                            level: 1,
                                            is_expandable: false,
                                            is_expanded: false,
                                            node_type: NodeType::Query,
                                        });
                                    }
                                }
                                for (offset, node) in new_nodes.into_iter().enumerate() {
                                    explorer_items.insert(selected + 1 + offset, node);
                                }
                            }
                        }
                        NodeType::Query => {
                            let path = queries_dir.join(&item_name);
                            
                            let mut already_open = None;
                            for (i, tab) in tabs.iter().enumerate() {
                                if tab.active_file.as_ref() == Some(&path) {
                                    already_open = Some(i);
                                    break;
                                }
                            }
                            
                            if let Some(i) = already_open {
                                active_tab_index = i;
                                focus = FocusPane::Editor;
                                status_msg = format!("Switched to {}", item_name);
                            } else if let Ok(content) = std::fs::read_to_string(&path) {
                                let current_is_empty = tabs[active_tab_index].active_file.is_none() && tabs[active_tab_index].textarea.lines().join("").trim().is_empty();
                                if !current_is_empty {
                                    tabs.push(EditorTab::new());
                                    active_tab_index = tabs.len() - 1;
                                }
                                
                                tabs[active_tab_index].textarea = TextArea::from(content.lines().map(|s| s.to_string()).collect::<Vec<_>>());
                                tabs[active_tab_index].textarea.set_search_pattern("(?i)\\b(SELECT|FROM|WHERE|INSERT|UPDATE|DELETE|JOIN|ON|GROUP BY|ORDER BY|LIMIT|CREATE|TABLE|DROP|ALTER|VALUES|AND|OR|NOT|AS|IN|IS|NULL|SET|INTO|VIEW|INDEX|SHOW|PRAGMA|DESCRIBE|ATTACH|USE|MACRO)\\b").unwrap_or(());
                                tabs[active_tab_index].textarea.set_search_style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD));
                                tabs[active_tab_index].textarea.set_line_number_style(Style::default().fg(Color::DarkGray));
                                tabs[active_tab_index].active_file = Some(path);
                                focus = FocusPane::Editor;
                                status_msg = format!("Loaded {}", item_name);
                            }
                        }
                        NodeType::Column { .. } => {}
                        NodeType::Macro => {}
                    }
                }
            }
        }
    }

    Ok(())
}

/// Translates native DuckDB data types into readable strings for the IDE Results pane.
/// Provides precise formatting for Dates, Timestamps, Numerics, and Arrays/Lists.
fn format_duckdb_value(v: duckdb::types::Value) -> String {
    use duckdb::types::{Value, TimeUnit};
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
                s[8..s.len()-1].to_string()
            } else {
                s
            }
        },
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
            if let Some(dt) = chrono::NaiveDateTime::from_timestamp_opt(timestamp_secs, timestamp_nanos as u32) {
                dt.format("%Y-%m-%d %H:%M:%S%.6f").to_string()
            } else {
                format!("{:?}", Value::Timestamp(unit, val))
            }
        }
        Value::Date32(days) => {
            if let Some(dt) = chrono::NaiveDateTime::from_timestamp_opt((days as i64) * 86400, 0) {
                dt.date().format("%Y-%m-%d").to_string()
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
                    return s[stripped.len()+1..s.len()-1].to_string();
                }
            }
            s
        }
    }
}

/// Generates an ASCII tree of the schema and copies a Mermaid diagram to the clipboard.
fn generate_schema_visualizer(db_str: &str) -> Result<String, Box<dyn std::error::Error>> {
    let conn = duckdb::Connection::open(db_str)?;
    
    // 1. Get Tables & Columns
    let mut stmt = conn.prepare("SELECT table_name, column_name, data_type FROM duckdb_columns ORDER BY table_name, column_index;")?;
    let mut rows = stmt.query([])?;
    
    let mut tables: std::collections::HashMap<String, Vec<(String, String)>> = std::collections::HashMap::new();
    while let Some(row) = rows.next()? {
        let table_name: String = row.get(0)?;
        let col_name: String = row.get(1)?;
        let data_type: String = row.get(2)?;
        tables.entry(table_name).or_insert_with(Vec::new).push((col_name, data_type));
    }
    
    // 2. Get Foreign Keys
    let mut stmt_fk = conn.prepare("SELECT table_name, constraint_column_names[1], referenced_table, referenced_column_names[1] FROM duckdb_constraints WHERE constraint_type = 'FOREIGN KEY';")?;
    let mut rows_fk = stmt_fk.query([])?;
    
    let mut fks: Vec<(String, String, String, String)> = Vec::new();
    while let Some(row) = rows_fk.next()? {
        let table_name: String = row.get(0)?;
        let col_name: String = row.get(1)?;
        let ref_table: String = row.get(2)?;
        let ref_col: String = row.get(3)?;
        fks.push((table_name, col_name, ref_table, ref_col));
    }
    
    // 3. Generate Mermaid
    let mut mermaid = String::from("erDiagram\n");
    for (table, cols) in &tables {
        mermaid.push_str(&format!("    {} {{\n", table));
        for (col, dtype) in cols {
            mermaid.push_str(&format!("        {} {}\n", dtype.replace(" ", "_"), col));
        }
        mermaid.push_str("    }\n");
    }
    for (table, _col, ref_table, _ref_col) in &fks {
        mermaid.push_str(&format!("    {} }}|--|| {} : \"\"\n", table, ref_table));
    }
    
    // Use arboard to copy
    if let Ok(mut clipboard) = arboard::Clipboard::new() {
        let _ = clipboard.set_text(&mermaid);
    }
    
    // 4. Generate ASCII Tree
    let mut tree = String::from("Database Schema (Mermaid.js diagram copied to clipboard!)\n");
    let mut sorted_tables: Vec<&String> = tables.keys().collect();
    sorted_tables.sort();
    
    for (i, table) in sorted_tables.iter().enumerate() {
        let is_last_table = i == sorted_tables.len() - 1;
        let prefix = if is_last_table { "└──" } else { "├──" };
        tree.push_str(&format!("{} {}\n", prefix, table));
        
        let cols = tables.get(*table).unwrap();
        for (j, (col, dtype)) in cols.iter().enumerate() {
            let is_last_col = j == cols.len() - 1;
            let col_prefix = if is_last_table { "    " } else { "│   " };
            let line_prefix = if is_last_col { "└──" } else { "├──" };
            
            // Check if col is a foreign key
            let fk = fks.iter().find(|(t, c, _, _)| t == *table && c == col);
            if let Some((_, _, ref_table, ref_col)) = fk {
                tree.push_str(&format!("{}{} {} ({}) -> {}.{}\n", col_prefix, line_prefix, col, dtype, ref_table, ref_col));
            } else {
                tree.push_str(&format!("{}{} {} ({})\n", col_prefix, line_prefix, col, dtype));
            }
        }
    }
    
    Ok(tree)
}

#[cfg(test)]
mod tests {
    use super::*;
    use duckdb::types::Value;

    #[test]
    fn test_format_duckdb_value() {
        assert_eq!(format_duckdb_value(Value::Null), "NULL");
        assert_eq!(format_duckdb_value(Value::Boolean(true)), "true");
        assert_eq!(format_duckdb_value(Value::Int(42)), "42");
        assert_eq!(format_duckdb_value(Value::Text("hello".to_string())), "hello");
    }
}
