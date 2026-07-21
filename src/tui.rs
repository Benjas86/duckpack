use crate::diff::{DiffItemType, DiffResult, DiffItem};
use anyhow::Result;
use ratatui::{
    backend::CrosstermBackend,
    crossterm::{
        event::{self, Event, KeyCode},
    },
    layout::{Constraint, Direction, Layout, Alignment},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap},
    Terminal,
};
use similar::{ChangeTag, TextDiff};

/// Defines the various actions that can be triggered by the user within the TUI.
pub enum TuiAction {
    Apply,
    Refresh,
    Quit,
    ToggleDestructiveMode,
    Inspect { obj_name: String },
    CloseInspect,
    Pull { obj_name: String, item_type: DiffItemType },
    ToggleShowIgnored,
    ToggleTheme,
    Explore,
    CopyStatus,
}

/// Recalculates the locked state of all diff items based on their dependencies.
/// If a parent item (like a CREATE TABLE) is deselected, dependent items (like ALTER TABLE or INSERT)
/// are automatically locked and deselected to prevent invalid SQL execution.
fn recompute_locks(items: &mut Vec<DiffItem>) {
    for item in items.iter_mut() {
        item.locked = false;
    }
    let mut changed = true;
    while changed {
        changed = false;
        for i in 0..items.len() {
            if !items[i].selected {
                let obj_name = items[i].obj_name.clone();
                for j in 0..items.len() {
                    if i != j && (!items[j].locked || items[j].selected) && items[j].sql.contains(&obj_name) {
                        items[j].selected = false;
                        items[j].locked = true;
                        changed = true;
                    }
                }
            }
        }
    }
}

/// A rudimentary SQL syntax highlighter that tokenizes a SQL string and applies Ratatui `Span` styles.
/// Keywords are highlighted in Cyan and strings in Yellow.
fn highlight_sql(sql: &str) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let keywords = ["SELECT", "FROM", "WHERE", "CREATE", "TABLE", "VIEW", "OR", "REPLACE", "MACRO", "AS", "DEFAULT", "TIMESTAMP", "INTEGER", "DATE", "DECIMAL", "VARCHAR", "ALTER", "ADD", "COLUMN", "DROP", "TYPE", "INSERT", "INTO", "VALUES", "UPDATE", "SET", "DELETE", "AND", "NOT", "NULL", "PRIMARY", "KEY", "UUID", "BOOLEAN"];
    
    let mut current_word = String::new();
    let mut is_string = false;
    
    for c in sql.chars() {
        if c == '\'' {
            if !current_word.is_empty() {
                let upper = current_word.to_uppercase();
                if keywords.contains(&upper.as_str()) {
                    spans.push(Span::styled(current_word.clone(), Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)));
                } else {
                    spans.push(Span::raw(current_word.clone()));
                }
                current_word.clear();
            }
            is_string = !is_string;
            current_word.push(c);
            if !is_string {
                spans.push(Span::styled(current_word.clone(), Style::default().fg(Color::Yellow)));
                current_word.clear();
            }
        } else if c.is_whitespace() || c == '(' || c == ')' || c == ',' || c == ';' || c == '=' {
            if !current_word.is_empty() {
                if is_string {
                    current_word.push(c);
                    continue;
                }
                let upper = current_word.to_uppercase();
                if keywords.contains(&upper.as_str()) {
                    spans.push(Span::styled(current_word.clone(), Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)));
                } else {
                    spans.push(Span::raw(current_word.clone()));
                }
                current_word.clear();
            }
            if !is_string {
                spans.push(Span::raw(c.to_string()));
            } else {
                current_word.push(c);
            }
        } else {
            current_word.push(c);
        }
    }
    
    if !current_word.is_empty() {
        let upper = current_word.to_uppercase();
        if !is_string && keywords.contains(&upper.as_str()) {
            spans.push(Span::styled(current_word, Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)));
        } else {
            spans.push(Span::styled(current_word, if is_string { Style::default().fg(Color::Yellow) } else { Style::default() }));
        }
    }
    spans
}

