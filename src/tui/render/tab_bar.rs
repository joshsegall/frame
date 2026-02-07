use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::tui::app::{App, View};

/// Render the tab bar: track tabs + special tabs, with separator line below
pub fn render_tab_bar(frame: &mut Frame, app: &App, area: Rect) {
    // Split into tab row and separator row
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // tabs
            Constraint::Length(1), // separator
        ])
        .split(area);

    render_tabs(frame, app, chunks[0]);
    render_separator(frame, app, chunks[1]);
}

fn render_tabs(frame: &mut Frame, app: &App, area: Rect) {
    let mut spans: Vec<Span> = Vec::new();
    let sep = Span::styled(
        "\u{2502}",
        Style::default().fg(app.theme.dim).bg(app.theme.background),
    );

    // Active track tabs
    for (i, track_id) in app.active_track_ids.iter().enumerate() {
        let name = app.track_name(track_id);
        let is_current = app.view == View::Track(i);
        let style = tab_style(app, is_current);
        spans.push(Span::styled(format!(" {} ", name), style));
        spans.push(sep.clone());
    }

    // Tracks view tab (▸)
    let is_tracks = app.view == View::Tracks;
    spans.push(Span::styled(" \u{25B8} ", tab_style(app, is_tracks)));
    spans.push(sep.clone());

    // Inbox tab with count (🔥N)
    let inbox_count = app.inbox_count();
    let is_inbox = app.view == View::Inbox;
    let inbox_label = if inbox_count > 0 {
        format!(" \u{1F525}{} ", inbox_count)
    } else {
        " \u{1F525} ".to_string()
    };
    spans.push(Span::styled(inbox_label, tab_style(app, is_inbox)));
    spans.push(sep.clone());

    // Recent tab (✓)
    let is_recent = app.view == View::Recent;
    spans.push(Span::styled(" \u{2713} ", tab_style(app, is_recent)));
    spans.push(sep.clone());

    let line = Line::from(spans);
    let tabs = Paragraph::new(line).style(Style::default().bg(app.theme.background));
    frame.render_widget(tabs, area);
}

fn render_separator(frame: &mut Frame, app: &App, area: Rect) {
    let separator = "\u{2500}".repeat(area.width as usize);
    let sep_widget = Paragraph::new(separator)
        .style(Style::default().fg(app.theme.dim).bg(app.theme.background));
    frame.render_widget(sep_widget, area);
}

/// Style for a tab: highlighted if current, normal otherwise
fn tab_style(app: &App, is_current: bool) -> Style {
    if is_current {
        Style::default()
            .fg(app.theme.text_bright)
            .bg(app.theme.highlight)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(app.theme.text).bg(app.theme.background)
    }
}
