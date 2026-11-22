use ratatui::{
    crossterm::event::{KeyCode, KeyEvent},
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph},
    Frame,
};

/// Computes a centered modal rectangle clamped within `area` (with horizontal/vertical padding).
pub(crate) fn centered_modal_rect(area: Rect, max_width: u16, max_height: u16) -> Rect {
    let popup_width = max_width.min(area.width.saturating_sub(4));
    let popup_height = max_height.min(area.height.saturating_sub(2));
    Rect {
        x: (area.width.saturating_sub(popup_width)) / 2,
        y: (area.height.saturating_sub(popup_height)) / 2,
        width: popup_width,
        height: popup_height,
    }
}

/// Sanitizes and pastes single-line clipboard text into `buf` at `*cursor`.
pub(crate) fn paste_single_line(buf: &mut String, cursor: &mut usize, raw: &str) {
    let cleaned: String = raw
        .trim_matches(|c| c == '\r' || c == '\n')
        .chars()
        .filter(|&c| c != '\r' && c != '\n' && !c.is_control())
        .collect();
    for ch in cleaned.chars() {
        insert_char_at_cursor(buf, cursor, ch);
    }
}

/// Handles standard single-line cursor movement and deletion keys (`Left`, `Right`, `Home`, `End`,
/// `Backspace`, `Delete`) for modal text inputs. Returns `true` if the key was consumed.
pub(crate) fn handle_text_nav_or_delete(buf: &mut String, cursor: &mut usize, key: KeyEvent) -> bool {
    match key.code {
        KeyCode::Left => {
            *cursor = cursor.saturating_sub(1);
            true
        }
        KeyCode::Right => {
            let len = buf.chars().count();
            *cursor = (*cursor + 1).min(len);
            true
        }
        KeyCode::Home => {
            *cursor = 0;
            true
        }
        KeyCode::End => {
            *cursor = buf.chars().count();
            true
        }
        KeyCode::Backspace => {
            delete_char_before_cursor(buf, cursor);
            true
        }
        KeyCode::Delete => {
            delete_char_at_cursor(buf, *cursor);
            true
        }
        _ => false,
    }
}

/// Renders a standard bordered modal text input box with active highlight & cursor indicator.
#[allow(clippy::too_many_arguments)]
pub(crate) fn render_text_input_box(
    f: &mut Frame,
    area: Rect,
    title: &str,
    text: &str,
    cursor_pos: usize,
    is_active: bool,
    is_masked: bool,
    active_color: Color,
    rounded: bool,
) {
    let border_style = if is_active {
        Style::default()
            .fg(active_color)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let mut block = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .border_style(border_style);
    if rounded {
        block = block.border_type(BorderType::Rounded);
    }
    let widget = Paragraph::new(render_text_with_cursor(
        text,
        cursor_pos,
        is_active,
        is_masked,
    ))
    .block(block);
    f.render_widget(widget, area);
}

/// Inserts a character into a string at the unicode character position `*cursor`,
/// advancing the cursor by 1.
pub(crate) fn insert_char_at_cursor(s: &mut String, cursor: &mut usize, c: char) {
    let char_count = s.chars().count();
    let pos = (*cursor).min(char_count);
    let byte_idx = s
        .char_indices()
        .nth(pos)
        .map(|(idx, _)| idx)
        .unwrap_or(s.len());
    s.insert(byte_idx, c);
    *cursor = pos + 1;
}

/// Deletes the character before `*cursor` (Backspace), moving the cursor back by 1.
pub(crate) fn delete_char_before_cursor(s: &mut String, cursor: &mut usize) {
    if *cursor == 0 || s.is_empty() {
        return;
    }
    let char_count = s.chars().count();
    let pos = (*cursor).min(char_count);
    if let Some((byte_idx, _)) = s.char_indices().nth(pos - 1) {
        s.remove(byte_idx);
        *cursor = pos - 1;
    }
}

/// Deletes the character at `cursor` (Delete key). The cursor position remains unchanged.
pub(crate) fn delete_char_at_cursor(s: &mut String, cursor: usize) {
    let char_count = s.chars().count();
    if cursor < char_count {
        if let Some((byte_idx, _)) = s.char_indices().nth(cursor) {
            s.remove(byte_idx);
        }
    }
}

/// Formats text for a modal text field with an active cursor indicator.
/// When active:
/// - If cursor is within the string, the character under the cursor has inverted Cyan background and bold Black text.
/// - If cursor is at the end of the string, a Yellow '█' is appended.
/// When inactive:
/// - Text is rendered plainly with no cursor indicator.
/// If `is_masked` is true, characters are masked with '*'.
pub(crate) fn render_text_with_cursor(
    text: &str,
    cursor_pos: usize,
    is_active: bool,
    is_masked: bool,
) -> Line<'static> {
    let char_count = text.chars().count();
    let display_string = if is_masked {
        "*".repeat(char_count)
    } else {
        text.to_string()
    };

    if !is_active {
        return Line::from(Span::raw(display_string));
    }

    let chars: Vec<char> = display_string.chars().collect();
    let pos = cursor_pos.min(chars.len());

    let mut spans = Vec::new();
    if pos < chars.len() {
        let before: String = chars[..pos].iter().collect();
        if !before.is_empty() {
            spans.push(Span::raw(before));
        }
        let cur_char = chars[pos].to_string();
        spans.push(Span::styled(
            cur_char,
            Style::default()
                .bg(Color::Cyan)
                .fg(Color::Black)
                .add_modifier(Modifier::BOLD),
        ));
        let after: String = chars[pos + 1..].iter().collect();
        if !after.is_empty() {
            spans.push(Span::raw(after));
        }
    } else {
        if !display_string.is_empty() {
            spans.push(Span::raw(display_string));
        }
        spans.push(Span::styled("█", Style::default().fg(Color::Yellow)));
    }

    Line::from(spans)
}
