use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use regex::Regex;

use crate::model::{Metadata, SectionKind, Task, TaskState};
use crate::tui::app::{App, EditTarget, FlatItem, Mode, MoveState};
use crate::tui::wrap;
use crate::util::unicode;

use super::detail_view::{UNDO_FLASH_COLORS, state_flash_colors, wrap_styled_spans};
use super::helpers::{abbreviated_id, spans_width, state_symbol};
use super::push_highlighted_spans;

/// Maximum visible lines for wrap-aware title editing
const MAX_EDIT_LINES: usize = 8;

/// Render the track view content area
pub fn render_track_view(frame: &mut Frame, app: &mut App, area: Rect) {
    let track_id = match app.current_track_id() {
        Some(id) => id.to_string(),
        None => {
            let empty = Paragraph::new("No track selected")
                .style(Style::default().fg(app.theme.dim).bg(app.theme.background));
            frame.render_widget(empty, area);
            return;
        }
    };

    // Build flat items and adjust scroll (mutable access to app.track_states)
    let mut flat_items = app.build_flat_items(&track_id);

    // Insert bulk move stand-in at the insertion position
    if let Some(MoveState::BulkTask {
        track_id: ref ms_tid,
        insert_pos,
        ref removed_tasks,
        ..
    }) = app.move_state
        && ms_tid == &track_id
    {
        let count = removed_tasks.len();
        let idx = insert_pos.min(flat_items.len());
        flat_items.insert(idx, FlatItem::BulkMoveStandin { count });
    }

    let visible_height = area.height as usize;
    {
        let state = app.get_track_state(&track_id);
        let cursor = state.cursor.min(flat_items.len().saturating_sub(1));
        state.cursor = cursor;
        if cursor < state.scroll_offset {
            state.scroll_offset = cursor;
        } else if cursor >= state.scroll_offset + visible_height {
            state.scroll_offset = cursor.saturating_sub(visible_height - 1);
        }
    }

    if flat_items.is_empty() {
        if app.filter_state.is_active() {
            let msg = " no matching tasks ";
            let bg = app.theme.background;
            let padding = (area.width as usize).saturating_sub(msg.len() + 1);
            let warn_style = Style::default()
                .fg(app.theme.text_bright)
                .bg(ratatui::style::Color::Rgb(0x8D, 0x0B, 0x0B))
                .add_modifier(Modifier::BOLD);
            let line = Line::from(vec![
                Span::styled(" ".repeat(padding), Style::default().bg(bg)),
                Span::styled(msg, warn_style),
            ]);
            let empty = Paragraph::new(line).style(Style::default().bg(bg));
            frame.render_widget(empty, area);
        } else {
            let bg = app.theme.background;
            let line = Line::from(vec![
                Span::styled(
                    " No tasks yet — press ",
                    Style::default().fg(app.theme.text).bg(bg),
                ),
                Span::styled("a", Style::default().fg(app.theme.highlight).bg(bg)),
                Span::styled(" to add one", Style::default().fg(app.theme.text).bg(bg)),
            ]);
            let empty = Paragraph::new(line).style(Style::default().bg(bg));
            frame.render_widget(empty, area);
        }
        return;
    }

    // Now reborrow immutably for rendering
    let cursor = app.track_states.get(&track_id).map_or(0, |s| s.cursor);
    let track = match app.current_track() {
        Some(t) => t,
        None => return,
    };

    let search_re = app.active_search_re();

    // Build all display lines, tracking cursor's display-line index
    let mut display_lines: Vec<Line> = Vec::new();
    let mut cursor_display_line: Option<usize> = None;
    let mut edit_anchor_info: Option<(u16, usize)> = None; // (prefix_w, display_line_index)
    let mut bulk_editor_anchor: Option<(u16, usize)> = None;

    // Compute range preview bounds for V-select
    let range_preview: Option<(usize, usize)> = app.range_anchor.map(|anchor| {
        if cursor <= anchor {
            (cursor, anchor)
        } else {
            (anchor, cursor)
        }
    });

    for (row, item) in flat_items.iter().enumerate() {
        let is_cursor = row == cursor;

        match item {
            FlatItem::Task {
                section,
                path,
                depth,
                has_children,
                is_expanded,
                is_last_sibling,
                ancestor_last,
                is_context,
            } => {
                if let Some(task) = resolve_task(track, *section, path) {
                    // Context rows (filter ancestors) are never selectable
                    let effective_cursor = is_cursor && !is_context;
                    let is_flash =
                        !is_context && task.id.as_deref().is_some_and(|id| app.is_flashing(id));
                    let in_range = !is_context
                        && range_preview.is_some_and(|(start, end)| row >= start && row <= end);
                    let is_selected = !is_context
                        && (in_range
                            || task
                                .id
                                .as_deref()
                                .is_some_and(|id| app.selection.contains(id)));

                    if is_cursor {
                        cursor_display_line = Some(display_lines.len());
                    }

                    let (task_lines, col) = render_task_line(
                        app,
                        task,
                        &TaskLineInfo {
                            depth: *depth,
                            has_children: *has_children,
                            is_expanded: *is_expanded,
                            is_last_sibling: *is_last_sibling,
                            ancestor_last,
                        },
                        effective_cursor,
                        is_flash,
                        is_selected,
                        *is_context,
                        area.width as usize,
                        search_re.as_ref(),
                    );
                    if let Some(prefix_w) = col {
                        edit_anchor_info =
                            Some((prefix_w, cursor_display_line.unwrap_or(display_lines.len())));
                    }
                    display_lines.extend(task_lines);

                    // Insert bulk inline editor below cursor row
                    if is_cursor && let Some(ref et) = app.edit_target {
                        let label = match et {
                            EditTarget::BulkTags => Some("tags:"),
                            EditTarget::BulkDeps => Some("deps:"),
                            _ => None,
                        };
                        if let Some(label) = label {
                            let (editor_line, ec) =
                                render_bulk_editor_line(app, label, area.width as usize);
                            bulk_editor_anchor = Some((ec, display_lines.len()));
                            display_lines.push(editor_line);
                        }
                    }
                }
            }
            FlatItem::ParkedSeparator => {
                if is_cursor {
                    cursor_display_line = Some(display_lines.len());
                }
                display_lines.push(render_parked_separator(app, area.width as usize, is_cursor));
            }
            FlatItem::BulkMoveStandin { count } => {
                display_lines.push(render_bulk_standin(app, *count, area.width as usize));
            }
            FlatItem::DoneSummary {
                depth,
                done_count,
                total_count,
                ancestor_last,
            } => {
                display_lines.push(render_done_summary(
                    app,
                    *depth,
                    *done_count,
                    *total_count,
                    ancestor_last,
                    area.width as usize,
                ));
            }
        }
    }

    // Adjust scroll in display-line space
    let cdl = cursor_display_line.unwrap_or(0);
    let mut scroll = app
        .track_states
        .get(&track_id)
        .map_or(0, |s| s.scroll_offset);
    if cdl < scroll {
        scroll = cdl;
    } else if cdl >= scroll + visible_height {
        scroll = cdl.saturating_sub(visible_height - 1);
    }
    // Persist scroll back (display-line space)
    {
        let state = app.get_track_state(&track_id);
        state.scroll_offset = scroll;
    }

    // Slice visible display lines
    let lines: Vec<Line> = display_lines
        .into_iter()
        .skip(scroll)
        .take(visible_height)
        .collect();

    let paragraph = Paragraph::new(lines).style(Style::default().bg(app.theme.background));
    frame.render_widget(paragraph, area);

    // Set autocomplete anchor now that immutable borrows are released
    if let Some((ec, dl_idx)) = bulk_editor_anchor {
        let screen_y = area.y + dl_idx.saturating_sub(scroll) as u16;
        let screen_x = area.x + ec;
        app.autocomplete_anchor = Some((screen_x, screen_y));
    } else if let Some((prefix_w, dl_idx)) = edit_anchor_info {
        let word_offset = app
            .autocomplete
            .as_ref()
            .map(|ac| ac.word_start_in_buffer(&app.edit_buffer) as u16)
            .unwrap_or(0);
        let screen_y = area.y + dl_idx.saturating_sub(scroll) as u16;
        let screen_x = area.x + prefix_w + word_offset;
        app.autocomplete_anchor = Some((screen_x, screen_y));
    } else if app.mode == Mode::Triage {
        // Cross-track move: anchor autocomplete to the cursor row
        let screen_y = area.y + cdl.saturating_sub(scroll) as u16;
        let screen_x = area.x + 4;
        app.autocomplete_anchor = Some((screen_x, screen_y));
    } else if app.mode == Mode::Edit && matches!(app.edit_target, Some(EditTarget::FilterTag)) {
        // Filter tag selection: anchor autocomplete to the cursor row
        let screen_y = area.y + cdl.saturating_sub(scroll) as u16;
        let screen_x = area.x + 4;
        app.autocomplete_anchor = Some((screen_x, screen_y));
    }
}

