use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};

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
