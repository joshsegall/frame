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

    let sep_cols = render_tabs(frame, app, chunks[0]);
    render_separator(frame, app, chunks[1], &sep_cols);
}

/// Render tabs and return the column positions of each separator character.
fn render_tabs(frame: &mut Frame, app: &App, area: Rect) -> Vec<usize> {
    let mut spans: Vec<Span> = Vec::new();
    let mut sep_cols: Vec<usize> = Vec::new();
    let sep = Span::styled(
        "\u{2502}",
        Style::default().fg(app.theme.dim).bg(app.theme.background),
    );

    // Leading icon
    let bg_style = Style::default().bg(app.theme.background);
    spans.push(Span::styled(" ", bg_style));
    spans.push(Span::styled(
        "\u{25B6}",
        Style::default().fg(app.theme.purple).bg(app.theme.background),
    ));
    spans.push(Span::styled(" ", bg_style));

    // Active track tabs
    let cc_focus = app.project.config.agent.cc_focus.as_deref();
    for (i, track_id) in app.active_track_ids.iter().enumerate() {
        let name = app.track_name(track_id);
        let track_id_str = track_id.as_str();
        let is_current = app.view == View::Track(i)
            || matches!(&app.view, View::Detail { track_id: tid, .. } if tid == track_id_str);
        let is_cc = cc_focus == Some(track_id.as_str());
        let style = tab_style(app, is_current);
        if is_cc {
            spans.push(Span::styled(format!(" {} ", name), style));
            spans.push(Span::styled(
                "\u{2605}",
                Style::default()
                    .fg(app.theme.purple)
                    .bg(if is_current { app.theme.selection_bg } else { app.theme.background }),
            ));
            spans.push(Span::styled(
                " ",
                Style::default()
                    .bg(if is_current { app.theme.selection_bg } else { app.theme.background }),
            ));
        } else {
            spans.push(Span::styled(format!(" {} ", name), style));
        }
        sep_cols.push(spans.iter().map(|s| s.content.chars().count()).sum());
        spans.push(sep.clone());
    }

    // Tracks view tab (▶)
    let is_tracks = app.view == View::Tracks;
    spans.push(Span::styled(" \u{25B6} ", tab_style(app, is_tracks)));
    sep_cols.push(spans.iter().map(|s| s.content.chars().count()).sum());
    spans.push(sep.clone());

    // Inbox tab with count (*N)
    let inbox_count = app.inbox_count();
    let is_inbox = app.view == View::Inbox;
    let tab_bg = if is_inbox { app.theme.selection_bg } else { app.theme.background };
    let style = tab_style(app, is_inbox);
    spans.push(Span::styled(" ", style));
    spans.push(Span::styled(
        "*",
        Style::default()
            .fg(app.theme.purple)
            .bg(tab_bg),
    ));
    if inbox_count > 0 {
        spans.push(Span::styled(format!("{} ", inbox_count), style));
    } else {
        spans.push(Span::styled(" ", style));
    }
    sep_cols.push(spans.iter().map(|s| s.content.chars().count()).sum());
    spans.push(sep.clone());

    // Recent tab (✓)
    let is_recent = app.view == View::Recent;
    spans.push(Span::styled(" \u{2713} ", tab_style(app, is_recent)));
    sep_cols.push(spans.iter().map(|s| s.content.chars().count()).sum());
    spans.push(sep.clone());

    let line = Line::from(spans);
    let tabs = Paragraph::new(line).style(Style::default().bg(app.theme.background));
    frame.render_widget(tabs, area);
    sep_cols
}

fn render_separator(frame: &mut Frame, app: &App, area: Rect, sep_cols: &[usize]) {
    let width = area.width as usize;
    let mut line: String = String::with_capacity(width * 3);
    for col in 0..width {
        if sep_cols.contains(&col) {
            line.push('\u{2534}'); // ┴
        } else {
            line.push('\u{2500}'); // ─
        }
    }
    let sep_widget = Paragraph::new(line)
        .style(Style::default().fg(app.theme.dim).bg(app.theme.background));
    frame.render_widget(sep_widget, area);
}

/// Style for a tab: highlighted if current, normal otherwise
fn tab_style(app: &App, is_current: bool) -> Style {
    if is_current {
        Style::default()
            .fg(app.theme.text_bright)
            .bg(app.theme.selection_bg)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(app.theme.text).bg(app.theme.background)
    }
}