/// Resolve a task from a path through the track's sections
fn resolve_task<'a>(
    track: &'a crate::model::Track,
    section: SectionKind,
    path: &[usize],
) -> Option<&'a Task> {
    let tasks = track.section_tasks(section);
    if path.is_empty() {
        return None;
    }

    let mut current = tasks.get(path[0])?;
    for &idx in &path[1..] {
        current = current.subtasks.get(idx)?;
    }
    Some(current)
}

/// Info about a task's position in the tree (passed to renderer)
struct TaskLineInfo<'a> {
    depth: usize,
    has_children: bool,
    is_expanded: bool,
    is_last_sibling: bool,
    ancestor_last: &'a [bool],
}

/// Render a single task as one or more display lines with all decorations.
/// Returns the lines and optionally the column offset where an edit buffer starts
/// (used for autocomplete anchor positioning).
#[allow(clippy::too_many_arguments)]
fn render_task_line(
    app: &App,
    task: &Task,
    info: &TaskLineInfo<'_>,
    is_cursor: bool,
    is_flash: bool,
    is_selected: bool,
    is_context: bool,
    width: usize,
    search_re: Option<&Regex>,
) -> (Vec<Line<'static>>, Option<u16>) {
    let mut spans: Vec<Span> = Vec::new();
    let mut edit_col: Option<u16> = None;
    let bg = app.theme.background;
    let dim_style = Style::default().fg(app.theme.dim).bg(bg);
    let state_color = if is_context {
        app.theme.dim
    } else {
        app.theme.state_color(task.state)
    };

    // Row background: flash > cursor > selected > normal
    let (flash_bg_color, flash_border_color) = match app.flash_state {
        Some(state) => state_flash_colors(state, &app.theme),
        None => UNDO_FLASH_COLORS,
    };
    let row_bg = if is_flash {
        flash_bg_color
    } else if is_cursor {
        app.theme.selection_bg
    } else if is_selected {
        app.theme.bulk_selection_bg
    } else {
        bg
    };

    // Column 0 reservation: left border accent for cursor/flash/selected, space otherwise
    let has_selection = !app.selection.is_empty();
    if is_flash {
        spans.push(Span::styled(
            "\u{258E}",
            Style::default().fg(flash_border_color).bg(row_bg),
        ));
    } else if is_cursor && (!has_selection || is_selected) {
        // Cursor bar only shows if no selection active, or cursor row is itself selected
        spans.push(Span::styled(
            "\u{258E}",
            Style::default().fg(app.theme.selection_border).bg(row_bg),
        ));
    } else if is_selected {
        // Selected but not cursor: show ▌ bar in highlight color
        spans.push(Span::styled(
            "\u{258C}",
            Style::default().fg(app.theme.highlight).bg(row_bg),
        ));
    } else {
        spans.push(Span::styled(" ", Style::default().bg(bg)));
    }

    // Build prefix based on depth
    if info.depth == 0 {
        // Top-level: [expand][state] ID Title  tags
        let expand_char = if info.has_children {
            if info.is_expanded {
                "\u{25BC}"
            } else {
                "\u{25B6}"
            } // ▼ / ▶
        } else {
            " "
        };
        spans.push(Span::styled(expand_char, dim_style));
    } else {
        // Subtask: indent + tree chars + [expand?][state] .ID Title  tags
        for (d, is_ancestor_last) in info.ancestor_last.iter().enumerate() {
            if d == 0 || *is_ancestor_last {
                spans.push(Span::styled("   ", dim_style));
            } else {
                spans.push(Span::styled("\u{2502}  ", dim_style)); // │ + 2 spaces
            }
        }

        // Tree char for current level
        let tree_char = if info.is_last_sibling {
            "\u{2514}"
        } else {
            "\u{251C}"
        }; // └ / ├
        spans.push(Span::styled(tree_char, dim_style));

        // Replace the space after tree char with expand indicator when applicable
        if info.has_children {
            let expand_char = if info.is_expanded {
                "\u{25BC}"
            } else {
                "\u{25B6}"
            };
            spans.push(Span::styled(expand_char, dim_style));
        } else {
            spans.push(Span::styled(" ", dim_style));
        }
    }

    // State symbol
    let state_style = if is_cursor {
        Style::default()
            .fg(state_color)
            .bg(bg)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(state_color).bg(bg)
    };
    spans.push(Span::styled(state_symbol(task.state), state_style));
    spans.push(Span::styled(" ", Style::default().bg(bg)));

    // ID
    let id_text = if info.depth == 0 {
        task.id
            .as_deref()
            .map(|id| format!("{} ", id))
            .unwrap_or_default()
    } else {
        task.id
            .as_deref()
            .map(|id| format!("{} ", abbreviated_id(id)))
            .unwrap_or_default()
    };
    if !id_text.is_empty() {
        let id_style = if is_context || task.state == TaskState::Done {
            Style::default().fg(app.theme.dim).bg(bg)
        } else if is_cursor {
            Style::default().fg(app.theme.selection_id).bg(bg)
        } else {
            Style::default().fg(app.theme.text).bg(bg)
        };
        let highlight_style = Style::default()
            .fg(app.theme.search_match_fg)
            .bg(app.theme.search_match_bg)
            .add_modifier(Modifier::BOLD);
        push_highlighted_spans(&mut spans, &id_text, id_style, highlight_style, search_re);
    }

    // Snapshot prefix spans (for wrap continuation indent calculation)
    let prefix_spans_snapshot = spans.clone();

    // Check if this task is being edited inline (title or tags)
    let is_editing = is_cursor
        && app.mode == Mode::Edit
        && app.edit_target.as_ref().is_some_and(|et| match et {
            EditTarget::NewTask { task_id, .. }
            | EditTarget::ExistingTitle { task_id, .. }
            | EditTarget::ExistingTags { task_id, .. } => task.id.as_deref() == Some(task_id),
            _ => false,
        });
    let is_editing_tags = is_cursor
        && app.mode == Mode::Edit
        && app.edit_target.as_ref().is_some_and(|et| matches!(et, EditTarget::ExistingTags { task_id, .. } if task.id.as_deref() == Some(task_id)));

    // Title (with search highlighting, or edit buffer if editing)
    let title_style = if is_context {
        Style::default().fg(app.theme.dim).bg(bg)
    } else if is_cursor {
        Style::default()
            .fg(app.theme.text_bright)
            .bg(bg)
            .add_modifier(Modifier::BOLD)
    } else if task.state == TaskState::Done {
        Style::default().fg(app.theme.dim).bg(bg)
    } else {
        Style::default().fg(app.theme.text_bright).bg(bg)
    };

    if is_editing && !is_editing_tags {
        // Wrap-aware title editing
        let prefix_width = spans_width(&spans);
        edit_col = Some(prefix_width as u16);
        let edit_available = width.saturating_sub(prefix_width);
        let buf = &app.edit_buffer;
        let cursor_pos = app.edit_cursor.min(buf.len());

        if edit_available > 0 {
            let edit_text = [buf.as_str()];
            let visual_lines = wrap::wrap_lines_for_edit(&edit_text, edit_available, 0, cursor_pos);
            let total_visual = visual_lines.len();
            let visible_visual = total_visual.clamp(1, MAX_EDIT_LINES);
            let cursor_vrow = wrap::logical_to_visual_row(&visual_lines, 0, cursor_pos);

            // Scroll to keep cursor visible
            let mut title_scroll = 0usize;
            if cursor_vrow >= visible_visual {
                title_scroll = cursor_vrow.saturating_sub(visible_visual - 1);
            }

            let edit_style = title_style;
            let cursor_block_style = Style::default()
                .fg(app.theme.background)
                .bg(app.theme.text_bright);
            let selection_style = Style::default()
                .fg(app.theme.text_bright)
                .bg(app.theme.blue);
            let sel_range = app.edit_selection_range();

            let mut edit_lines: Vec<Line<'static>> = Vec::new();

            for view_row in 0..visible_visual {
                let vrow_idx = title_scroll + view_row;
                if vrow_idx >= total_visual {
                    break;
                }
                let vl = &visual_lines[vrow_idx];
                let slice = &buf[vl.byte_start..vl.byte_end];
                let has_cursor = vrow_idx == cursor_vrow;

                let mut line_spans: Vec<Span<'static>> = Vec::new();

                if view_row == 0 {
                    // First visual line: reuse existing prefix spans
                    line_spans.append(&mut spans);
                } else {
                    // Continuation lines: indent to match prefix
                    line_spans.push(Span::styled(
                        " ".repeat(prefix_width),
                        Style::default().bg(row_bg),
                    ));
                }

                // Render text with cursor and selection
                let graphemes: Vec<(usize, &str)> =
                    unicode_segmentation::UnicodeSegmentation::grapheme_indices(slice, true)
                        .collect();

                if let Some((sel_start, sel_end)) = sel_range {
                    if sel_start != sel_end {
                        // Active selection: highlight selected range
                        for &(gi, g) in &graphemes {
                            let abs_byte = vl.byte_start + gi;
                            if abs_byte >= sel_start && abs_byte < sel_end {
                                line_spans.push(Span::styled(g.to_string(), selection_style));
                            } else if has_cursor
                                && gi == cursor_pos.saturating_sub(vl.byte_start)
                                && cursor_pos < buf.len()
                            {
                                line_spans.push(Span::styled(g.to_string(), cursor_block_style));
                            } else {
                                line_spans.push(Span::styled(g.to_string(), edit_style));
                            }
                        }
                        if has_cursor && cursor_pos >= vl.byte_end {
                            line_spans.push(Span::styled(" ".to_string(), cursor_block_style));
                        }
                    } else {
                        // Empty selection, render with cursor
                        render_edit_graphemes_with_cursor(
                            &mut line_spans,
                            &graphemes,
                            vl,
                            cursor_pos,
                            has_cursor,
                            edit_style,
                            cursor_block_style,
                            buf.len(),
                        );
                    }
                } else {
                    render_edit_graphemes_with_cursor(
                        &mut line_spans,
                        &graphemes,
                        vl,
                        cursor_pos,
                        has_cursor,
                        edit_style,
                        cursor_block_style,
                        buf.len(),
                    );
                }

                // Pad to full width with row background
                let content_width: usize = line_spans
                    .iter()
                    .map(|s| unicode::display_width(&s.content))
                    .sum();
                if content_width < width {
                    line_spans.push(Span::styled(
                        " ".repeat(width - content_width),
                        Style::default().bg(row_bg),
                    ));
                }

                edit_lines.push(Line::from(line_spans));
            }

            // Scroll indicator when total visual lines exceed visible
            if total_visual > visible_visual {
                let dim_style = Style::default().fg(app.theme.dim).bg(app.theme.background);
                let indicator = format!(
                    "{}[{}/{}]",
                    " ".repeat(prefix_width),
                    title_scroll + visible_visual,
                    total_visual,
                );
                edit_lines.push(Line::from(Span::styled(indicator, dim_style)));
            }

            return (edit_lines, edit_col);
        }
        // Fallback: edit_available == 0, just push buffer as-is
        spans.push(Span::styled(buf.to_string(), title_style));
    } else {
        let highlight_style = Style::default()
            .fg(app.theme.search_match_fg)
            .bg(app.theme.search_match_bg)
            .add_modifier(Modifier::BOLD);
        // Push full title (wrapping will handle overflow)
        push_highlighted_spans(
            &mut spans,
            &task.title,
            title_style,
            highlight_style,
            search_re,
        );
    }

    // Tags (inline edit buffer when editing tags, otherwise normal rendering)
    if is_editing_tags {
        spans.push(Span::styled("  ", Style::default().bg(bg)));
        // Record prefix width for autocomplete anchor (after the "  " spacer)
        edit_col = Some(spans_width(&spans) as u16);
        let buf = &app.edit_buffer;
        let cursor_pos = app.edit_cursor.min(buf.len());
        let cursor_style = Style::default()
            .fg(app.theme.background)
            .bg(app.theme.text_bright);
        let tag_edit_style = title_style;
        let selection_style = Style::default()
            .fg(app.theme.text_bright)
            .bg(app.theme.blue);

        if let Some((sel_start, sel_end)) = app.edit_selection_range() {
            if sel_start != sel_end {
                if sel_start > 0 {
                    spans.push(Span::styled(buf[..sel_start].to_string(), tag_edit_style));
                }
                spans.push(Span::styled(
                    buf[sel_start..sel_end].to_string(),
                    selection_style,
                ));
                if sel_end < buf.len() {
                    spans.push(Span::styled(buf[sel_end..].to_string(), tag_edit_style));
                }
                if cursor_pos >= buf.len() {
                    spans.push(Span::styled(" ".to_string(), cursor_style));
                }
            } else {
                let before = &buf[..cursor_pos];
                if !before.is_empty() {
                    spans.push(Span::styled(before.to_string(), tag_edit_style));
                }
                if cursor_pos < buf.len() {
                    let grapheme = unicode::grapheme_at(buf, cursor_pos);
                    spans.push(Span::styled(grapheme.to_string(), cursor_style));
                    let after = &buf[cursor_pos + grapheme.len()..];
                    if !after.is_empty() {
                        spans.push(Span::styled(after.to_string(), tag_edit_style));
                    }
                } else {
                    spans.push(Span::styled(" ".to_string(), cursor_style));
                }
            }
        } else {
            let before = &buf[..cursor_pos];
            if !before.is_empty() {
                spans.push(Span::styled(before.to_string(), tag_edit_style));
            }
            if cursor_pos < buf.len() {
                let grapheme = unicode::grapheme_at(buf, cursor_pos);
                spans.push(Span::styled(grapheme.to_string(), cursor_style));
                let after = &buf[cursor_pos + grapheme.len()..];
                if !after.is_empty() {
                    spans.push(Span::styled(after.to_string(), tag_edit_style));
                }
            } else {
                spans.push(Span::styled(" ".to_string(), cursor_style));
            }
        }
    } else if !task.tags.is_empty() {
        spans.push(Span::styled("  ", Style::default().bg(bg)));
        let tag_hl_style = Style::default()
            .fg(app.theme.search_match_fg)
            .bg(app.theme.search_match_bg)
            .add_modifier(Modifier::BOLD);
        for (i, tag) in task.tags.iter().enumerate() {
            let tag_color = app.theme.tag_color(tag);
            let tag_style = if is_context || task.state == TaskState::Done {
                Style::default().fg(app.theme.dim).bg(bg)
            } else {
                Style::default().fg(tag_color).bg(bg)
            };
            if i > 0 {
                spans.push(Span::styled(" ", Style::default().bg(bg)));
            }
            push_highlighted_spans(
                &mut spans,
                &format!("#{}", tag),
                tag_style,
                tag_hl_style,
                search_re,
            );
        }
    }

    // Hidden match indicator for non-visible field matches
    if let Some(indicator) = hidden_match_indicator(task, search_re) {
        let hl_style = Style::default()
            .fg(app.theme.search_match_fg)
            .bg(app.theme.search_match_bg)
            .add_modifier(Modifier::BOLD);
        spans.push(Span::styled("  ", Style::default().bg(bg)));
        spans.push(Span::styled(indicator, hl_style));
    }

    // Wrap spans into multiple display lines
    let prefix_width = spans_width(&prefix_spans_snapshot);
    let wrapped = wrap_styled_spans(spans, width, prefix_width, bg);

    let mut result_lines: Vec<Line<'static>> = Vec::new();
    for mut wrapped_line in wrapped {
        // Apply row background + padding for cursor/flash/selected lines
        if is_cursor || is_flash || is_selected {
            let content_width: usize = wrapped_line
                .spans
                .iter()
                .map(|s| unicode::display_width(&s.content))
                .sum();
            if content_width < width {
                wrapped_line.spans.push(Span::styled(
                    " ".repeat(width - content_width),
                    Style::default().bg(row_bg),
                ));
            }
            // Re-style all spans with row background
            // Skip column 0 border (first line only), search match spans,
            // text selection spans, and cursor spans
            let search_bg = app.theme.search_match_bg;
            let selection_blue = app.theme.blue;
            let cursor_bg = app.theme.text_bright;
            let skip = if result_lines.is_empty() { 1 } else { 0 };
            for span in wrapped_line.spans.iter_mut().skip(skip) {
                let bg_color = span.style.bg;
                if bg_color != Some(search_bg)
                    && bg_color != Some(selection_blue)
                    && bg_color != Some(cursor_bg)
                {
                    span.style = span.style.bg(row_bg);
                }
            }
        }
        result_lines.push(wrapped_line);
    }

    (result_lines, edit_col)
}

