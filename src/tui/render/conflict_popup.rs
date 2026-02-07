use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

use crate::tui::app::App;

/// Render the conflict popup when an external change conflicts with in-progress edit
pub fn render_conflict_popup(frame: &mut Frame, app: &App, area: Rect) {
    let overlay_area = centered_rect(60, 50, area);

    frame.render_widget(Clear, overlay_area);

    let bg = app.theme.background;
    let text_color = app.theme.text;
    let bright = app.theme.text_bright;
    let highlight = app.theme.highlight;
    let dim = app.theme.dim;

    let header_style = Style::default()
        .fg(highlight)
        .bg(bg)
        .add_modifier(Modifier::BOLD);
    let text_style = Style::default().fg(text_color).bg(bg);
    let dim_style = Style::default().fg(dim).bg(bg);
    let bright_style = Style::default().fg(bright).bg(bg);

    let orphaned = app
        .conflict_text
        .as_deref()
        .unwrap_or("");

    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::from(Span::styled(" External Change Conflict", header_style)));
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        " The task you were editing was modified externally.",
        text_style,
    )));
    lines.push(Line::from(Span::styled(
        " Your unsaved edit text is shown below:",
        text_style,
    )));
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        format!("   \u{201c}{}\u{201d}", orphaned),
        bright_style,
    )));
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        " Press Esc to dismiss. You can re-enter edit mode (e) to retype.",
        dim_style,
    )));

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(highlight).bg(bg))
        .style(Style::default().bg(bg));

    let paragraph = Paragraph::new(lines)
        .block(block)
        .wrap(Wrap { trim: false })
        .style(Style::default().bg(bg));

    frame.render_widget(paragraph, overlay_area);
}

fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}
