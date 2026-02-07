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
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use regex::Regex;

use super::app::{App, TriageStep, View};

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
        View::Inbox => {
            inbox_view::render_inbox_view(frame, app, chunks[1]);
        }
        View::Recent => {
            recent_view::render_recent_view(frame, app, chunks[1]);
        }
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

    // Triage position popup (rendered on top of content)
    render_triage_position_popup(frame, app);

    // Status row
    status_row::render_status_row(frame, app, chunks[2]);
}

/// Render the triage position-selection popup, if in the SelectPosition step.
fn render_triage_position_popup(frame: &mut Frame, app: &App) {
    let ts = match &app.triage_state {
        Some(ts) => ts,
        None => return,
    };
    let track_id = match &ts.step {
        TriageStep::SelectPosition { track_id } => track_id,
        _ => return,
    };
    let (anchor_x, anchor_y) = match ts.popup_anchor {
        Some(pos) => pos,
        None => return,
    };

    let bg = app.theme.background;
    let text_color = app.theme.text;
    let bright = app.theme.text_bright;
    let dim = app.theme.dim;
    let highlight = app.theme.highlight;

    let track_name = app.track_name(track_id);
    let title = format!(" {} ", track_name);

    let key_style = Style::default()
        .fg(highlight)
        .bg(bg)
        .add_modifier(Modifier::BOLD);
    let desc_style = Style::default().fg(text_color).bg(bg);
    let hint_style = Style::default().fg(dim).bg(bg);

    let entries: Vec<Line> = vec![
        Line::from(vec![
            Span::styled("  t   ", key_style),
            Span::styled("Top of backlog  ", desc_style),
        ]),
        Line::from(vec![
            Span::styled("  b   ", key_style),
            Span::styled("Bottom (default)", desc_style),
        ]),
        Line::from(vec![
            Span::styled("  Esc ", hint_style),
            Span::styled("Cancel          ", hint_style),
        ]),
    ];

    let popup_w: u16 = 24;
    let popup_h: u16 = entries.len() as u16 + 2; // +2 for borders

    let term_area = frame.area();

    // Position: same top-left as the autocomplete was
    let cursor_bottom = anchor_y + 1;
    let y = if cursor_bottom + popup_h <= term_area.height {
        cursor_bottom
    } else {
        anchor_y.saturating_sub(popup_h)
    };
    let text_inset: u16 = 4;
    let x = anchor_x.saturating_sub(text_inset).min(term_area.width.saturating_sub(popup_w));

    let popup_area = Rect::new(x, y, popup_w, popup_h);

    frame.render_widget(Clear, popup_area);

    let block = Block::default()
        .title(Span::styled(
            title,
            Style::default()
                .fg(bright)
                .bg(bg)
                .add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(dim).bg(bg))
        .style(Style::default().bg(bg));

    let paragraph = Paragraph::new(entries)
        .block(block)
        .style(Style::default().bg(bg));
    frame.render_widget(paragraph, popup_area);
}

/// Truncate a string to fit within `max_chars`, adding "…" if truncated.
pub(super) fn truncate_with_ellipsis(s: &str, max_chars: usize) -> String {
    if max_chars == 0 {
        return String::new();
    }
    let char_count = s.chars().count();
    if char_count <= max_chars {
        s.to_string()
    } else if max_chars <= 1 {
        "\u{2026}".to_string()
    } else {
        let truncated: String = s.chars().take(max_chars - 1).collect();
        format!("{}\u{2026}", truncated)
    }
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