/// Helper: render graphemes for an edit visual line, placing cursor block at the right position.
#[allow(clippy::too_many_arguments)]
fn render_edit_graphemes_with_cursor(
    line_spans: &mut Vec<Span<'static>>,
    graphemes: &[(usize, &str)],
    vl: &wrap::VisualLine,
    cursor_pos: usize,
    has_cursor: bool,
    edit_style: Style,
    cursor_block_style: Style,
    buf_len: usize,
) {
    if has_cursor {
        let cursor_byte_in_row = cursor_pos.min(vl.byte_end).saturating_sub(vl.byte_start);
        let mut cursor_rendered = false;
        for &(gi, g) in graphemes {
            if gi == cursor_byte_in_row && !cursor_rendered {
                line_spans.push(Span::styled(g.to_string(), cursor_block_style));
                cursor_rendered = true;
            } else {
                line_spans.push(Span::styled(g.to_string(), edit_style));
            }
        }
        if !cursor_rendered {
            line_spans.push(Span::styled(" ".to_string(), cursor_block_style));
        }
    } else if !graphemes.is_empty() {
        // Non-cursor line: emit as single span
        let text: String = graphemes.iter().map(|(_, g)| *g).collect();
        line_spans.push(Span::styled(text, edit_style));
    }
    let _ = (vl, buf_len); // suppress unused warnings
}

