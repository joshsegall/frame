use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use crate::tui::app::App;

/// Render the conflict popup when an external change conflicts with in-progress edit
pub fn render_conflict_popup(frame: &mut Frame, app: &App, area: Rect) {
    let popup_w: u16 = 48.min(area.width.saturating_sub(2));
    let inner_w = popup_w.saturating_sub(2) as usize;

    let bg = app.theme.background;
    let text_color = app.theme.text;
    let bright = app.theme.text_bright;
    let highlight = app.theme.highlight;
    let header_style = Style::default()
        .fg(highlight)
        .bg(bg)
        .add_modifier(Modifier::BOLD);
    let text_style = Style::default().fg(text_color).bg(bg);
    let bright_style = Style::default().fg(bright).bg(bg);

    let orphaned = app.conflict_text.as_deref().unwrap_or("");

    // Build content lines with word wrapping
    let mut styled_lines: Vec<(String, Style)> = Vec::new();

    styled_lines.push((" External Change Conflict".into(), header_style));
    styled_lines.push(("".into(), text_style));

    for s in wrap_text(
        " ",
        "The task you were editing was modified externally.",
        inner_w,
    ) {
        styled_lines.push((s, text_style));
    }
    for s in wrap_text(" ", "Your unsaved text is shown below:", inner_w) {
        styled_lines.push((s, text_style));
    }
    styled_lines.push(("".into(), text_style));

    let quoted = format!("\u{201c}{}\u{201d}", orphaned);
    for s in wrap_text("   ", &quoted, inner_w) {
        styled_lines.push((s, bright_style));
    }
    styled_lines.push(("".into(), text_style));

    for s in wrap_text(
        " ",
        "Press Esc to dismiss. Re-enter edit mode (e) to retype.",
        inner_w,
    ) {
        styled_lines.push((s, text_style));
    }

    // Dynamic height from content + 2 for borders
    let popup_h = ((styled_lines.len() as u16) + 2).min(area.height.saturating_sub(2));

    let overlay_area = centered_rect_fixed(popup_w, popup_h, area);
    frame.render_widget(Clear, overlay_area);

    let lines: Vec<Line> = styled_lines
        .into_iter()
        .map(|(text, style)| Line::from(Span::styled(text, style)))
        .collect();

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(highlight).bg(bg))
        .style(Style::default().bg(bg));

    let paragraph = Paragraph::new(lines)
        .block(block)
        .style(Style::default().bg(bg));

    frame.render_widget(paragraph, overlay_area);
}

/// Word-wrap `text` into lines of at most `max_width` characters.
/// Every line (including the first) is prefixed with `indent`.
fn wrap_text(indent: &str, text: &str, max_width: usize) -> Vec<String> {
    let indent_len = indent.len();
    let mut lines = Vec::new();
    let mut current = indent.to_string();

    for word in text.split_whitespace() {
        let space = if current.len() == indent_len { 0 } else { 1 };
        if current.len() + space + word.len() > max_width && current.len() > indent_len {
            lines.push(current);
            current = indent.to_string();
        }
        if current.len() > indent_len {
            current.push(' ');
        }
        current.push_str(word);
    }
    if current.len() > indent_len || lines.is_empty() {
        lines.push(current);
    }
    lines
}

fn centered_rect_fixed(width: u16, height: u16, area: Rect) -> Rect {
    let x = area.x + area.width.saturating_sub(width) / 2;
    let y = area.y + area.height.saturating_sub(height) / 2;
    Rect::new(x, y, width, height)
}
