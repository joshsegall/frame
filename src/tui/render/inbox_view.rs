use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::tui::app::{App, EditTarget, Mode};

use super::push_highlighted_spans;

/// Render the inbox view
pub fn render_inbox_view(frame: &mut Frame, app: &mut App, area: Rect) {
    let inbox = match &app.project.inbox {
        Some(inbox) => inbox,
        None => {
            let empty = Paragraph::new(" No inbox")
                .style(Style::default().fg(app.theme.dim).bg(app.theme.background));
            frame.render_widget(empty, area);
            return;
        }
    };

    if inbox.items.is_empty() {
        let empty = Paragraph::new(" Inbox is empty")
            .style(Style::default().fg(app.theme.dim).bg(app.theme.background));
        frame.render_widget(empty, area);
        return;
    }

    // Clamp cursor
    let item_count = inbox.items.len();
    let cursor = app.inbox_cursor.min(item_count.saturating_sub(1));
    app.inbox_cursor = cursor;
    let visible_height = area.height as usize;

    let search_re = app.active_search_re();

    // Build all display lines with their item indices
    let mut display_lines: Vec<(Option<usize>, Line)> = Vec::new();

    for (i, item) in inbox.items.iter().enumerate() {
        let is_cursor = i == cursor;
        let bg = if is_cursor {
            app.theme.selection_bg
        } else {
            app.theme.background
        };

        // Blank line before each item (except first)
        if i > 0 {
            display_lines.push((None, Line::from("")));
        }

        // Number + title + tags
        let mut spans: Vec<Span> = Vec::new();

        // Column 0 reservation
        if is_cursor {
            spans.push(Span::styled(
                "\u{258E}",
                Style::default()
                    .fg(app.theme.selection_border)
                    .bg(app.theme.selection_bg),
            ));
        } else {
            spans.push(Span::styled(
                " ",
                Style::default().bg(app.theme.background),
            ));
        }

        let num_style = Style::default().fg(app.theme.dim).bg(bg);
        spans.push(Span::styled(format!("{:>2}  ", i + 1), num_style));

        // Check if we're editing this item's title or tags
        let editing_title = is_cursor
            && app.mode == Mode::Edit
            && matches!(
                &app.edit_target,
                Some(EditTarget::NewInboxItem { .. })
                    | Some(EditTarget::ExistingInboxTitle { .. })
            );
        let editing_tags = is_cursor
            && app.mode == Mode::Edit
            && matches!(&app.edit_target, Some(EditTarget::ExistingInboxTags { .. }));

        if editing_title {
            // Show edit buffer with cursor
            let cursor_pos = app.edit_cursor.min(app.edit_buffer.len());
            let before = &app.edit_buffer[..cursor_pos];
            let after = &app.edit_buffer[cursor_pos..];
            let edit_style = Style::default()
                .fg(app.theme.text_bright)
                .bg(bg)
                .add_modifier(Modifier::BOLD);
            spans.push(Span::styled(before.to_string(), edit_style));
            spans.push(Span::styled(
                "\u{258C}",
                Style::default().fg(app.theme.highlight).bg(bg),
            ));
            if !after.is_empty() {
                spans.push(Span::styled(after.to_string(), edit_style));
            }
        } else {
            let title_style = if is_cursor {
                Style::default()
                    .fg(app.theme.text_bright)
                    .bg(bg)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(app.theme.text_bright).bg(bg)
            };
            let hl_style = Style::default()
                .fg(app.theme.search_match_fg)
                .bg(app.theme.search_match_bg)
                .add_modifier(Modifier::BOLD);
            // Truncate title at available width
            let prefix_width: usize = spans.iter().map(|s| s.content.chars().count()).sum();
            let tag_width: usize = item.tags.iter().map(|t| t.len() + 2).sum::<usize>() + if item.tags.is_empty() { 0 } else { 2 };
            let available = (area.width as usize).saturating_sub(prefix_width + tag_width + 1);
            let display_title = super::truncate_with_ellipsis(&item.title, available);
            push_highlighted_spans(
                &mut spans,
                &display_title,
                title_style,
                hl_style,
                search_re.as_ref(),
            );
        }

        // Tags
        if editing_tags {
            // Show tag edit buffer with cursor
            spans.push(Span::styled("  ", Style::default().bg(bg)));
            let cursor_pos = app.edit_cursor.min(app.edit_buffer.len());
            let before = &app.edit_buffer[..cursor_pos];
            let after = &app.edit_buffer[cursor_pos..];
            let edit_style = Style::default().fg(app.theme.highlight).bg(bg);
            spans.push(Span::styled(before.to_string(), edit_style));
            spans.push(Span::styled(
                "\u{258C}",
                Style::default().fg(app.theme.highlight).bg(bg),
            ));
            if !after.is_empty() {
                spans.push(Span::styled(after.to_string(), edit_style));
            }
        } else if !item.tags.is_empty() {
            spans.push(Span::styled("  ", Style::default().bg(bg)));
            for (j, tag) in item.tags.iter().enumerate() {
                if j > 0 {
                    spans.push(Span::styled(" ", Style::default().bg(bg)));
                }
                let tag_color = app.theme.tag_color(tag);
                spans.push(Span::styled(
                    format!("#{}", tag),
                    Style::default().fg(tag_color).bg(bg),
                ));
            }
        }

        // Pad cursor line
        if is_cursor {
            let content_width: usize = spans.iter().map(|s| s.content.chars().count()).sum();
            let w = area.width as usize;
            if content_width < w {
                spans.push(Span::styled(
                    " ".repeat(w - content_width),
                    Style::default().bg(bg),
                ));
            }
        }

        display_lines.push((Some(i), Line::from(spans)));

        // Body text (dimmed, indented)
        if let Some(body) = &item.body {
            for body_line in body.lines() {
                let body_spans = vec![
                    Span::styled("      ", Style::default().bg(app.theme.background)),
                    Span::styled(
                        body_line.to_string(),
                        Style::default().fg(app.theme.text).bg(app.theme.background),
                    ),
                ];
                display_lines.push((Some(i), Line::from(body_spans)));
            }
        }
    }

    // Find the display line index of the cursor item (for autocomplete anchor + scroll)
    let mut cursor_display_line: Option<usize> = None;
    for (dl_idx, (item_idx, _)) in display_lines.iter().enumerate() {
        if *item_idx == Some(cursor) {
            if cursor_display_line.is_none() {
                cursor_display_line = Some(dl_idx);
            }
        }
    }

    // Auto-adjust scroll to keep cursor visible
    let mut scroll = app.inbox_scroll;
    if let Some(cdl) = cursor_display_line {
        if cdl < scroll {
            scroll = cdl;
        } else if cdl >= scroll + visible_height {
            scroll = cdl.saturating_sub(visible_height - 1);
        }
    }
    app.inbox_scroll = scroll;

    // Set autocomplete anchor if editing tags or in triage mode
    let needs_anchor = (app.mode == Mode::Edit
        && matches!(&app.edit_target, Some(EditTarget::ExistingInboxTags { .. })))
        || app.mode == Mode::Triage;
    if needs_anchor {
        if let Some(dl) = cursor_display_line {
            let screen_line = dl.saturating_sub(scroll);
            let screen_y = area.y + screen_line as u16;
            // Anchor x: after the number prefix (col 5 roughly)
            let screen_x = area.x + 5;
            app.autocomplete_anchor = Some((screen_x, screen_y));
        }
    }

    // Apply scroll and collect visible lines
    let lines: Vec<Line> = display_lines
        .into_iter()
        .skip(scroll)
        .take(visible_height)
        .map(|(_, line)| line)
        .collect();

    let paragraph = Paragraph::new(lines).style(Style::default().bg(app.theme.background));
    frame.render_widget(paragraph, area);
}