/// Render the bulk inline editor line (tags:/deps: label + edit buffer + cursor).
/// Returns the line and the column offset of the edit text start (for autocomplete anchor).
fn render_bulk_editor_line(app: &App, label: &str, width: usize) -> (Line<'static>, u16) {
    let bg = app.theme.background;
    let mut spans: Vec<Span> = Vec::new();

    // Indent + label
    spans.push(Span::styled("  ", Style::default().bg(bg)));
    spans.push(Span::styled(
        format!("{} ", label),
        Style::default().fg(app.theme.dim).bg(bg),
    ));

    let edit_col = spans_width(&spans) as u16;

    // Edit buffer with cursor
    let buf = &app.edit_buffer;
    let cursor_pos = app.edit_cursor.min(buf.len());
    let title_style = Style::default().fg(app.theme.text_bright).bg(bg);
    let cursor_style = Style::default()
        .fg(app.theme.background)
        .bg(app.theme.text_bright);

    let before = &buf[..cursor_pos];
    if !before.is_empty() {
        spans.push(Span::styled(before.to_string(), title_style));
    }
    if cursor_pos < buf.len() {
        let grapheme = unicode::grapheme_at(buf, cursor_pos);
        spans.push(Span::styled(grapheme.to_string(), cursor_style));
        let after = &buf[cursor_pos + grapheme.len()..];
        if !after.is_empty() {
            spans.push(Span::styled(after.to_string(), title_style));
        }
    } else {
        spans.push(Span::styled(" ".to_string(), cursor_style));
    }

    // Fill remaining width
    let content_width = spans_width(&spans);
    if content_width < width {
        spans.push(Span::styled(
            " ".repeat(width - content_width),
            Style::default().bg(bg),
        ));
    }

    (Line::from(spans), edit_col)
}

