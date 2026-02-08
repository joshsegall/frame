use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::tui::app::{App, StateFilter, View};

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
    let bg = app.theme.background;
    let dim = app.theme.dim;

    // Build filter indicator text if filter is active and in track view
    let is_track_view = matches!(app.view, View::Track(_));
    let filter = &app.filter_state;

    if is_track_view && filter.is_active() {
        // Build indicator spans: "filter: " + state + " " + #tag
        let mut indicator_spans: Vec<Span> = Vec::new();
        indicator_spans.push(Span::styled("filter: ", Style::default().fg(app.theme.purple).bg(bg)));

        if let Some(sf) = &filter.state_filter {
            let state_color = match sf {
                StateFilter::Active => app.theme.state_color(crate::model::TaskState::Active),
                StateFilter::Todo => app.theme.state_color(crate::model::TaskState::Todo),
                StateFilter::Blocked => app.theme.state_color(crate::model::TaskState::Blocked),
                StateFilter::Parked => app.theme.state_color(crate::model::TaskState::Parked),
                StateFilter::Ready => app.theme.state_color(crate::model::TaskState::Active),
            };
            indicator_spans.push(Span::styled(
                sf.label(),
                Style::default().fg(state_color).bg(bg),
            ));
        }

        if let Some(ref tag) = filter.tag_filter {
            if filter.state_filter.is_some() {
                indicator_spans.push(Span::styled(" ", Style::default().bg(bg)));
            }
            let tag_color = app.theme.tag_color(tag);
            indicator_spans.push(Span::styled(
                format!("#{}", tag),
                Style::default().fg(tag_color).bg(bg),
            ));
        }

        // Calculate indicator width
        let indicator_width: usize = indicator_spans.iter().map(|s| s.content.chars().count()).sum();
        // +2: one space before indicator, one space after (right edge buffer)
        let separator_end = width.saturating_sub(indicator_width + 2);

        let mut spans: Vec<Span> = Vec::new();
        // Build separator chars up to where indicator starts
        let mut sep_text = String::with_capacity(separator_end * 3);
        for col in 0..separator_end {
            if sep_cols.contains(&col) {
                sep_text.push('\u{2534}');
            } else {
                sep_text.push('\u{2500}');
            }
        }
        spans.push(Span::styled(sep_text, Style::default().fg(dim).bg(bg)));
        spans.push(Span::styled(" ", Style::default().bg(bg)));
        spans.extend(indicator_spans);
        // Trailing space
        let current_width: usize = spans.iter().map(|s| s.content.chars().count()).sum();
        if current_width < width {
            spans.push(Span::styled(
                " ".repeat(width - current_width),
                Style::default().bg(bg),
            ));
        }

        let line = Line::from(spans);
        let sep_widget = Paragraph::new(line).style(Style::default().bg(bg));
        frame.render_widget(sep_widget, area);
    } else {
        // No filter — plain separator
        let mut line: String = String::with_capacity(width * 3);
        for col in 0..width {
            if sep_cols.contains(&col) {
                line.push('\u{2534}');
            } else {
                line.push('\u{2500}');
            }
        }
        let sep_widget = Paragraph::new(line)
            .style(Style::default().fg(dim).bg(bg));
        frame.render_widget(sep_widget, area);
    }
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
