use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use crate::tui::app::App;

/// Maximum number of visible entries in the dropdown
const MAX_VISIBLE: usize = 8;

/// Render the autocomplete dropdown anchored to the edit cursor position.
///
/// Positioning rules:
/// - Left edge aligned with the start of the edit text (autocomplete_anchor.x),
///   shifted left if the popup would overflow the right edge of the screen.
/// - If there is enough room below the cursor line, the popup appears just below it
///   with the top edge fixed; the bottom grows/shrinks with the entry count.
/// - If there is NOT enough room below, the popup appears above the cursor line
///   with the bottom edge fixed just above the cursor; the top grows/shrinks.
pub fn render_autocomplete(frame: &mut Frame, app: &App, content_area: Rect) {
    let ac = match &app.autocomplete {
        Some(ac) if ac.visible && !ac.filtered.is_empty() => ac,
        _ => return,
    };

    let (anchor_x, anchor_y) = match app.autocomplete_anchor {
        Some(pos) => pos,
        None => return,
    };

    let bg = app.theme.background;
    let text_color = app.theme.text;
    let bright = app.theme.text_bright;
    let dim = app.theme.dim;

    let count = ac.filtered.len().min(MAX_VISIBLE);

    // Width = widest entry across ALL filtered entries + chrome (borders + prefix + padding)
    // Chrome: 1 (left border) + 3 (prefix " ▶ ") + 1 (right padding) + 1 (right border) = 6
    let max_entry_width = ac
        .filtered
        .iter()
        .map(|s| s.len())
        .max()
        .unwrap_or(8);
    let max_width = max_entry_width + 6;

    let term_area = frame.area();
    let max_popup_w: u16 = 40;
    let popup_w = (max_width as u16)
        .max(12)
        .min(max_popup_w)
        .min(content_area.width.saturating_sub(2));
    let popup_h = (count as u16) + 2; // +2 for borders

    // Vertical: prefer below the cursor line, fall back to above
    let cursor_bottom = anchor_y + 1; // row just below the edit line
    let y = if cursor_bottom + popup_h <= term_area.height {
        // Enough room below: top of popup is just below cursor line
        cursor_bottom
    } else {
        // Not enough room below: bottom of popup is just above cursor line
        anchor_y.saturating_sub(popup_h)
    };

    // Horizontal: align entry text with the cursor insertion point.
    // The entry text is inset by 1 (left border) + 3 (prefix " ▸ ") = 4 chars,
    // so shift the popup left by that amount. Clamp to screen bounds.
    let text_inset: u16 = 4;
    let x = anchor_x.saturating_sub(text_inset).min(term_area.width.saturating_sub(popup_w));

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

        let prefix = if is_selected { " \u{25B6} " } else { "   " };
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