/// Render the parked section separator
fn render_parked_separator(app: &App, width: usize, is_cursor: bool) -> Line<'static> {
    let bg = if is_cursor {
        app.theme.selection_bg
    } else {
        app.theme.background
    };
    let style = Style::default().fg(app.theme.dim).bg(bg);

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

    let label = " Parked ";
    let dashes_before = 2;
    let dashes_after = width.saturating_sub(label.len() + dashes_before + 2);

    let line_text = format!(
        "{}{}{}",
        "\u{2500}".repeat(dashes_before),
        label,
        "\u{2500}".repeat(dashes_after.max(2))
    );

    spans.push(Span::styled(line_text, style));
    Line::from(spans)
}

/// Render the bulk move stand-in row: "━━━ N tasks ━━━"
fn render_bulk_standin(app: &App, count: usize, width: usize) -> Line<'static> {
    let bg = app.theme.selection_bg;
    let style = Style::default()
        .fg(app.theme.highlight)
        .bg(bg)
        .add_modifier(Modifier::BOLD);

    let mut spans: Vec<Span> = Vec::new();
    spans.push(Span::styled(
        "\u{258E}",
        Style::default().fg(app.theme.selection_border).bg(bg),
    ));

    let label = format!(" {} task{} ", count, if count == 1 { "" } else { "s" });
    let bar_char = "\u{2501}"; // ━ heavy horizontal
    let dashes_before = 3;
    let dashes_after = width.saturating_sub(label.len() + dashes_before + 2);
    let line_text = format!(
        "{}{}{}",
        bar_char.repeat(dashes_before),
        label,
        bar_char.repeat(dashes_after.max(2))
    );

    spans.push(Span::styled(line_text, style));
    Line::from(spans)
}

