use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use crate::tui::app::App;

const MAX_VISIBLE: usize = 10;
const MAX_INNER_WIDTH: u16 = 60;

/// Render the command palette overlay
pub fn render_command_palette(frame: &mut Frame, app: &App, area: Rect) {
    let cp = match &app.command_palette {
        Some(cp) => cp,
        None => return,
    };

    let bg = app.theme.background;
    let text_color = app.theme.text;
    let bright = app.theme.text_bright;
    let highlight = app.theme.highlight;
    let dim = app.theme.dim;

    let sel_bg = app.theme.selection_bg;

    // Styles
    let prompt_style = Style::default()
        .fg(highlight)
        .bg(bg)
        .add_modifier(Modifier::BOLD);
    let input_style = Style::default().fg(bright).bg(bg);
    let cursor_style = Style::default().fg(highlight).bg(bg);
    let normal_style = Style::default().fg(text_color).bg(bg);
    let footer_style = Style::default().fg(dim).bg(bg);
    let blank_style = Style::default().bg(bg);

    // Calculate overlay dimensions
    let content_width = area.width.saturating_sub(4); // 2 chars padding each side
    let inner_w = content_width.min(MAX_INNER_WIDTH) as usize;
    let popup_w = (inner_w as u16) + 2; // +2 for borders

    let visible_count = cp.results.len().min(MAX_VISIBLE);

    // Build lines
    let mut lines: Vec<Line> = Vec::new();

    // Input line: "> filter text|"
    let mut input_spans = vec![
        Span::styled(" > ", prompt_style),
        Span::styled(cp.input.clone(), input_style),
        Span::styled("\u{258C}", cursor_style),
    ];
    // Pad to fill width
    let input_used: usize = 3 + cp.input.chars().count() + 1;
    if input_used < inner_w {
        input_spans.push(Span::styled(" ".repeat(inner_w - input_used), blank_style));
    }
    lines.push(Line::from(input_spans));

    // Separator
    let sep = "\u{2500}".repeat(inner_w);
    lines.push(Line::from(Span::styled(
        sep,
        Style::default().fg(dim).bg(bg),
    )));

    // Results
    if cp.results.is_empty() {
        // Empty state
        lines.push(Line::from(Span::styled(" ".repeat(inner_w), blank_style)));
        let msg = "No matching actions";
        let msg_len = msg.chars().count();
        let left_pad = inner_w.saturating_sub(msg_len) / 2;
        let right_pad = inner_w.saturating_sub(msg_len + left_pad);
        lines.push(Line::from(vec![
            Span::styled(" ".repeat(left_pad), blank_style),
            Span::styled(msg, normal_style),
            Span::styled(" ".repeat(right_pad), blank_style),
        ]));
        lines.push(Line::from(Span::styled(" ".repeat(inner_w), blank_style)));
    } else {
        // Determine scroll window
        let scroll_offset = if cp.selected >= visible_count {
            cp.selected - visible_count + 1
        } else {
            0
        };

        for i in 0..visible_count {
            let result_idx = scroll_offset + i;
            if result_idx >= cp.results.len() {
                break;
            }
            let scored = &cp.results[result_idx];
            let is_selected = result_idx == cp.selected;

            // Per-row styles: selected row uses selection_bg
            let row_bg = if is_selected { sel_bg } else { bg };
            let row_pad = Style::default().bg(row_bg);
            let indicator_style = if is_selected {
                Style::default()
                    .fg(highlight)
                    .bg(row_bg)
                    .add_modifier(Modifier::BOLD)
            } else {
                row_pad
            };
            let label_style = if is_selected {
                Style::default()
                    .fg(bright)
                    .bg(row_bg)
                    .add_modifier(Modifier::BOLD)
            } else {
                normal_style
            };
            let sc_style = Style::default().fg(dim).bg(row_bg);
            let hl_style = Style::default()
                .fg(highlight)
                .bg(row_bg)
                .add_modifier(Modifier::BOLD);

            let indicator = if is_selected { " \u{25B6} " } else { "   " };
            let mut spans: Vec<Span> = vec![Span::styled(indicator, indicator_style)];

            // Build label with matched character highlights
            let label = &scored.action.label;
            let label_chars: Vec<char> = label.chars().collect();
            push_highlighted_chars(
                &mut spans,
                &label_chars,
                &scored.label_matched,
                label_style,
                hl_style,
                is_selected,
            );

            // Right-align shortcut with matched character highlights
            let shortcut_text = scored.action.shortcut.unwrap_or("");
            let label_len = 3 + label_chars.len(); // indicator + label
            let shortcut_len = shortcut_text.chars().count();
            let total_needed = label_len + 1 + shortcut_len; // +1 for min gap

            if total_needed < inner_w && !shortcut_text.is_empty() {
                let padding = inner_w - label_len - shortcut_len;
                spans.push(Span::styled(" ".repeat(padding), row_pad));
                let shortcut_chars: Vec<char> = shortcut_text.chars().collect();
                push_highlighted_chars(
                    &mut spans,
                    &shortcut_chars,
                    &scored.shortcut_matched,
                    sc_style,
                    hl_style,
                    is_selected,
                );
            } else if label_len < inner_w {
                spans.push(Span::styled(" ".repeat(inner_w - label_len), row_pad));
            }

            lines.push(Line::from(spans));
        }
    }

    // Blank line separator before footer
    lines.push(Line::from(Span::styled(" ".repeat(inner_w), blank_style)));

    // Footer: "  N of M actions"
    let footer_text = format!("   {} of {} actions", cp.results.len(), cp.total_count);
    let footer_len = footer_text.chars().count();
    let mut footer_spans = vec![Span::styled(footer_text, footer_style)];
    if footer_len < inner_w {
        footer_spans.push(Span::styled(" ".repeat(inner_w - footer_len), blank_style));
    }
    lines.push(Line::from(footer_spans));

    // Calculate height
    let popup_h = (lines.len() as u16 + 2).min(area.height.saturating_sub(2)); // +2 for borders

    // Position: centered horizontally, top at row 3 of content area
    let x = area.x + area.width.saturating_sub(popup_w) / 2;
    let y = area.y + 3.min(area.height.saturating_sub(popup_h));
    let popup_area = Rect::new(x, y, popup_w, popup_h);

    frame.render_widget(Clear, popup_area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(dim).bg(bg))
        .style(Style::default().bg(bg));

    let paragraph = Paragraph::new(lines)
        .block(block)
        .style(Style::default().bg(bg));

    frame.render_widget(paragraph, popup_area);
}

/// Push spans for a char array with specific indices highlighted.
fn push_highlighted_chars<'a>(
    spans: &mut Vec<Span<'a>>,
    chars: &[char],
    matched: &[usize],
    base_style: Style,
    highlight_style: Style,
    is_selected: bool,
) {
    let hl = if is_selected {
        highlight_style.add_modifier(Modifier::BOLD)
    } else {
        highlight_style
    };
    let mut last = 0;
    for &idx in matched {
        if idx >= chars.len() {
            continue;
        }
        if idx > last {
            let segment: String = chars[last..idx].iter().collect();
            spans.push(Span::styled(segment, base_style));
        }
        spans.push(Span::styled(chars[idx].to_string(), hl));
        last = idx + 1;
    }
    if last < chars.len() {
        let segment: String = chars[last..].iter().collect();
        spans.push(Span::styled(segment, base_style));
    }
}
