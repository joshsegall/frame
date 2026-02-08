use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use crate::tui::app::App;

/// Render the prefix rename confirmation popup
pub fn render_prefix_confirm(frame: &mut Frame, app: &App, area: Rect) {
    let pr = match &app.prefix_rename {
        Some(pr) if pr.confirming => pr,
        _ => return,
    };

    let bg = app.theme.background;
    let text_color = app.theme.text;
    let bright = app.theme.text_bright;
    let dim = app.theme.dim;
    let highlight = app.theme.highlight;
    let warn_color = app.theme.state_color(crate::model::TaskState::Blocked);

    let header_style = Style::default()
        .fg(highlight)
        .bg(bg)
        .add_modifier(Modifier::BOLD);
    let text_style = Style::default().fg(text_color).bg(bg);
    let bright_style = Style::default()
        .fg(bright)
        .bg(bg)
        .add_modifier(Modifier::BOLD);
    let warn_style = Style::default().fg(warn_color).bg(bg);
    let dim_style = Style::default().fg(dim).bg(bg);

    let popup_w: u16 = 50.min(area.width.saturating_sub(2));

    let mut lines: Vec<Line> = Vec::new();

    // Title
    lines.push(Line::from(Span::styled(" Rename Prefix", header_style)));
    lines.push(Line::from(Span::styled("", text_style)));

    // Old → New
    lines.push(Line::from(vec![
        Span::styled("  ", text_style),
        Span::styled(&pr.old_prefix, bright_style),
        Span::styled(" \u{2192} ", text_style),
        Span::styled(&pr.new_prefix, bright_style),
    ]));
    lines.push(Line::from(Span::styled("", text_style)));

    // Blast radius
    lines.push(Line::from(Span::styled("  This will rename:", text_style)));
    lines.push(Line::from(Span::styled(
        format!(
            "    {} task ID{} in {}",
            pr.task_id_count,
            if pr.task_id_count == 1 { "" } else { "s" },
            pr.track_name,
        ),
        text_style,
    )));
    if pr.dep_ref_count > 0 {
        lines.push(Line::from(Span::styled(
            format!(
                "    {} dep reference{} across {} other track{}",
                pr.dep_ref_count,
                if pr.dep_ref_count == 1 { "" } else { "s" },
                pr.affected_track_count,
                if pr.affected_track_count == 1 {
                    ""
                } else {
                    "s"
                },
            ),
            text_style,
        )));
    } else {
        lines.push(Line::from(Span::styled(
            "    0 dep references across other tracks",
            text_style,
        )));
    }
    lines.push(Line::from(Span::styled("", text_style)));

    // Warning
    lines.push(Line::from(Span::styled(
        "  This cannot be undone. Use git to revert.",
        warn_style,
    )));
    lines.push(Line::from(Span::styled("", text_style)));

    // Key hints
    lines.push(Line::from(vec![
        Span::styled("  ", text_style),
        Span::styled("Enter", dim_style),
        Span::styled(" confirm  ", text_style),
        Span::styled("Esc", dim_style),
        Span::styled(" cancel", text_style),
    ]));

    let popup_h = ((lines.len() as u16) + 2).min(area.height.saturating_sub(2));

    let overlay_area = centered_rect_fixed(popup_w, popup_h, area);
    frame.render_widget(Clear, overlay_area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(highlight).bg(bg))
        .style(Style::default().bg(bg));

    let paragraph = Paragraph::new(lines)
        .block(block)
        .style(Style::default().bg(bg));

    frame.render_widget(paragraph, overlay_area);
}

fn centered_rect_fixed(width: u16, height: u16, area: Rect) -> Rect {
    let x = area.x + area.width.saturating_sub(width) / 2;
    let y = area.y + area.height.saturating_sub(height) / 2;
    Rect::new(x, y, width, height)
}
