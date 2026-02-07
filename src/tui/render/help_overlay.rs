use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use crate::tui::app::{App, View};

/// Render the help overlay (toggled with ?)
pub fn render_help_overlay(frame: &mut Frame, app: &App, area: Rect) {
    // Center the overlay, leaving some margin
    let overlay_area = centered_rect(60, 80, area);

    // Clear the area behind the overlay
    frame.render_widget(Clear, overlay_area);

    let bg = app.theme.background;
    let text_color = app.theme.text;
    let bright = app.theme.text_bright;
    let highlight = app.theme.highlight;
    let dim = app.theme.dim;

    let key_style = Style::default()
        .fg(highlight)
        .bg(bg)
        .add_modifier(Modifier::BOLD);
    let desc_style = Style::default().fg(text_color).bg(bg);
    let header_style = Style::default()
        .fg(bright)
        .bg(bg)
        .add_modifier(Modifier::BOLD);

    let mut lines: Vec<Line> = Vec::new();

    lines.push(Line::from(Span::styled(" Key Bindings", header_style)));
    lines.push(Line::from(""));

    // Context-sensitive help
    match &app.view {
        View::Track(_) => {
            lines.push(Line::from(Span::styled(" Navigation", header_style)));
            add_binding(
                &mut lines,
                " \u{2191}\u{2193}/jk",
                "Move cursor up/down",
                key_style,
                desc_style,
            );
            add_binding(
                &mut lines,
                " \u{2190}/h",
                "Collapse / go to parent",
                key_style,
                desc_style,
            );
            add_binding(
                &mut lines,
                " \u{2192}/l",
                "Expand / go to first child",
                key_style,
                desc_style,
            );
            add_binding(
                &mut lines,
                " g/G",
                "Jump to top/bottom",
                key_style,
                desc_style,
            );
            add_binding(
                &mut lines,
                " Enter",
                "Open detail view",
                key_style,
                desc_style,
            );
            add_binding(&mut lines, " Esc", "Back / close", key_style, desc_style);
            lines.push(Line::from(""));

            lines.push(Line::from(Span::styled(" Views", header_style)));
            add_binding(
                &mut lines,
                " 1-9",
                "Switch to track N",
                key_style,
                desc_style,
            );
            add_binding(&mut lines, " Tab", "Next track", key_style, desc_style);
            add_binding(&mut lines, " i", "Inbox view", key_style, desc_style);
            add_binding(&mut lines, " r", "Recent view", key_style, desc_style);
            add_binding(&mut lines, " 0/`", "Tracks view", key_style, desc_style);
            add_binding(&mut lines, " /", "Search", key_style, desc_style);
            lines.push(Line::from(""));
        }
        View::Tracks => {
            lines.push(Line::from(Span::styled(" Tracks View", header_style)));
            add_binding(
                &mut lines,
                " \u{2191}\u{2193}/jk",
                "Move cursor",
                key_style,
                desc_style,
            );
            add_binding(
                &mut lines,
                " 1-9",
                "Switch to track N",
                key_style,
                desc_style,
            );
            add_binding(&mut lines, " Tab", "Next view", key_style, desc_style);
            lines.push(Line::from(""));
        }
        View::Inbox => {
            lines.push(Line::from(Span::styled(" Inbox", header_style)));
            add_binding(
                &mut lines,
                " \u{2191}\u{2193}/jk",
                "Move cursor",
                key_style,
                desc_style,
            );
            add_binding(&mut lines, " /", "Search inbox", key_style, desc_style);
            lines.push(Line::from(""));
        }
        View::Recent => {
            lines.push(Line::from(Span::styled(" Recent", header_style)));
            add_binding(
                &mut lines,
                " \u{2191}\u{2193}/jk",
                "Move cursor",
                key_style,
                desc_style,
            );
            lines.push(Line::from(""));
        }
    }

    // Global keys
    lines.push(Line::from(Span::styled(" Global", header_style)));
    add_binding(&mut lines, " ?", "Toggle this help", key_style, desc_style);
    add_binding(&mut lines, " QQ", "Quit", key_style, desc_style);
    add_binding(&mut lines, " Ctrl+Q", "Quit (immediate)", key_style, desc_style);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(dim).bg(bg))
        .style(Style::default().bg(bg));

    let paragraph = Paragraph::new(lines)
        .block(block)
        .style(Style::default().bg(bg));

    frame.render_widget(paragraph, overlay_area);
}

fn add_binding<'a>(
    lines: &mut Vec<Line<'a>>,
    key: &'a str,
    desc: &'a str,
    key_style: Style,
    desc_style: Style,
) {
    let key_width = 16;
    let padded_key = format!("{:<width$}", key, width = key_width);
    lines.push(Line::from(vec![
        Span::styled(padded_key, key_style),
        Span::styled(desc, desc_style),
    ]));
}

/// Create a centered rectangle of the given percentage of the parent
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
