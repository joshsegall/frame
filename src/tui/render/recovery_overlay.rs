use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use crate::tui::app::App;
use crate::tui::wrap;

/// Render the recovery log overlay (full-screen popup)
pub fn render_recovery_overlay(frame: &mut Frame, app: &mut App, area: Rect) {
    let bg = app.theme.background;
    let text_color = app.theme.text;
    let bright = app.theme.text_bright;
    let dim = app.theme.dim;
    let highlight = app.theme.highlight;

    // Size: centered, taking most of the screen
    let margin_x = 4u16.min(area.width / 8);
    let margin_y = 2u16.min(area.height / 8);
    let popup_area = Rect::new(
        area.x + margin_x,
        area.y + margin_y,
        area.width.saturating_sub(margin_x * 2),
        area.height.saturating_sub(margin_y * 2),
    );

    frame.render_widget(Clear, popup_area);

    let block = Block::default()
        .title(Span::styled(
            " Recovery Log ",
            Style::default()
                .fg(bright)
                .bg(bg)
                .add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(dim).bg(bg))
        .style(Style::default().bg(bg));

    let inner = block.inner(popup_area);
    frame.render_widget(block, popup_area);

    if app.recovery_log_lines.is_empty() {
        let empty = Paragraph::new(Line::from(Span::styled(
            "No recovery log entries.",
            Style::default().fg(dim).bg(bg),
        )))
        .style(Style::default().bg(bg));
        frame.render_widget(empty, inner);
        return;
    }

    // Word-wrap logical lines to inner width, producing visual lines with styles
    let wrap_width = inner.width as usize;
    let mut visual_lines: Vec<(String, Style)> = Vec::new();
    let mut line_offsets: Vec<usize> = Vec::with_capacity(app.recovery_log_lines.len());

    for logical_line in &app.recovery_log_lines {
        line_offsets.push(visual_lines.len());

        let style = if logical_line.starts_with("## ") {
            Style::default()
                .fg(highlight)
                .bg(bg)
                .add_modifier(Modifier::BOLD)
        } else if logical_line.starts_with("```") || logical_line == "---" {
            Style::default().fg(dim).bg(bg)
        } else {
            Style::default().fg(text_color).bg(bg)
        };

        let wrapped = wrap::wrap_line(logical_line, wrap_width, 0);
        for vl in &wrapped {
            let text = &logical_line[vl.byte_start..vl.byte_end];
            visual_lines.push((text.to_string(), style));
        }
    }

    let total_visual = visual_lines.len();
    app.recovery_log_wrapped_count = total_visual;
    app.recovery_log_line_offsets = line_offsets;

    // Clamp scroll
    let visible_height = inner.height as usize;
    let scroll = app
        .recovery_log_scroll
        .min(total_visual.saturating_sub(visible_height));
    app.recovery_log_scroll = scroll;

    let lines: Vec<Line> = visual_lines
        .iter()
        .skip(scroll)
        .take(visible_height)
        .map(|(text, style)| Line::from(Span::styled(text.clone(), *style)))
        .collect();

    let paragraph = Paragraph::new(lines).style(Style::default().bg(bg));
    frame.render_widget(paragraph, inner);

    // Scroll indicator
    if total_visual > visible_height {
        let indicator = format!(
            " {}/{} ",
            scroll + 1,
            total_visual.saturating_sub(visible_height) + 1
        );
        let indicator_style = Style::default()
            .fg(Color::Black)
            .bg(dim)
            .add_modifier(Modifier::BOLD);
        let indicator_width = indicator.len() as u16;
        let indicator_x = popup_area.x + popup_area.width.saturating_sub(indicator_width + 1);
        let indicator_y = popup_area.y + popup_area.height - 1;
        if indicator_x < popup_area.x + popup_area.width && indicator_y < area.y + area.height {
            let indicator_area = Rect::new(indicator_x, indicator_y, indicator_width, 1);
            let indicator_widget =
                Paragraph::new(Line::from(Span::styled(indicator, indicator_style)));
            frame.render_widget(indicator_widget, indicator_area);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::render::test_helpers::*;
    use insta::assert_snapshot;

    #[test]
    fn recovery_log_visible() {
        let mut app = app_with_track(SIMPLE_TRACK_MD);
        app.show_recovery_log = true;
        app.recovery_log_lines = vec![
            "Recovery entry 1: restored track.md".into(),
            "Recovery entry 2: restored inbox.md".into(),
        ];
        let output = render_to_string(TERM_W, TERM_H, |frame, area| {
            render_recovery_overlay(frame, &mut app, area);
        });
        assert_snapshot!(output);
    }
}