/// Render the "X/Y done" summary row for hidden done subtasks
fn render_done_summary(
    app: &App,
    _depth: usize,
    done_count: usize,
    total_count: usize,
    ancestor_last: &[bool],
    width: usize,
) -> Line<'static> {
    let bg = app.theme.background;
    let dim_style = Style::default().fg(app.theme.dim).bg(bg);

    let mut spans: Vec<Span> = Vec::new();

    // Column 0: space (never selectable)
    spans.push(Span::styled(" ", Style::default().bg(bg)));

    // Tree indentation (same logic as subtask rendering)
    for (d, is_ancestor_last) in ancestor_last.iter().enumerate() {
        if d == 0 || *is_ancestor_last {
            spans.push(Span::styled("   ", dim_style));
        } else {
            spans.push(Span::styled("\u{2502}  ", dim_style)); // │ + 2 spaces
        }
    }

    // Tree char: vertical bar + space (continuation, not a branch)
    spans.push(Span::styled("\u{2502} ", dim_style)); // │ + space

    // Summary text
    let text = format!("{}/{} done", done_count, total_count);
    spans.push(Span::styled(text.clone(), dim_style));

    // Fill remaining width
    let used: usize = 1 + ancestor_last.len() * 3 + 2 + text.len();
    if width > used {
        spans.push(Span::styled(
            " ".repeat(width - used),
            Style::default().bg(bg),
        ));
    }

    Line::from(spans)
}