/// Wraps the `sqlformat` crate to standardize formatting for SQL diffing.
/// We use this to ensure that whitespace or casing differences in `.sql` files 
/// don't trigger false positive diffs.
fn format_sql_string(sql: &str) -> String {
    let mut options = sqlformat::FormatOptions::default();
    options.indent = sqlformat::Indent::Spaces(4);
    options.uppercase = Some(true);
    options.lines_between_queries = 1;
    
    sqlformat::format(sql, &sqlformat::QueryParams::None, &options)
}

/// Generates a side-by-side terminal diff view between two SQL definitions.
/// It highlights added lines in Green and deleted lines in Red,
/// while leaving unchanged lines with standard syntax highlighting.
fn get_side_by_side_diff(old_def: &str, new_def: &str) -> (Vec<Line<'static>>, Vec<Line<'static>>) {
    let old_formatted = format_sql_string(old_def);
    let new_formatted = format_sql_string(new_def);
    
    let diff = TextDiff::from_lines(&old_formatted, &new_formatted);
    let mut left_lines = Vec::new();
    let mut right_lines = Vec::new();
    
    for change in diff.iter_all_changes() {
        let val = change.value();
        let val_trim = if val.ends_with('\n') { &val[..val.len()-1] } else { val };

        match change.tag() {
            ChangeTag::Equal => {
                let mut spans = vec![Span::styled("  ", Style::default().fg(Color::DarkGray))];
                spans.extend(highlight_sql(val_trim));
                left_lines.push(Line::from(spans.clone()));
                right_lines.push(Line::from(spans));
            }
            ChangeTag::Delete => {
                let mut spans = vec![Span::styled("- ", Style::default().fg(Color::Red).add_modifier(Modifier::BOLD))];
                spans.push(Span::styled(val_trim.to_string(), Style::default().fg(Color::Red)));
                left_lines.push(Line::from(spans));
                right_lines.push(Line::from(vec![Span::raw("")]));
            }
            ChangeTag::Insert => {
                left_lines.push(Line::from(vec![Span::raw("")]));
                let mut spans = vec![Span::styled("+ ", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD))];
                spans.push(Span::styled(val_trim.to_string(), Style::default().fg(Color::Green)));
                right_lines.push(Line::from(spans));
            }
        }
    }
    
    (left_lines, right_lines)
}

/// Sorts the execution order of `DiffItem`s to satisfy SQL dependencies.
/// `CREATE` statements execute first, followed by `ALTER`, and finally `DROP` statements.
fn sort_diff_items(items: &mut Vec<DiffItem>) {
    items.sort_by_key(|item| {
        match item.item_type {
            DiffItemType::CreateSchema | DiffItemType::CreateTable | DiffItemType::CreateView | DiffItemType::CreateMacro => 1,
            DiffItemType::AlterTable | DiffItemType::RenameTable | DiffItemType::RenameColumn => 2,
            DiffItemType::DropSchema | DiffItemType::DropTable | DiffItemType::DropView | DiffItemType::DropMacro | DiffItemType::DropColumn => 3,
        }
    });
}

