use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use crate::tui::app::{App, TAG_COLOR_PALETTE};
use crate::tui::theme;

/// Render the tag color editor popup overlay
pub fn render_tag_color_popup(frame: &mut Frame, app: &App, area: Rect) {
    let tcp = match &app.tag_color_popup {
        Some(tcp) => tcp,
        None => return,
    };

    let bg = app.theme.background;
    let highlight = app.theme.highlight;
    let dim = app.theme.dim;
    let sel_bg = app.theme.selection_bg;

    // Sizing: 50% width, min 36, max 60
    let target_w = (area.width as f32 * 0.5) as u16;
    let inner_w = target_w.clamp(36, 60).min(area.width.saturating_sub(2)) as usize;
    let popup_w = (inner_w as u16) + 2; // +2 for borders

    let mut lines: Vec<Line> = Vec::new();

    // Blank line at top
    lines.push(Line::from(Span::styled(
        " ".repeat(inner_w),
        Style::default().bg(bg),
    )));

    if tcp.tags.is_empty() {
        // Empty state
        let empty_lines = [
            "  No tags in project.",
            "  Add tags to tasks with t in the TUI",
            "  or --tag in the CLI.",
        ];
        for text in &empty_lines {
            let mut spans = vec![Span::styled(
                text.to_string(),
                Style::default().fg(dim).bg(bg),
            )];
            let used = text.chars().count();
            if used < inner_w {
                spans.push(Span::styled(
                    " ".repeat(inner_w - used),
                    Style::default().bg(bg),
                ));
            }
            lines.push(Line::from(spans));
        }
    } else {
        // Compute max tag name length for column alignment
        let max_tag_len = tcp
            .tags
            .iter()
            .map(|(name, _)| name.chars().count())
            .max()
            .unwrap_or(0);
        // Tag list
        for (i, (tag_name, hex_opt)) in tcp.tags.iter().enumerate() {
            let is_selected = i == tcp.cursor;
            let row_bg = if is_selected { sel_bg } else { bg };
            let row_pad = Style::default().bg(row_bg);

            let mut spans: Vec<Span> = Vec::new();

            // Cursor indicator
            let indicator = if is_selected { " \u{25B6} " } else { "   " };
            spans.push(Span::styled(indicator, row_pad));

            // Tag name — rendered in its assigned color
            let tag_color = resolve_tag_color(app, tag_name, hex_opt.as_deref());
            let tag_style = if is_selected {
                Style::default()
                    .fg(tag_color)
                    .bg(row_bg)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(tag_color).bg(row_bg)
            };
            spans.push(Span::styled(tag_name.to_string(), tag_style));

            // Pad tag name to fixed column width
            let tag_chars = tag_name.chars().count();
            let pad_after_tag = max_tag_len + 2 - tag_chars; // align to color_col
            spans.push(Span::styled(" ".repeat(pad_after_tag), row_pad));

            if is_selected && tcp.picker_open {
                // Inline palette picker
                render_palette_swatches(&mut spans, tcp, row_bg, dim);
            } else {
                // Color label or (none)
                render_color_label(
                    &mut spans,
                    hex_opt.as_deref(),
                    app,
                    tag_name,
                    row_bg,
                    dim,
                );
            }

            pad_to_width(&mut spans, inner_w, row_pad);
            lines.push(Line::from(spans));
        }
    }

    // Blank line before hint bar
    lines.push(Line::from(Span::styled(
        " ".repeat(inner_w),
        Style::default().bg(bg),
    )));

    // Hint bar
    let hint = if tcp.picker_open {
        "\u{2190}\u{2192} pick  Enter \u{2713}  Bksp clear  Esc cancel"
    } else if tcp.tags.is_empty() {
        "Esc close"
    } else {
        "Enter pick  Bksp clear  Esc close"
    };
    let hint_style = Style::default().fg(dim).bg(bg);
    let hint_len = hint.chars().count();
    let hint_pad = inner_w.saturating_sub(hint_len);
    let left_pad = hint_pad / 2;
    let right_pad = hint_pad - left_pad;
    lines.push(Line::from(vec![
        Span::styled(" ".repeat(left_pad), Style::default().bg(bg)),
        Span::styled(hint, hint_style),
        Span::styled(" ".repeat(right_pad), Style::default().bg(bg)),
    ]));

    // Height: content-sized up to 70%
    let max_h = ((area.height as f32) * 0.7) as u16;
    let content_h = lines.len() as u16;
    let popup_h = (content_h + 2)
        .min(max_h)
        .min(area.height.saturating_sub(2));

    // Position: centered
    let x = area.x + area.width.saturating_sub(popup_w) / 2;
    let y = area.y + area.height.saturating_sub(popup_h) / 2;
    let popup_area = Rect::new(x, y, popup_w, popup_h);

    frame.render_widget(Clear, popup_area);

    // Title
    let title = " Tag Colors ";
    let title_style = Style::default()
        .fg(highlight)
        .bg(bg)
        .add_modifier(Modifier::BOLD);

    let block = Block::default()
        .title(Span::styled(title, title_style))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(dim).bg(bg))
        .style(Style::default().bg(bg));

    // Scroll
    let inner_h = popup_h.saturating_sub(2) as usize;
    let scroll = tcp.scroll_offset;

    let paragraph = Paragraph::new(lines)
        .block(block)
        .scroll((scroll as u16, 0))
        .style(Style::default().bg(bg));

    frame.render_widget(paragraph, popup_area);

    // Scroll indicators
    if scroll > 0 {
        let up_area = Rect::new(popup_area.x + popup_w - 2, popup_area.y, 1, 1);
        frame.render_widget(
            Paragraph::new(Span::styled("\u{25B2}", Style::default().fg(dim).bg(bg))),
            up_area,
        );
    }
    if content_h > inner_h as u16 && scroll + inner_h < content_h as usize {
        let down_area = Rect::new(popup_area.x + popup_w - 2, popup_area.y + popup_h - 1, 1, 1);
        frame.render_widget(
            Paragraph::new(Span::styled("\u{25BC}", Style::default().fg(dim).bg(bg))),
            down_area,
        );
    }
}