/// Build a hidden match indicator for non-visible field matches on a task.
/// Returns None if the regex doesn't match any hidden fields (note, dep, ref, spec).
/// Returns e.g. "[2 matches: note, dep]" or "[1 match: note]".
fn hidden_match_indicator(task: &Task, search_re: Option<&Regex>) -> Option<String> {
    let re = search_re?;

    // Count matches per hidden field type
    let mut note_count = 0usize;
    let mut dep_count = 0usize;
    let mut ref_count = 0usize;
    let mut spec_count = 0usize;

    for meta in &task.metadata {
        match meta {
            Metadata::Note(text) => note_count += re.find_iter(text).count(),
            Metadata::Dep(deps) => {
                for dep in deps {
                    dep_count += re.find_iter(dep).count();
                }
            }
            Metadata::Ref(refs) => {
                for r in refs {
                    ref_count += re.find_iter(r).count();
                }
            }
            Metadata::Spec(spec) => spec_count += re.find_iter(spec).count(),
            _ => {}
        }
    }

    let total = note_count + dep_count + ref_count + spec_count;
    if total == 0 {
        return None;
    }

    // Build field list (up to 3 names, then ...)
    let mut fields: Vec<&str> = Vec::new();
    if note_count > 0 {
        fields.push("note");
    }
    if dep_count > 0 {
        fields.push("dep");
    }
    if ref_count > 0 {
        fields.push("ref");
    }
    if spec_count > 0 {
        fields.push("spec");
    }

    let match_word = if total == 1 { "match" } else { "matches" };

    let field_str = if fields.len() > 3 {
        format!("{}, ...", fields[..3].join(", "))
    } else {
        fields.join(", ")
    };

    Some(format!("[{} {}: {}]", total, match_word, field_str))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::render::test_helpers::*;
    use insta::assert_snapshot;

    #[test]
    fn empty_track() {
        let mut app = app_with_track("# Empty\n\n## Backlog\n\n## Done\n");
        let output = render_to_string(TERM_W, TERM_H, |frame, area| {
            render_track_view(frame, &mut app, area);
        });
        assert_snapshot!(output);
    }

    #[test]
    fn simple_backlog() {
        let mut app = app_with_track(SIMPLE_TRACK_MD);
        let output = render_to_string(TERM_W, TERM_H, |frame, area| {
            render_track_view(frame, &mut app, area);
        });
        assert_snapshot!(output);
    }

    #[test]
    fn track_with_subtasks() {
        let md = "\
# Test

## Backlog

- [ ] `T-1` Parent task
  - [ ] `T-1.1` Child one
  - [>] `T-1.2` Child two
- [ ] `T-2` Another task

## Done
";
        let mut app = app_with_track(md);
        let output = render_to_string(TERM_W, TERM_H, |frame, area| {
            render_track_view(frame, &mut app, area);
        });
        assert_snapshot!(output);
    }

    #[test]
    fn complex_fixture() {
        let mut app = app_with_fixture();
        let output = render_to_string(TERM_W, TERM_H, |frame, area| {
            render_track_view(frame, &mut app, area);
        });
        assert_snapshot!(output);
    }

    #[test]
    fn track_with_done_tasks() {
        let md = "\
# Test

## Backlog

- [ ] `T-1` A todo task

## Done

- [x] `T-2` Finished task
  - resolved: 2025-05-14
- [x] `T-3` Another done
  - resolved: 2025-05-12
";
        let mut app = app_with_track(md);
        let output = render_to_string(TERM_W, TERM_H, |frame, area| {
            render_track_view(frame, &mut app, area);
        });
        assert_snapshot!(output);
    }
}