/// The main execution loop for the `apply` command's Terminal UI.
/// Handles rendering the list of changes, capturing keyboard events, and toggling selection states.
pub fn draw_and_handle_events(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    diff: &mut DiffResult,
    force_drop: bool,
    status_msg: &str,
    draw_only: bool,
    inspect_data: &Option<(String, Vec<String>, Vec<Vec<String>>, i64)>,
) -> Result<TuiAction> {
    sort_diff_items(&mut diff.items);

    let mut list_state = ListState::default();
    let mut scroll_position: u16 = 0;
    let mut show_help = false;
    let mut show_ignored = false;
    let mut inspect_scroll: u16 = 0;
    let mut max_inspect_scroll: u16 = 0;
    if !diff.items.is_empty() {
        list_state.select(Some(0));
    }

    loop {
        let visible_items: Vec<(usize, &DiffItem)> = diff.items.iter().enumerate()
            .filter(|(_, item)| show_ignored || !item.ignored)
            .collect();
        terminal.draw(|f| {
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .margin(1)
                .constraints([
                    Constraint::Length(6), // Status & Title
                    Constraint::Percentage(50), // List
                    Constraint::Percentage(50), // Details
                    Constraint::Length(3), // Footer
                ].as_ref())
                .split(f.area());

            let mut title_text = vec![Span::styled(format!("DuckPack v{}", env!("CARGO_PKG_VERSION")), Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD))];
            if force_drop {
                title_text.push(Span::styled(" [WARNING: DESTRUCTIVE MODE ACTIVE]", Style::default().fg(Color::Red).add_modifier(Modifier::BOLD).add_modifier(Modifier::RAPID_BLINK)));
            }
            if !status_msg.is_empty() {
                title_text.push(Span::styled(format!(" - {}", status_msg), Style::default().fg(Color::Green)));
            }
            
            let title = Paragraph::new(Line::from(title_text))
                .block(Block::default().borders(Borders::ALL).title("Status"))
                .wrap(ratatui::widgets::Wrap { trim: false });
            f.render_widget(title, chunks[0]);

            if diff.is_empty() {
                let p = Paragraph::new(Span::styled("No schema changes detected. Database is up to date.", Style::default().fg(Color::Green)))
                    .block(Block::default().borders(Borders::ALL));
                f.render_widget(p, chunks[1]);
            } else {
                let items: Vec<ListItem> = visible_items.iter().map(|(_, item)| {
                    let checkbox = if item.locked {
                        "[-] "
                    } else if item.selected {
                        "[x] "
                    } else {
                        "[ ] "
                    };

                    let (mut prefix, mut style) = match item.item_type {
                        DiffItemType::CreateSchema => ("[NEW SCHEMA] ", Style::default().fg(Color::Cyan)),
                        DiffItemType::DropSchema => if force_drop { ("[DROP SCHEMA] ", Style::default().fg(Color::Red)) } else { ("[IGNORED SCHEMA] ", Style::default().fg(Color::DarkGray)) },
                        DiffItemType::CreateTable => ("[NEW TABLE] ", Style::default().fg(Color::Green)),
                        DiffItemType::AlterTable => ("[ALTER TABLE] ", Style::default().fg(Color::Yellow)),
                        DiffItemType::RenameTable => ("[RENAME TABLE] ", Style::default().fg(Color::LightBlue)),
                        DiffItemType::RenameColumn => ("[RENAME COL] ", Style::default().fg(Color::LightBlue)),
                        DiffItemType::DropTable => if force_drop { ("[DROP TABLE] ", Style::default().fg(Color::Red)) } else { ("[IGNORED DROP] ", Style::default().fg(Color::DarkGray)) },
                        DiffItemType::DropColumn => if force_drop { ("[DROP COL] ", Style::default().fg(Color::Red)) } else { ("[IGNORED DROP] ", Style::default().fg(Color::DarkGray)) },
                        DiffItemType::CreateView => ("[VIEW] ", Style::default().fg(Color::Blue)),
                        DiffItemType::DropView => {
                            if force_drop { ("[DROP VIEW] ", Style::default().fg(Color::Red)) } 
                            else { ("[IGNORED DROP] ", Style::default().fg(Color::DarkGray)) }
                        },
                        DiffItemType::CreateMacro => ("[MACRO] ", Style::default().fg(Color::Magenta)),
                        DiffItemType::DropMacro => {
                            if force_drop { ("[DROP MACRO] ", Style::default().fg(Color::Red)) } 
                            else { ("[IGNORED DROP] ", Style::default().fg(Color::DarkGray)) }
                        },
                    };

                    if item.locked {
                        prefix = "[LOCKED] ";
                        style = Style::default().fg(Color::Red).add_modifier(Modifier::CROSSED_OUT);
                    } else if !item.selected {
                        style = Style::default().fg(Color::DarkGray);
                    }

                    let line = Line::from(vec![
                        Span::raw(checkbox),
                        Span::styled(prefix, style.add_modifier(Modifier::BOLD)),
                        Span::raw(&item.obj_name),
                    ]);

                    ListItem::new(line)
                }).collect();

                let list = List::new(items)
                    .block(Block::default().borders(Borders::ALL).title("Proposed Changes (Space to toggle, Up/Down to navigate)"))
                    .highlight_style(Style::default().bg(Color::DarkGray).add_modifier(Modifier::BOLD))
                    .highlight_symbol(">> ");

                f.render_stateful_widget(list, chunks[1], &mut list_state);

                // Render Details Pane (Side-by-Side)
                if let Some(idx) = list_state.selected() {
                    let selected_item = if idx < visible_items.len() { visible_items[idx].1 } else { return; };
                    
                    let old_def = selected_item.old_def.as_deref().unwrap_or("");
                    let new_def = selected_item.new_def.as_deref().unwrap_or("");
                    
                    let (left_lines, right_lines) = get_side_by_side_diff(old_def, new_def);
                    
                    let details_chunks = Layout::default()
                        .direction(Direction::Horizontal)
                        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)].as_ref())
                        .split(chunks[2]);
                        
                    let max_scroll = left_lines.len() as u16;
                    scroll_position = scroll_position.min(max_scroll);
                        
                    let left_pane = Paragraph::new(left_lines)
                        .block(Block::default().borders(Borders::ALL).title("Deployed (Online)").border_style(Style::default().fg(Color::Cyan)))
                        .wrap(Wrap { trim: false })
                        .scroll((scroll_position, 0));
                        
                    let right_pane = Paragraph::new(right_lines)
                        .block(Block::default().borders(Borders::ALL).title("Workspace (Offline)").border_style(Style::default().fg(Color::Yellow)))
                        .wrap(Wrap { trim: false })
                        .scroll((scroll_position, 0));

                    f.render_widget(left_pane, details_chunks[0]);
                    f.render_widget(right_pane, details_chunks[1]);
                } else {
                    let p = Paragraph::new("Select an item to view details").block(Block::default().borders(Borders::ALL).title("Details"));
                    f.render_widget(p, chunks[2]);
                }
            }

            let footer_text = if diff.is_empty() {
                "Press 'Esc' or 'q' to exit"
            } else if inspect_data.is_some() {
                "Press 'Esc' or 'q' to close inspect view"
            } else if show_help {
                "Press 'Esc' or 'q' to close help"
            } else {
                "Press 'Enter' to Apply, 'Space' to toggle, '?' or 'h' for Help, 'Esc' or 'q' to Quit"
            };

            let footer = Paragraph::new(Span::styled(footer_text, Style::default().fg(Color::Cyan)))
                .block(Block::default()
                    .borders(Borders::ALL)
                    .title_bottom(Line::from(vec![
                        Span::styled(" e ", Style::default().fg(Color::Black).bg(Color::Yellow).add_modifier(Modifier::BOLD)),
                        Span::styled(" Explorer / IDE ", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
                    ]).alignment(Alignment::Right))
                );
            f.render_widget(footer, chunks[3]);

            if let Some((obj_name, headers, rows, total_rows)) = inspect_data {
                let popup_area = ratatui::layout::Rect {
                    x: f.area().width / 8,
                    y: f.area().height / 8,
                    width: (f.area().width * 3) / 4,
                    height: (f.area().height * 3) / 4,
                };
                f.render_widget(ratatui::widgets::Clear, popup_area);

                let mut lines = Vec::new();
                for (row_idx, row) in rows.iter().enumerate() {
                    lines.push(ratatui::text::Line::from(vec![
                        ratatui::text::Span::styled(format!("--- Row {} ---", row_idx + 1), Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD))
                    ]));
                    for (i, cell) in row.iter().enumerate() {
                        let header = &headers[i];
                        let padding = " ".repeat(20_usize.saturating_sub(header.len()));
                        lines.push(ratatui::text::Line::from(vec![
                            ratatui::text::Span::styled(format!("{}{}", header, padding), Style::default().fg(Color::Cyan)),
                            ratatui::text::Span::raw(format!(" | {}", cell))
                        ]));
                    }
                    lines.push(ratatui::text::Line::from(""));
                }

                max_inspect_scroll = lines.len().saturating_sub((popup_area.height as usize).saturating_sub(2)) as u16;
                inspect_scroll = inspect_scroll.min(max_inspect_scroll);

                let paragraph = Paragraph::new(lines)
                    .block(Block::default()
                        .title(format!(" Inspecting: {} (Total Rows: {}) - Scroll: {}/{} ", obj_name, total_rows, inspect_scroll, max_inspect_scroll))
                        .borders(Borders::ALL)
                        .border_style(Style::default().fg(Color::Cyan)))
                    .scroll((inspect_scroll, 0));
                f.render_widget(paragraph, popup_area);
            } else if show_help {
                let popup_area = ratatui::layout::Rect {
                    x: f.area().width / 4,
                    y: f.area().height / 4,
                    width: f.area().width / 2,
                    height: f.area().height / 2,
                };
                f.render_widget(ratatui::widgets::Clear, popup_area);

                let help_lines = vec![
                    ratatui::text::Line::from(vec![ratatui::text::Span::styled(" [?]       ", Style::default().fg(Color::Cyan)), ratatui::text::Span::raw("Toggle Help Menu")]),
                    ratatui::text::Line::from(vec![ratatui::text::Span::styled(" [p]       ", Style::default().fg(Color::Cyan)), ratatui::text::Span::raw("Pull dropped item down to local files")]),
                    ratatui::text::Line::from(vec![ratatui::text::Span::styled(" [v]       ", Style::default().fg(Color::Cyan)), ratatui::text::Span::raw("Toggle visibility of .duckdbignore items")]),
                    ratatui::text::Line::from(vec![ratatui::text::Span::styled(" [d]       ", Style::default().fg(Color::Cyan)), ratatui::text::Span::raw("Toggle destructive mode (force drop)")]),
                    ratatui::text::Line::from(vec![ratatui::text::Span::styled(" [i]       ", Style::default().fg(Color::Cyan)), ratatui::text::Span::raw("Inspect a table's data in target")]),
                    ratatui::text::Line::from(vec![ratatui::text::Span::styled(" [c]       ", Style::default().fg(Color::Cyan)), ratatui::text::Span::raw("Copy current status text to duckpack.log")]),
                    ratatui::text::Line::from(vec![ratatui::text::Span::styled(" [Enter]   ", Style::default().fg(Color::Cyan)), ratatui::text::Span::raw("Apply selected changes")]),
                    ratatui::text::Line::from(vec![ratatui::text::Span::styled(" [Space]   ", Style::default().fg(Color::Cyan)), ratatui::text::Span::raw("Toggle selection for an item")]),
                    ratatui::text::Line::from(vec![ratatui::text::Span::styled(" [PgUp/Dn] ", Style::default().fg(Color::Cyan)), ratatui::text::Span::raw("Scroll SQL diff panes")]),
                    ratatui::text::Line::from(vec![ratatui::text::Span::styled(" [Esc/q]   ", Style::default().fg(Color::Cyan)), ratatui::text::Span::raw("Exit / Close panels")]),
                ];

                let help_paragraph = Paragraph::new(help_lines)
                    .block(Block::default()
                        .title(" Help Menu ")
                        .borders(Borders::ALL)
                        .border_style(Style::default().fg(Color::Yellow)));
                f.render_widget(help_paragraph, popup_area);
            }
        })?;

        if draw_only {
            return Ok(TuiAction::Refresh);
        }

        match event::read()? {
            Event::Key(key) => {
                if inspect_data.is_some() {
                    match key.code {
                        KeyCode::Char('q') | KeyCode::Esc => {
                            return Ok(TuiAction::CloseInspect);
                        }
                        KeyCode::Up => {
                            inspect_scroll = inspect_scroll.saturating_sub(1);
                        }
                        KeyCode::Down => {
                            inspect_scroll = inspect_scroll.saturating_add(1).min(max_inspect_scroll);
                        }
                        KeyCode::PageUp => {
                            inspect_scroll = inspect_scroll.saturating_sub(10);
                        }
                        KeyCode::PageDown => {
                            inspect_scroll = inspect_scroll.saturating_add(10).min(max_inspect_scroll);
                        }
                        _ => {}
                    }
                    continue;
                }

                match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => {
                        if show_help {
                            show_help = false;
                        } else {
                            return Ok(TuiAction::Quit);
                        }
                    }
                    KeyCode::Char('?') | KeyCode::Char('h') => {
                        show_help = !show_help;
                    }
                    KeyCode::Char('r') => {
                        if key.modifiers.contains(event::KeyModifiers::CONTROL) || !key.modifiers.contains(event::KeyModifiers::CONTROL) {
                            return Ok(TuiAction::Refresh);
                        }
                    }
                    KeyCode::Char('e') => {
                        return Ok(TuiAction::Explore);
                    }
                    KeyCode::Char('v') => {
                        show_ignored = !show_ignored;
                    }
                    KeyCode::Char('c') => {
                        return Ok(TuiAction::CopyStatus);
                    }
                    KeyCode::Char('p') => {
                        if let Some(idx) = list_state.selected() {
                            if idx < visible_items.len() {
                                let item = visible_items[idx].1;
                                return Ok(TuiAction::Pull { obj_name: item.obj_name.clone(), item_type: item.item_type.clone() });
                            }
                        }
                    }
                    KeyCode::Char('d') => {
                        return Ok(TuiAction::ToggleDestructiveMode);
                    }
                    KeyCode::Char('i') => {
                        if let Some(selected) = list_state.selected() {
                            if selected < visible_items.len() {
                                return Ok(TuiAction::Inspect { obj_name: visible_items[selected].1.obj_name.clone() });
                            }
                        }
                    }
                    KeyCode::Down => {
                        if let Some(selected) = list_state.selected() {
                            if selected < visible_items.len().saturating_sub(1) {
                                list_state.select(Some(selected + 1));
                                scroll_position = 0;
                            }
                        }
                    }
                    KeyCode::Up => {
                        if let Some(selected) = list_state.selected() {
                            if selected > 0 {
                                list_state.select(Some(selected - 1));
                                scroll_position = 0;
                            }
                        }
                    }
                    KeyCode::PageDown => {
                        scroll_position = scroll_position.saturating_add(5);
                    }
                    KeyCode::PageUp => {
                        scroll_position = scroll_position.saturating_sub(5);
                    }
                    KeyCode::Char(' ') => {
                        if let Some(idx) = list_state.selected() {
                            if idx < visible_items.len() {
                                let real_idx = visible_items[idx].0;
                                if !diff.items[real_idx].locked && !diff.items[real_idx].ignored {
                                    diff.items[real_idx].selected = !diff.items[real_idx].selected;
                                    recompute_locks(&mut diff.items);
                                }
                            }
                        }
                    }
                    KeyCode::Enter => {
                        if !diff.is_empty() {
                            return Ok(TuiAction::Apply);
                        }
                    }
                    _ => {}
                }
            }
            Event::Mouse(mouse_event) => {
                if inspect_data.is_some() {
                    match mouse_event.kind {
                        event::MouseEventKind::ScrollDown => {
                            inspect_scroll = inspect_scroll.saturating_add(3).min(max_inspect_scroll);
                        }
                        event::MouseEventKind::ScrollUp => {
                            inspect_scroll = inspect_scroll.saturating_sub(3);
                        }
                        _ => {}
                    }
                    continue;
                }

                match mouse_event.kind {
                    event::MouseEventKind::ScrollDown => {
                        scroll_position = scroll_position.saturating_add(3);
                    }
                    event::MouseEventKind::ScrollUp => {
                        scroll_position = scroll_position.saturating_sub(3);
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
}