/// Render the color label (swatch + name) or "(none)" for a tag row.
/// Called after the tag name has already been padded to column width.
fn render_color_label<'a>(
    spans: &mut Vec<Span<'a>>,
    hex_opt: Option<&str>,
    app: &App,
    tag_name: &str,
    row_bg: ratatui::style::Color,
    dim: ratatui::style::Color,
) {
    match hex_opt {
        Some(hex) => {
            let label = palette_label_for_hex(hex);
            let color = resolve_tag_color(app, tag_name, Some(hex));
            spans.push(Span::styled(
                "\u{25A0} ",
                Style::default().fg(color).bg(row_bg),
            ));
            spans.push(Span::styled(
                label.to_string(),
                Style::default().fg(color).bg(row_bg),
            ));
        }
        None => {
            spans.push(Span::styled(
                "  (none)",
                Style::default().fg(dim).bg(row_bg),
            ));
        }
    }
}

/// Render the inline palette swatches for the picker mode.
/// Called after the tag name has already been padded to column width.
fn render_palette_swatches<'a>(
    spans: &mut Vec<Span<'a>>,
    tcp: &crate::tui::app::TagColorPopupState,
    row_bg: ratatui::style::Color,
    dim: ratatui::style::Color,
) {
    for (i, (_, hex)) in TAG_COLOR_PALETTE.iter().enumerate() {
        let color = theme::parse_hex_color_pub(hex).unwrap_or(ratatui::style::Color::White);
        let is_picker_selected = i == tcp.picker_cursor;
        let style = if is_picker_selected {
            Style::default()
                .fg(color)
                .bg(row_bg)
                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
        } else {
            Style::default().fg(color).bg(row_bg)
        };
        spans.push(Span::styled("\u{25A0} ", style));
    }

    // "×" for clear option (after all swatches)
    let is_clear_selected = tcp.picker_cursor == TAG_COLOR_PALETTE.len();
    let clear_style = if is_clear_selected {
        Style::default()
            .fg(dim)
            .bg(row_bg)
            .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
    } else {
        Style::default().fg(dim).bg(row_bg)
    };
    spans.push(Span::styled(" \u{00D7}", clear_style));
}

/// Find the palette label for a hex color, or return the hex as-is for custom colors
fn palette_label_for_hex(hex: &str) -> &str {
    let hex_upper = hex.to_uppercase();
    for (label, palette_hex) in TAG_COLOR_PALETTE {
        if palette_hex.to_uppercase() == hex_upper {
            return label;
        }
    }
    // Custom hex — return as-is (borrow won't work, but we can leak since it's display-only)
    // Actually, return a static reference isn't possible for dynamic strings.
    // We'll handle this differently — return the hex from the palette or a placeholder
    hex // The caller should handle lifetimes; since hex comes from the tag data, this is fine
}

/// Resolve the display color for a tag — uses theme tag_color (which includes defaults + config)
fn resolve_tag_color(app: &App, tag_name: &str, hex_opt: Option<&str>) -> ratatui::style::Color {
    // If there's an explicit hex in config, parse it directly
    if let Some(hex) = hex_opt
        && let Some(color) = theme::parse_hex_color_pub(hex)
    {
        return color;
    }
    // Fall back to theme (includes hardcoded defaults)
    app.theme.tag_color(tag_name)
}

/// Pad spans to fill `target_width` with background.
fn pad_to_width<'a>(spans: &mut Vec<Span<'a>>, target_width: usize, pad_style: Style) {
    let total_used: usize = spans.iter().map(|s| s.content.chars().count()).sum();
    if total_used < target_width {
        spans.push(Span::styled(
            " ".repeat(target_width - total_used),
            pad_style,
        ));
    }
}
