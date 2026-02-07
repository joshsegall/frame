pub mod autocomplete;
pub mod conflict_popup;
pub mod detail_view;
pub mod help_overlay;
pub mod inbox_view;
pub mod recent_view;
pub mod status_row;
pub mod tab_bar;
pub mod track_view;
pub mod tracks_view;

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::Style;
use ratatui::text::Span;
use ratatui::widgets::Block;
use regex::Regex;

use super::app::{App, View};

/// Main render function — dispatches to sub-renderers
pub fn render(frame: &mut Frame, app: &mut App) {
    let area = frame.area();

    // Background fill
    let bg_style = Style::default().bg(app.theme.background);
    frame.render_widget(Block::default().style(bg_style), area);

    // Layout: tab bar (2 rows) | content | status row (1 row)
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2), // tab bar + separator
            Constraint::Min(1),    // content area
            Constraint::Length(1), // status row
        ])
        .split(area);

    // Render tab bar
    tab_bar::render_tab_bar(frame, app, chunks[0]);

    // Clear autocomplete anchor before rendering content (will be set by track/detail view if editing)
    app.autocomplete_anchor = None;

    // Render content area (clone view to avoid borrow conflict)
    let view = app.view.clone();
    match &view {
        View::Track(_) => track_view::render_track_view(frame, app, chunks[1]),
        View::Detail { .. } => detail_view::render_detail_view(frame, app, chunks[1]),
        View::Tracks => tracks_view::render_tracks_view(frame, app, chunks[1]),
        View::Inbox => inbox_view::render_inbox_view(frame, app, chunks[1]),
        View::Recent => recent_view::render_recent_view(frame, app, chunks[1]),
    }

    // Help overlay (rendered on top of everything)
    if app.show_help {
        help_overlay::render_help_overlay(frame, app, frame.area());
    }

    // Conflict popup (rendered on top of everything)
    if app.conflict_text.is_some() {
        conflict_popup::render_conflict_popup(frame, app, frame.area());
    }

    // Autocomplete dropdown (rendered on top of content)
    if app.autocomplete.is_some() {
        autocomplete::render_autocomplete(frame, app, chunks[1]);
    }

    // Status row
    status_row::render_status_row(frame, app, chunks[2]);
}

/// Push spans for text with regex match highlighting. If no regex or no matches,
/// pushes a single span with `base_style`. Otherwise splits text at match boundaries.
pub(super) fn push_highlighted_spans<'a>(
    spans: &mut Vec<Span<'a>>,
    text: &str,
    base_style: Style,
    highlight_style: Style,
    search_re: Option<&Regex>,
) {
    let re = match search_re {
        Some(r) => r,
        None => {
            spans.push(Span::styled(text.to_string(), base_style));
            return;
        }
    };

    let mut last_end = 0;
    let mut has_match = false;
    for m in re.find_iter(text) {
        has_match = true;
        if m.start() > last_end {
            spans.push(Span::styled(
                text[last_end..m.start()].to_string(),
                base_style,
            ));
        }
        spans.push(Span::styled(
            text[m.start()..m.end()].to_string(),
            highlight_style,
        ));
        last_end = m.end();
    }
    if !has_match {
        spans.push(Span::styled(text.to_string(), base_style));
    } else if last_end < text.len() {
        spans.push(Span::styled(text[last_end..].to_string(), base_style));
    }
}
