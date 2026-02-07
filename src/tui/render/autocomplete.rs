use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use crate::tui::app::App;

/// Maximum number of visible entries in the dropdown
const MAX_VISIBLE: usize = 8;

/// Render the autocomplete dropdown floating below the edit cursor
pub fn render_autocomplete(frame: &mut Frame, app: &App, edit_area: Rect) {
    let ac = match &app.autocomplete {
        Some(ac) if ac.visible && !ac.filtered.is_empty() => ac,
        _ => return,
    };

    let bg = app.theme.background;
    let text_color = app.theme.text;
    let bright = app.theme.text_bright;
    let dim = app.theme.dim;

    let count = ac.filtered.len().min(MAX_VISIBLE);

    // Determine the widest entry (+ padding)
    let max_width = ac
        .filtered
        .iter()
        .take(MAX_VISIBLE)
        .map(|s| s.len())
        .max()
        .unwrap_or(10)
        + 4; // padding

    let popup_w = (max_width as u16).min(edit_area.width.saturating_sub(2)).max(12);
    let popup_h = (count as u16) + 2; // +2 for borders

    // Position: below the edit area, offset from left
    // If not enough room below, show above
    let term_area = frame.area();
    let y = if edit_area.y + edit_area.height + popup_h <= term_area.height {
        edit_area.y + edit_area.height
    } else {
        edit_area.y.saturating_sub(popup_h)
    };

    // Horizontal: align with cursor position in edit buffer
    let cursor_offset = app.edit_cursor.min(edit_area.width as usize);
    let x = (edit_area.x + cursor_offset as u16).min(term_area.width.saturating_sub(popup_w));

    let popup_area = Rect::new(x, y, popup_w, popup_h);

    // Scroll window around selected item
    let scroll_start = if ac.selected >= MAX_VISIBLE {
        ac.selected - MAX_VISIBLE + 1
    } else {
        0
    };

    let mut lines: Vec<Line> = Vec::new();
    for (i, entry) in ac.filtered.iter().skip(scroll_start).take(MAX_VISIBLE).enumerate() {
        let actual_idx = scroll_start + i;
        let is_selected = actual_idx == ac.selected;

        let style = if is_selected {
            Style::default()
                .fg(bright)
                .bg(app.theme.selection_bg)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(text_color).bg(bg)
        };

        let prefix = if is_selected { " \u{25B8} " } else { "   " };
        let label = format!("{:<width$}", entry, width = (popup_w as usize).saturating_sub(5));

        lines.push(Line::from(vec![
            Span::styled(prefix, style),
            Span::styled(label, style),
        ]));
    }

    frame.render_widget(Clear, popup_area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(dim).bg(bg))
        .style(Style::default().bg(bg));

    let paragraph = Paragraph::new(lines).block(block).style(Style::default().bg(bg));
    frame.render_widget(paragraph, popup_area);
}
