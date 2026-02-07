use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::ops::track_ops::{TrackStats, task_counts};
use crate::tui::app::App;

use super::push_highlighted_spans;

/// Render the tracks overview: all tracks grouped by state with stats
pub fn render_tracks_view(frame: &mut Frame, app: &App, area: Rect) {
    let mut lines: Vec<Line> = Vec::new();
    let cursor = app.tracks_cursor;

    // Group tracks by state
    let mut active_tracks = Vec::new();
    let mut shelved_tracks = Vec::new();
    let mut archived_tracks = Vec::new();

    for tc in &app.project.config.tracks {
        match tc.state.as_str() {
            "active" => active_tracks.push(tc),
            "shelved" => shelved_tracks.push(tc),
            "archived" => archived_tracks.push(tc),
            _ => {}
        }
    }

    let cc_focus = app.project.config.agent.cc_focus.as_deref();
    let search_re = app.active_search_re();

    let mut flat_idx = 0usize;

    // Active section
    if !active_tracks.is_empty() {
        lines.push(Line::from(Span::styled(
            " Active",
            Style::default()
                .fg(app.theme.text)
                .bg(app.theme.background)
                .add_modifier(Modifier::BOLD),
        )));
        for (i, tc) in active_tracks.iter().enumerate() {
            let is_cursor = flat_idx == cursor;
            lines.push(render_track_line(
                app,
                tc,
                i + 1,
                is_cursor,
                cc_focus,
                area.width,
                search_re.as_ref(),
            ));
            flat_idx += 1;
        }
        lines.push(Line::from(""));
    }

    // Shelved section
    if !shelved_tracks.is_empty() {
        lines.push(Line::from(Span::styled(
            " Shelved",
            Style::default()
                .fg(app.theme.text)
                .bg(app.theme.background)
                .add_modifier(Modifier::BOLD),
        )));
        for tc in &shelved_tracks {
            let is_cursor = flat_idx == cursor;
            let idx = flat_idx + 1;
            lines.push(render_track_line(
                app,
                tc,
                idx,
                is_cursor,
                cc_focus,
                area.width,
                search_re.as_ref(),
            ));
            flat_idx += 1;
        }
        lines.push(Line::from(""));
    }

    // Archived section
    if !archived_tracks.is_empty() {
        lines.push(Line::from(Span::styled(
            " Archived",
            Style::default()
                .fg(app.theme.dim)
                .bg(app.theme.background)
                .add_modifier(Modifier::BOLD),
        )));
        for tc in &archived_tracks {
            let is_cursor = flat_idx == cursor;
            let idx = flat_idx + 1;
            lines.push(render_track_line(
                app,
                tc,
                idx,
                is_cursor,
                cc_focus,
                area.width,
                search_re.as_ref(),
            ));
            flat_idx += 1;
        }
    }

    if lines.is_empty() {
        lines.push(Line::from(Span::styled(
            " No tracks",
            Style::default().fg(app.theme.dim).bg(app.theme.background),
        )));
    }

    let paragraph = Paragraph::new(lines).style(Style::default().bg(app.theme.background));
    frame.render_widget(paragraph, area);
}

fn render_track_line<'a>(
    app: &'a App,
    tc: &crate::model::TrackConfig,
    _number: usize,
    is_cursor: bool,
    cc_focus: Option<&str>,
    width: u16,
    search_re: Option<&regex::Regex>,
) -> Line<'a> {
    let bg = if is_cursor {
        app.theme.highlight
    } else {
        app.theme.background
    };
    let text_color = app.theme.text_bright;

    // Get stats for this track
    let stats = app
        .project
        .tracks
        .iter()
        .find(|(id, _)| id == &tc.id)
        .map(|(_, track)| task_counts(track))
        .unwrap_or_default();

    let mut spans: Vec<Span> = Vec::new();

    // Indent
    spans.push(Span::styled("  ", Style::default().bg(bg)));

    // Track name (with search highlighting)
    let name_style = Style::default().fg(text_color).bg(bg);
    let hl_style = name_style.bg(app.theme.purple);
    push_highlighted_spans(&mut spans, &tc.name, name_style, hl_style, search_re);

    // Stats
    let stats_str = format_stats(&stats, app, bg);
    spans.push(Span::styled("  ", Style::default().bg(bg)));
    for s in stats_str {
        spans.push(s);
    }

    // cc-focus indicator
    if cc_focus == Some(tc.id.as_str()) {
        spans.push(Span::styled("  ", Style::default().bg(bg)));
        spans.push(Span::styled(
            "\u{2605}cc",
            Style::default().fg(app.theme.purple).bg(bg),
        ));
    }

    // Pad to full width for cursor
    if is_cursor {
        let content_width: usize = spans.iter().map(|s| s.content.chars().count()).sum();
        let w = width as usize;
        if content_width < w {
            spans.push(Span::styled(
                " ".repeat(w - content_width),
                Style::default().bg(bg),
            ));
        }
    }

    Line::from(spans)
}

fn format_stats<'a>(stats: &TrackStats, app: &'a App, bg: ratatui::style::Color) -> Vec<Span<'a>> {
    let mut spans = Vec::new();
    let items: Vec<(usize, &str, ratatui::style::Color)> = vec![
        (stats.active, "\u{25D0}", app.theme.highlight), // ◐
        (stats.blocked, "\u{2298}", app.theme.red),      // ⊘
        (stats.todo, "\u{25CB}", app.theme.text),        // ○
        (stats.parked, "\u{25C7}", app.theme.yellow),    // ◇
        (stats.done, "\u{2713}", app.theme.dim),         // ✓
    ];

    for (count, symbol, color) in items {
        if count > 0 {
            spans.push(Span::styled(
                format!("{}{} ", count, symbol),
                Style::default().fg(color).bg(bg),
            ));
        }
    }
    spans
}
