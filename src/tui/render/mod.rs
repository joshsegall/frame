pub mod autocomplete;
pub mod command_palette;
pub mod conflict_popup;
pub mod dep_popup;
pub mod detail_view;
pub mod help_overlay;
mod helpers;
pub mod inbox_view;
pub mod prefix_confirm;
pub mod project_picker;
pub mod recent_view;
pub mod recovery_overlay;
pub mod results_overlay;
pub mod status_row;
pub mod tab_bar;
pub mod tag_color_popup;
pub mod track_view;
pub mod tracks_view;

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use regex::Regex;

use crate::util::unicode;

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
        View::Tracks => {
            tracks_view::render_tracks_view(frame, app, chunks[1]);
        }
        View::Inbox => {
            inbox_view::render_inbox_view(frame, app, chunks[1]);
        }
        View::Recent => {
            recent_view::render_recent_view(frame, app, chunks[1]);
        }
    }

    // Dep popup overlay (rendered on top of content)
    if app.dep_popup.is_some() {
        dep_popup::render_dep_popup(frame, app, chunks[1]);
    }

    // Tag color editor popup overlay
    if app.tag_color_popup.is_some() {
        tag_color_popup::render_tag_color_popup(frame, app, chunks[1]);
    }

    // Prefix rename confirmation popup
    if app.prefix_rename.as_ref().is_some_and(|pr| pr.confirming) {
        prefix_confirm::render_prefix_confirm(frame, app, chunks[1]);
    }

    // Project picker overlay
    if app.project_picker.is_some() {
        project_picker::render_project_picker(frame, app, chunks[1]);
    }

    // Help overlay (rendered on top of everything)
    if app.show_help {
        help_overlay::render_help_overlay(frame, app, frame.area());
    }

    // Command palette overlay (rendered on top of everything)
    if app.command_palette.is_some() {
        command_palette::render_command_palette(frame, app, chunks[1]);
    }

    // Recovery log overlay (rendered on top of everything)
    if app.show_recovery_log {
        recovery_overlay::render_recovery_overlay(frame, app, frame.area());
    }

    // Results overlay (rendered on top of everything)
    if app.show_results_overlay {
        results_overlay::render_results_overlay(frame, app, frame.area());
    }

    // Conflict popup (rendered on top of everything)
    if app.conflict_text.is_some() {
        conflict_popup::render_conflict_popup(frame, app, frame.area());
    }

    // For JumpTo: anchor autocomplete to the status row (edit buffer lives there)
    if app.mode == super::app::Mode::Edit
        && matches!(app.edit_target, Some(super::app::EditTarget::JumpTo))
    {
        // " jump: " = 7 chars, then edit_buffer text, then cursor
        let x = chunks[2].x + 7 + unicode::display_width(&app.edit_buffer) as u16;
        let y = chunks[2].y;
        app.autocomplete_anchor = Some((x, y));
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

    let cursor = ts.position_cursor;

    let selected_style = Style::default()
        .fg(highlight)
        .bg(bg)
        .add_modifier(Modifier::BOLD);
    let normal_style = Style::default().fg(text_color).bg(bg);
    let hint_style = Style::default().fg(dim).bg(bg);

    let options: &[(&str, &str)] = &[
        ("Top of backlog  ", "t"),
        ("Bottom (default)", "b"),
        ("Cancel          ", ""),
    ];

    let entries: Vec<Line> = options
        .iter()
        .enumerate()
        .map(|(i, (label, key))| {
            let is_selected = i as u8 == cursor;
            let (indicator, style) = if is_selected {
                ("▶ ", selected_style)
            } else {
                ("  ", normal_style)
            };
            let mut spans = vec![Span::styled(indicator, style), Span::styled(*label, style)];
            if !key.is_empty() {
                spans.push(Span::styled(format!(" {}", key), hint_style));
            }
            Line::from(spans)
        })
        .collect();

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
    let x = anchor_x
        .saturating_sub(text_inset)
        .min(term_area.width.saturating_sub(popup_w));

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

#[cfg(test)]
pub(crate) mod test_helpers;

/// Truncate a string to fit within `max_cells` terminal cells, adding "…" if truncated.
pub(super) fn truncate_with_ellipsis(s: &str, max_cells: usize) -> String {
    unicode::truncate_to_width(s, max_cells)
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

#[cfg(test)]
mod tests {
    use super::*;
    use insta::assert_snapshot;
    use test_helpers::*;

    #[test]
    fn full_render_track_view() {
        let mut app = app_with_track(SIMPLE_TRACK_MD);
        let output = render_to_string(TERM_W, TERM_H, |frame, area| {
            render(frame, &mut app);
            let _ = area; // full-frame render ignores passed area
        });
        assert_snapshot!(output);
    }

    #[test]
    fn full_render_inbox_view() {
        let mut app = app_with_inbox(INBOX_MD);
        app.view = View::Inbox;
        let output = render_to_string(TERM_W, TERM_H, |frame, _area| {
            render(frame, &mut app);
        });
        assert_snapshot!(output);
    }

    #[test]
    fn truncate_with_ellipsis_short() {
        assert_eq!(truncate_with_ellipsis("hello", 10), "hello");
    }

    #[test]
    fn truncate_with_ellipsis_exact() {
        assert_eq!(truncate_with_ellipsis("hello", 5), "hello");
    }

    #[test]
    fn truncate_with_ellipsis_long() {
        let result = truncate_with_ellipsis("hello world this is long", 10);
        assert!(result.len() <= 13); // 10 cells max, ellipsis is multi-byte
        assert!(result.ends_with('…'));
    }

    #[test]
    fn push_highlighted_spans_no_regex() {
        let mut spans = Vec::new();
        push_highlighted_spans(
            &mut spans,
            "hello world",
            Style::default(),
            Style::default(),
            None,
        );
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].content.as_ref(), "hello world");
    }

    #[test]
    fn push_highlighted_spans_with_match() {
        let re = Regex::new("world").unwrap();
        let mut spans = Vec::new();
        push_highlighted_spans(
            &mut spans,
            "hello world!",
            Style::default(),
            Style::default().add_modifier(Modifier::BOLD),
            Some(&re),
        );
        assert_eq!(spans.len(), 3);
        assert_eq!(spans[0].content.as_ref(), "hello ");
        assert_eq!(spans[1].content.as_ref(), "world");
        assert_eq!(spans[2].content.as_ref(), "!");
    }
}
