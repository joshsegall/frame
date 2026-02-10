use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::tui::app::{App, EditTarget, Mode};
use crate::tui::input::{multiline_selection_range, selection_cols_for_line};
use crate::tui::wrap;
use crate::util::unicode;

use super::detail_view::wrap_styled_spans;
use super::push_highlighted_spans;

/// Maximum visible lines for the note editor / view-mode body
const MAX_NOTE_LINES: usize = 8;

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

    // Determine if we're editing a note for the cursor item
    let editing_note_for =
        if app.mode == Mode::Edit && app.edit_target.is_none() && app.inbox_note_index.is_some() {
            app.inbox_note_index
        } else {
            None
        };

    // Snapshot item data to avoid borrow conflict with mutable app borrow in editor
    let items_snapshot: Vec<_> = inbox
        .items
        .iter()
        .map(|item| (item.title.clone(), item.tags.clone(), item.body.clone()))
        .collect();

    // Build all display lines with their item indices
    let mut display_lines: Vec<(Option<usize>, Line)> = Vec::new();

    // Track the editor cursor line (for scroll adjustment)
    let mut editor_active_line: Option<usize> = None;

    for (i, (title, tags, body)) in items_snapshot.iter().enumerate() {
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
            spans.push(Span::styled(" ", Style::default().bg(app.theme.background)));
        }

        let num_style = Style::default().fg(app.theme.dim).bg(bg);
        spans.push(Span::styled(format!("{:>2}  ", i + 1), num_style));

        // Check if we're editing this item's title or tags
        let editing_title = is_cursor
            && app.mode == Mode::Edit
            && matches!(
                &app.edit_target,
                Some(EditTarget::NewInboxItem { .. }) | Some(EditTarget::ExistingInboxTitle { .. })
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
            let prefix_width: usize = spans
                .iter()
                .map(|s| unicode::display_width(&s.content))
                .sum();
            let tag_width: usize = tags.iter().map(|t| t.len() + 2).sum::<usize>()
                + if tags.is_empty() { 0 } else { 2 };
            let available = (area.width as usize).saturating_sub(prefix_width + tag_width + 1);
            let display_title = super::truncate_with_ellipsis(title, available);
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
        } else if !tags.is_empty() {
            spans.push(Span::styled("  ", Style::default().bg(bg)));
            let tag_hl_style = Style::default()
                .fg(app.theme.search_match_fg)
                .bg(app.theme.search_match_bg)
                .add_modifier(Modifier::BOLD);
            for (j, tag) in tags.iter().enumerate() {
                if j > 0 {
                    spans.push(Span::styled(" ", Style::default().bg(bg)));
                }
                let tag_color = app.theme.tag_color(tag);
                let tag_style = Style::default().fg(tag_color).bg(bg);
                push_highlighted_spans(
                    &mut spans,
                    &format!("#{}", tag),
                    tag_style,
                    tag_hl_style,
                    search_re.as_ref(),
                );
            }
        }

        // Pad cursor line
        if is_cursor {
            let content_width: usize = spans
                .iter()
                .map(|s| unicode::display_width(&s.content))
                .sum();
            let w = area.width as usize;
            if content_width < w {
                spans.push(Span::styled(
                    " ".repeat(w - content_width),
                    Style::default().bg(bg),
                ));
            }
        }

        display_lines.push((Some(i), Line::from(spans)));

        // Inline note editor or body text
        if editing_note_for == Some(i) {
            // Render the multi-line note editor inline
            render_inline_note_editor(app, &mut display_lines, &mut editor_active_line, i, area);
        } else if let Some(body) = body {
            // View-mode body text
            render_body_view_mode(app, &mut display_lines, body, i, search_re.as_ref(), area);
        }
    }

    // Find the display line index of the cursor item (for autocomplete anchor + scroll)
    let mut cursor_display_line: Option<usize> = None;
    for (dl_idx, (item_idx, _)) in display_lines.iter().enumerate() {
        if *item_idx == Some(cursor) && cursor_display_line.is_none() {
            cursor_display_line = Some(dl_idx);
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

    // When editing a note, ensure the editor's active line (cursor line) is visible
    if let Some(active) = editor_active_line {
        if active >= scroll + visible_height {
            scroll = active.saturating_sub(visible_height - 1);
        }
        // Also ensure the header (cursor item line) stays visible
        if let Some(cdl) = cursor_display_line
            && cdl < scroll
        {
            // Header scrolled off top — pull scroll back so header is row 0
            scroll = cdl;
        }
    }

    app.inbox_scroll = scroll;

    // Set autocomplete anchor if editing tags or in triage mode
    let needs_anchor = (app.mode == Mode::Edit
        && matches!(&app.edit_target, Some(EditTarget::ExistingInboxTags { .. })))
        || app.mode == Mode::Triage;
    if needs_anchor && let Some(dl) = cursor_display_line {
        let screen_line = dl.saturating_sub(scroll);
        let screen_y = area.y + screen_line as u16;
        // Anchor x: after the number prefix (col 5 roughly)
        let screen_x = area.x + 5;
        app.autocomplete_anchor = Some((screen_x, screen_y));
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

/// Render body text in view mode with optional line numbers (>= 4 lines)
/// and capped at MAX_NOTE_LINES with a "N more lines" indicator.
fn render_body_view_mode(
    app: &App,
    display_lines: &mut Vec<(Option<usize>, Line)>,
    body: &str,
    item_index: usize,
    search_re: Option<&regex::Regex>,
    area: Rect,
) {
    let body_lines: Vec<&str> = body.lines().collect();
    let line_count = body_lines.len();
    let truncated = line_count > MAX_NOTE_LINES;
    let visible_count = if truncated {
        MAX_NOTE_LINES
    } else {
        line_count
    };

    let body_style = Style::default().fg(app.theme.text).bg(app.theme.background);
    let body_hl_style = Style::default()
        .fg(app.theme.search_match_fg)
        .bg(app.theme.search_match_bg)
        .add_modifier(Modifier::BOLD);
    let dim_style = Style::default().fg(app.theme.dim).bg(app.theme.background);
    let indent = "      ";
    let indent_len = indent.len();
    let content_width = (area.width as usize).saturating_sub(indent_len);

    let mut visual_lines_rendered = 0;
    for body_line in body_lines.iter().take(visible_count) {
        let mut content_spans: Vec<Span> = Vec::new();
        push_highlighted_spans(
            &mut content_spans,
            body_line,
            body_style,
            body_hl_style,
            search_re,
        );

        let wrapped = wrap_styled_spans(content_spans, content_width, 0, app.theme.background);
        for wrapped_line in wrapped {
            let indent_span = Span::styled(
                indent.to_string(),
                Style::default().bg(app.theme.background),
            );
            let mut line_spans = vec![indent_span];
            line_spans.extend(wrapped_line.spans);
            display_lines.push((Some(item_index), Line::from(line_spans)));
            visual_lines_rendered += 1;
        }
    }
    let _ = visual_lines_rendered;

    if truncated {
        let remaining = line_count - MAX_NOTE_LINES;
        let indicator = format!(
            "{}… {} more line{}",
            indent,
            remaining,
            if remaining == 1 { "" } else { "s" }
        );
        display_lines.push((
            Some(item_index),
            Line::from(Span::styled(indicator, dim_style)),
        ));
    }
}

/// Render the inline multi-line note editor below the selected inbox item.
fn render_inline_note_editor(
    app: &mut App,
    display_lines: &mut Vec<(Option<usize>, Line)>,
    editor_active_line: &mut Option<usize>,
    item_index: usize,
    area: Rect,
) {
    let ds = match &app.detail_state {
        Some(ds) => ds,
        None => return,
    };

    let edit_lines: Vec<&str> = ds.edit_buffer.split('\n').collect();
    let total_lines = edit_lines.len();
    let visible_lines = total_lines.clamp(1, MAX_NOTE_LINES);

    // Auto-adjust editor scroll to keep cursor line visible
    let cursor_line = ds.edit_cursor_line;
    let mut editor_scroll = app.inbox_note_editor_scroll;
    // Clamp scroll so the editor always fills its visible_lines window
    let max_scroll = total_lines.saturating_sub(visible_lines);
    editor_scroll = editor_scroll.min(max_scroll);
    if cursor_line < editor_scroll {
        editor_scroll = cursor_line;
    } else if cursor_line >= editor_scroll + visible_lines {
        editor_scroll = cursor_line.saturating_sub(visible_lines - 1);
    }
    app.inbox_note_editor_scroll = editor_scroll;

    // Compute gutter width from total line count
    let gutter_width = wrap::gutter_width(total_lines);
    let num_display_width = gutter_width - 1;

    // Available width for note content (gutter eats into base indent)
    const BASE_INDENT: usize = 6;
    let indent_width = BASE_INDENT.saturating_sub(gutter_width);
    let note_available = (area.width as usize).saturating_sub(BASE_INDENT);

    app.last_edit_available_width = note_available as u16;
    let cursor_col = ds.edit_cursor_col;
    let bright_style = Style::default()
        .fg(app.theme.text_bright)
        .bg(app.theme.background);
    let text_style = Style::default().fg(app.theme.text).bg(app.theme.background);
    let cursor_style = Style::default()
        .fg(app.theme.background)
        .bg(app.theme.text_bright);
    let selection_style = Style::default()
        .fg(app.theme.text_bright)
        .bg(app.theme.blue);
    let dim_arrow_style = Style::default().fg(app.theme.dim).bg(app.theme.background);

    // Compute selection range
    let sel_range = multiline_selection_range(ds);

    let mut h_scroll = ds.note_h_scroll;

    if app.note_wrap && note_available > 0 {
        // --- Wrap-aware rendering ---
        let visual_lines = wrap::wrap_lines(&edit_lines, note_available);
        let total_visual = visual_lines.len();
        let visible_visual = total_visual.clamp(1, MAX_NOTE_LINES);

        let cursor_vrow = wrap::logical_to_visual_row(&visual_lines, cursor_line, cursor_col);

        // Scroll to keep cursor visual row visible
        let max_vscroll = total_visual.saturating_sub(visible_visual);
        editor_scroll = editor_scroll.min(max_vscroll);
        if cursor_vrow < editor_scroll {
            editor_scroll = cursor_vrow;
        } else if cursor_vrow >= editor_scroll + visible_visual {
            editor_scroll = cursor_vrow.saturating_sub(visible_visual - 1);
        }
        app.inbox_note_editor_scroll = editor_scroll;

        for view_row in 0..visible_visual {
            let vrow_idx = editor_scroll + view_row;
            if vrow_idx >= total_visual {
                break;
            }
            let vl = &visual_lines[vrow_idx];
            let line_text = edit_lines.get(vl.logical_line).copied().unwrap_or("");
            let slice = &line_text[vl.byte_start..vl.byte_end];
            let graphemes: Vec<(usize, &str)> =
                unicode_segmentation::UnicodeSegmentation::grapheme_indices(slice, true).collect();

            let has_cursor = vrow_idx == cursor_vrow;
            if has_cursor {
                *editor_active_line = Some(display_lines.len());
            }

            let mut spans: Vec<Span> = Vec::new();
            if indent_width > 0 {
                spans.push(Span::styled(
                    " ".repeat(indent_width),
                    Style::default().bg(app.theme.background),
                ));
            }

            // Gutter
            if vl.is_first {
                let num_str = format!(
                    "{:>width$} ",
                    vl.logical_line + 1,
                    width = num_display_width,
                );
                spans.push(Span::styled(num_str, text_style));
            } else {
                spans.push(Span::styled(
                    " ".repeat(gutter_width),
                    Style::default().bg(app.theme.background),
                ));
            }

            let vl_sel = sel_range
                .and_then(|(s, e)| selection_cols_for_line(&ds.edit_buffer, s, e, vl.logical_line));

            let cursor_byte_in_row = if has_cursor {
                Some(cursor_col.saturating_sub(vl.char_start))
            } else {
                None
            };

            if let Some((sc, ec)) = vl_sel {
                for &(gi, g) in &graphemes {
                    let abs_byte = vl.byte_start + gi;
                    let s = if abs_byte >= sc && abs_byte < ec {
                        selection_style
                    } else {
                        bright_style
                    };
                    if cursor_byte_in_row == Some(gi) {
                        spans.push(Span::styled(g.to_string(), cursor_style));
                    } else {
                        spans.push(Span::styled(g.to_string(), s));
                    }
                }
                if sc == ec && graphemes.is_empty() && !has_cursor {
                    spans.push(Span::styled(" ", selection_style));
                }
                if has_cursor && cursor_col >= vl.char_end {
                    spans.push(Span::styled(" ", cursor_style));
                }
            } else if has_cursor {
                let byte_in_row = cursor_col.min(vl.char_end).saturating_sub(vl.char_start);
                for &(gi, g) in &graphemes {
                    if gi == byte_in_row {
                        spans.push(Span::styled(g.to_string(), cursor_style));
                    } else {
                        spans.push(Span::styled(g.to_string(), bright_style));
                    }
                }
                if byte_in_row >= slice.len() {
                    spans.push(Span::styled(" ", cursor_style));
                }
            } else if !slice.is_empty() {
                spans.push(Span::styled(slice.to_string(), bright_style));
            }

            display_lines.push((Some(item_index), Line::from(spans)));
        }

        // Scroll indicator
        if total_visual > visible_visual {
            let dim_style = Style::default().fg(app.theme.dim).bg(app.theme.background);
            let indicator = format!(
                "{}[{}/{}]",
                " ".repeat(BASE_INDENT),
                editor_scroll + visible_visual,
                total_visual
            );
            display_lines.push((
                Some(item_index),
                Line::from(Span::styled(indicator, dim_style)),
            ));
        }
    } else {
        // --- Original horizontal-scroll rendering ---
        if note_available > 0 {
            let margin = 10.min(note_available / 3);
            let cursor_line_len = edit_lines.get(cursor_line).map_or(0, |l| l.len());
            let content_end = if cursor_col >= cursor_line_len {
                cursor_line_len + 1
            } else {
                cursor_line_len
            };
            if cursor_col >= h_scroll + note_available.saturating_sub(margin) {
                h_scroll = cursor_col.saturating_sub(note_available.saturating_sub(margin + 1));
            }
            h_scroll = h_scroll.min(content_end.saturating_sub(note_available.saturating_sub(1)));
            if cursor_col < h_scroll + margin {
                h_scroll = cursor_col.saturating_sub(margin);
            }
        }

        for view_row in 0..visible_lines {
            let line_idx = editor_scroll + view_row;
            if line_idx >= total_lines {
                break;
            }
            let edit_line = edit_lines[line_idx];
            let has_cursor = line_idx == cursor_line;

            if has_cursor {
                *editor_active_line = Some(display_lines.len());
            }

            let mut spans: Vec<Span> = Vec::new();
            if indent_width > 0 {
                spans.push(Span::styled(
                    " ".repeat(indent_width),
                    Style::default().bg(app.theme.background),
                ));
            }

            let graphemes: Vec<(usize, &str)> =
                unicode_segmentation::UnicodeSegmentation::grapheme_indices(edit_line, true)
                    .collect();
            let total_graphemes = graphemes.len();
            let clipped_left = h_scroll > 0 && total_graphemes > 0;
            let left_indicator = if clipped_left { 1 } else { 0 };
            let avail_after_left = note_available.saturating_sub(left_indicator);
            let clipped_right = h_scroll + avail_after_left < total_graphemes;
            let right_indicator = if clipped_right { 1 } else { 0 };
            let view_count = avail_after_left.saturating_sub(right_indicator);
            let view_start = h_scroll.min(total_graphemes);
            let view_end = (view_start + view_count).min(total_graphemes);

            let line_num_str = format!("{:>width$}", line_idx + 1, width = num_display_width);
            if clipped_left {
                spans.push(Span::styled(line_num_str, text_style));
                spans.push(Span::styled("\u{25C2}", dim_arrow_style)); // ◂
            } else {
                spans.push(Span::styled(format!("{} ", line_num_str), text_style));
            }

            let line_sel = sel_range
                .and_then(|(s, e)| selection_cols_for_line(&ds.edit_buffer, s, e, line_idx));

            // Convert cursor byte offset to grapheme index
            let cursor_gi = graphemes
                .iter()
                .position(|&(bo, _)| bo >= cursor_col)
                .unwrap_or(total_graphemes);

            if let Some((sc, ec)) = line_sel {
                for (gi, &(bo, g)) in graphemes
                    .iter()
                    .enumerate()
                    .skip(view_start)
                    .take(view_end - view_start)
                {
                    let s = if bo >= sc && bo < ec {
                        selection_style
                    } else {
                        bright_style
                    };
                    if has_cursor && gi == cursor_gi {
                        spans.push(Span::styled(g.to_string(), cursor_style));
                    } else {
                        spans.push(Span::styled(g.to_string(), s));
                    }
                }
                if sc == ec && total_graphemes == 0 && !has_cursor {
                    spans.push(Span::styled(" ", selection_style));
                }
                if has_cursor && cursor_gi >= total_graphemes && cursor_gi >= view_start {
                    spans.push(Span::styled(" ", cursor_style));
                }
            } else if has_cursor {
                let col_gi = cursor_gi.min(total_graphemes);
                for (gi, &(_bo, g)) in graphemes
                    .iter()
                    .enumerate()
                    .skip(view_start)
                    .take(view_end - view_start)
                {
                    if gi == col_gi {
                        spans.push(Span::styled(g.to_string(), cursor_style));
                    } else {
                        spans.push(Span::styled(g.to_string(), bright_style));
                    }
                }
                if col_gi >= total_graphemes && col_gi >= view_start {
                    spans.push(Span::styled(" ", cursor_style));
                }
            } else if view_start < view_end {
                let slice: String = graphemes[view_start..view_end]
                    .iter()
                    .map(|g| g.1)
                    .collect();
                spans.push(Span::styled(slice, bright_style));
            }

            if clipped_right {
                spans.push(Span::styled("\u{25B8}", dim_arrow_style)); // ▸
            }

            display_lines.push((Some(item_index), Line::from(spans)));
        }

        // Scroll indicator
        if total_lines > visible_lines {
            let dim_style = Style::default().fg(app.theme.dim).bg(app.theme.background);
            let indicator = format!(
                "{}[{}/{}]",
                " ".repeat(BASE_INDENT),
                editor_scroll + visible_lines,
                total_lines
            );
            display_lines.push((
                Some(item_index),
                Line::from(Span::styled(indicator, dim_style)),
            ));
        }
    }

    // Write back adjusted h_scroll
    if let Some(ds_mut) = app.detail_state.as_mut() {
        ds_mut.note_h_scroll = h_scroll;
    }
}
