use crate::diff::{DiffItemType, DiffResult, DiffItem};
use anyhow::Result;
use ratatui::{
    backend::CrosstermBackend,
    crossterm::{
        event::{self, Event, KeyCode},
        execute,
        terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
    },
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph},
    Terminal,
};
use std::io::stdout;

fn recompute_locks(items: &mut Vec<DiffItem>) {
    // 1. Reset all locks
    for item in items.iter_mut() {
        item.locked = false;
    }

    // 2. Cascade locks recursively
    let mut changed = true;
    while changed {
        changed = false;
        
        for i in 0..items.len() {
            if !items[i].selected {
                let obj_name = items[i].obj_name.clone();
                for j in 0..items.len() {
                    // Prevent circular dependencies from looping infinitely, though they shouldn't exist in SQL
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

pub fn run_tui(diff: &mut DiffResult, force_drop: bool) -> Result<bool> {
    enable_raw_mode()?;
    let mut stdout = stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut apply = false;
    let mut list_state = ListState::default();
    if !diff.items.is_empty() {
        list_state.select(Some(0));
    }

    loop {
        terminal.draw(|f| {
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .margin(1)
                .constraints([
                    Constraint::Length(3), // Title
                    Constraint::Min(5),    // List
                    Constraint::Length(3), // Footer
                ].as_ref())
                .split(f.area());

            let title = Paragraph::new(Span::styled(
                "DuckDB Declarative Migration - Select Changes",
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
            ))
            .block(Block::default().borders(Borders::ALL).title("Status"));
            f.render_widget(title, chunks[0]);

            if diff.is_empty() {
                let p = Paragraph::new(Span::styled("No schema changes detected. Database is up to date.", Style::default().fg(Color::Green)))
                    .block(Block::default().borders(Borders::ALL));
                f.render_widget(p, chunks[1]);
            } else {
                let items: Vec<ListItem> = diff.items.iter().map(|item| {
                    let checkbox = if item.locked {
                        "[-] "
                    } else if item.selected {
                        "[x] "
                    } else {
                        "[ ] "
                    };
                    
                    let (mut prefix, mut style) = match item.item_type {
                        DiffItemType::CreateTable => ("[NEW TABLE] ", Style::default().fg(Color::Green)),
                        DiffItemType::AlterTable => ("[ALTER TABLE] ", Style::default().fg(Color::Yellow)),
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
                        Span::styled(&item.sql, if item.locked || !item.selected { Style::default().fg(Color::DarkGray) } else { Style::default() }),
                    ]);

                    ListItem::new(line)
                }).collect();

                let list = List::new(items)
                    .block(Block::default().borders(Borders::ALL).title("Proposed Changes (Space to toggle, Up/Down to navigate)"))
                    .highlight_style(Style::default().bg(Color::DarkGray).add_modifier(Modifier::BOLD))
                    .highlight_symbol(">> ");

                f.render_stateful_widget(list, chunks[1], &mut list_state);
            }

            let footer_text = if diff.is_empty() {
                "Press 'Esc' or 'q' to exit"
            } else {
                "Press 'Enter' to Apply SELECTED changes, 'Esc' or 'q' to Cancel"
            };

            let footer = Paragraph::new(Span::styled(footer_text, Style::default().fg(Color::Cyan)))
                .block(Block::default().borders(Borders::ALL));
            f.render_widget(footer, chunks[2]);
        })?;

        if let Event::Key(key) = event::read()? {
            match key.code {
                KeyCode::Char('q') | KeyCode::Esc => {
                    break;
                }
                KeyCode::Down => {
                    if let Some(selected) = list_state.selected() {
                        if selected < diff.items.len().saturating_sub(1) {
                            list_state.select(Some(selected + 1));
                        }
                    }
                }
                KeyCode::Up => {
                    if let Some(selected) = list_state.selected() {
                        if selected > 0 {
                            list_state.select(Some(selected - 1));
                        }
                    }
                }
                KeyCode::Char(' ') => {
                    if let Some(idx) = list_state.selected() {
                        if !diff.items[idx].locked {
                            diff.items[idx].selected = !diff.items[idx].selected;
                            recompute_locks(&mut diff.items);
                        }
                    }
                }
                KeyCode::Enter => {
                    if !diff.is_empty() {
                        apply = true;
                        break;
                    }
                }
                _ => {}
            }
        }
    }

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    Ok(apply)
}
