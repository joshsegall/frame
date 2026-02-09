use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use regex::Regex;

use crate::model::{Metadata, SectionKind, Task, TaskState};
use crate::tui::app::{App, EditTarget, FlatItem, Mode, MoveState};

use super::detail_view::{UNDO_FLASH_COLORS, state_flash_colors};
use super::push_highlighted_spans;

/// State symbols for each task state (markdown checkbox style)
fn state_symbol(state: TaskState) -> &'static str {
    match state {
        TaskState::Todo => "[ ]",
        TaskState::Active => "[>]",
        TaskState::Blocked => "[-]",
        TaskState::Done => "[x]",
        TaskState::Parked => "[~]",
    }
}

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
    {
        if ms_tid == &track_id {
            let count = removed_tasks.len();
            let idx = insert_pos.min(flat_items.len());
            flat_items.insert(idx, FlatItem::BulkMoveStandin { count });
        }
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
            let empty = Paragraph::new(" No tasks")
                .style(Style::default().fg(app.theme.dim).bg(app.theme.background));
            frame.render_widget(empty, area);
        }
        return;
    }

    // Now reborrow immutably for rendering
    let cursor = app.track_states.get(&track_id).map_or(0, |s| s.cursor);
    let scroll = app
        .track_states
        .get(&track_id)
        .map_or(0, |s| s.scroll_offset);
    let track = match app.current_track() {
        Some(t) => t,
        None => return,
    };

    let search_re = app.active_search_re();
    let end = flat_items.len().min(scroll + visible_height);
    let mut lines: Vec<Line> = Vec::with_capacity(visible_height);
    let mut edit_anchor: Option<(u16, u16)> = None;

    // Compute range preview bounds for V-select
    let range_preview: Option<(usize, usize)> = app.range_anchor.map(|anchor| {
        if cursor <= anchor {
            (cursor, anchor)
        } else {
            (anchor, cursor)
        }
    });

    for (item, row) in flat_items[scroll..end].iter().zip(scroll..end) {
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
                    let (line, col) = render_task_line(
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
                        let word_offset = app
                            .autocomplete
                            .as_ref()
                            .map(|ac| ac.word_start_in_buffer(&app.edit_buffer) as u16)
                            .unwrap_or(0);
                        let screen_y = area.y + (row - scroll) as u16;
                        let screen_x = area.x + prefix_w + word_offset;
                        edit_anchor = Some((screen_x, screen_y));
                    }
                    lines.push(line);

                    // Insert bulk inline editor below cursor row
                    if is_cursor && lines.len() < visible_height {
                        if let Some(ref et) = app.edit_target {
                            let label = match et {
                                EditTarget::BulkTags => Some("tags:"),
                                EditTarget::BulkDeps => Some("deps:"),
                                _ => None,
                            };
                            if let Some(label) = label {
                                let (editor_line, ec) =
                                    render_bulk_editor_line(app, label, area.width as usize);
                                let screen_y = area.y + lines.len() as u16;
                                let screen_x = area.x + ec;
                                edit_anchor = Some((screen_x, screen_y));
                                lines.push(editor_line);
                            }
                        }
                    }
                }
            }
            FlatItem::ParkedSeparator => {
                lines.push(render_parked_separator(app, area.width as usize, is_cursor));
            }
            FlatItem::BulkMoveStandin { count } => {
                lines.push(render_bulk_standin(app, *count, area.width as usize));
            }
        }
    }

    let paragraph = Paragraph::new(lines).style(Style::default().bg(app.theme.background));
    frame.render_widget(paragraph, area);

    // Set autocomplete anchor now that immutable borrows are released
    if let Some(anchor) = edit_anchor {
        app.autocomplete_anchor = Some(anchor);
    } else if app.mode == Mode::Triage {
        // Cross-track move: anchor autocomplete to the cursor row
        let screen_y = area.y + cursor.saturating_sub(scroll) as u16;
        let screen_x = area.x + 4;
        app.autocomplete_anchor = Some((screen_x, screen_y));
    } else if app.mode == Mode::Edit && matches!(app.edit_target, Some(EditTarget::FilterTag)) {
        // Filter tag selection: anchor autocomplete to the cursor row
        let screen_y = area.y + cursor.saturating_sub(scroll) as u16;
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

/// Render a single task line with all decorations.
/// Returns the line and optionally the column offset where an edit buffer starts
/// (used for autocomplete anchor positioning).
fn render_task_line<'a>(
    app: &'a App,
    task: &Task,
    info: &TaskLineInfo<'_>,
    is_cursor: bool,
    is_flash: bool,
    is_selected: bool,
    is_context: bool,
    width: usize,
    search_re: Option<&Regex>,
) -> (Line<'a>, Option<u16>) {
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
        abbreviated_id(task).map_or(String::new(), |s| format!("{} ", s))
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
        // Record prefix width for autocomplete anchor
        edit_col = Some(spans.iter().map(|s| s.content.chars().count() as u16).sum());
        // Render edit buffer with cursor/selection highlighting
        let buf = &app.edit_buffer;
        let cursor_pos = app.edit_cursor.min(buf.len());
        let cursor_style = Style::default()
            .fg(app.theme.background)
            .bg(app.theme.text_bright);
        let selection_style = Style::default()
            .fg(app.theme.text_bright)
            .bg(app.theme.blue);

        if let Some((sel_start, sel_end)) = app.edit_selection_range() {
            if sel_start != sel_end {
                if sel_start > 0 {
                    spans.push(Span::styled(buf[..sel_start].to_string(), title_style));
                }
                spans.push(Span::styled(
                    buf[sel_start..sel_end].to_string(),
                    selection_style,
                ));
                if sel_end < buf.len() {
                    spans.push(Span::styled(buf[sel_end..].to_string(), title_style));
                }
                if cursor_pos >= buf.len() {
                    spans.push(Span::styled(" ".to_string(), cursor_style));
                }
            } else {
                // Empty selection, render normally with cursor
                let before = &buf[..cursor_pos];
                if !before.is_empty() {
                    spans.push(Span::styled(before.to_string(), title_style));
                }
                if cursor_pos < buf.len() {
                    let cursor_char = &buf[cursor_pos..cursor_pos + 1];
                    spans.push(Span::styled(cursor_char.to_string(), cursor_style));
                    let after = &buf[cursor_pos + 1..];
                    if !after.is_empty() {
                        spans.push(Span::styled(after.to_string(), title_style));
                    }
                } else {
                    spans.push(Span::styled(" ".to_string(), cursor_style));
                }
            }
        } else {
            let before = &buf[..cursor_pos];
            if !before.is_empty() {
                spans.push(Span::styled(before.to_string(), title_style));
            }
            if cursor_pos < buf.len() {
                let cursor_char = &buf[cursor_pos..cursor_pos + 1];
                spans.push(Span::styled(cursor_char.to_string(), cursor_style));
                let after = &buf[cursor_pos + 1..];
                if !after.is_empty() {
                    spans.push(Span::styled(after.to_string(), title_style));
                }
            } else {
                spans.push(Span::styled(" ".to_string(), cursor_style));
            }
        }
    } else {
        let highlight_style = Style::default()
            .fg(app.theme.search_match_fg)
            .bg(app.theme.search_match_bg)
            .add_modifier(Modifier::BOLD);
        // Truncate title if it would overflow the available width
        let prefix_width: usize = spans.iter().map(|s| s.content.chars().count()).sum();
        let tag_width: usize = task.tags.iter().map(|t| t.len() + 2).sum::<usize>()
            + if task.tags.is_empty() { 0 } else { 2 };
        let available = width.saturating_sub(prefix_width + tag_width + 1);
        let display_title = super::truncate_with_ellipsis(&task.title, available);
        push_highlighted_spans(
            &mut spans,
            &display_title,
            title_style,
            highlight_style,
            search_re,
        );
    }

    // Tags (inline edit buffer when editing tags, otherwise normal rendering)
    if is_editing_tags {
        spans.push(Span::styled("  ", Style::default().bg(bg)));
        // Record prefix width for autocomplete anchor (after the "  " spacer)
        edit_col = Some(spans.iter().map(|s| s.content.chars().count() as u16).sum());
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
                    let cursor_char = &buf[cursor_pos..cursor_pos + 1];
                    spans.push(Span::styled(cursor_char.to_string(), cursor_style));
                    let after = &buf[cursor_pos + 1..];
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
                let cursor_char = &buf[cursor_pos..cursor_pos + 1];
                spans.push(Span::styled(cursor_char.to_string(), cursor_style));
                let after = &buf[cursor_pos + 1..];
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
        let indicator_width = indicator.chars().count() + 2; // "  " + indicator
        let content_width: usize = spans.iter().map(|s| s.content.chars().count()).sum();

        // Truncate line content if needed to make room for indicator
        if content_width + indicator_width > width {
            truncate_spans(&mut spans, width.saturating_sub(indicator_width + 1));
            spans.push(Span::styled(
                "\u{2026}", // …
                Style::default().fg(app.theme.dim).bg(bg),
            ));
        }

        let hl_style = Style::default()
            .fg(app.theme.search_match_fg)
            .bg(app.theme.search_match_bg)
            .add_modifier(Modifier::BOLD);
        spans.push(Span::styled("  ", Style::default().bg(bg)));
        spans.push(Span::styled(indicator, hl_style));
    }

    // Highlight cursor line, flash line, or selected line
    if is_cursor || is_flash || is_selected {
        let content_width: usize = spans.iter().map(|s| s.content.chars().count()).sum();
        if content_width < width {
            spans.push(Span::styled(
                " ".repeat(width - content_width),
                Style::default().bg(row_bg),
            ));
        }
        // Re-style all spans with row background
        // Skip column 0 border (already styled), search match spans (keep teal bg),
        // text selection spans (keep blue bg), and cursor spans (keep text_bright bg)
        let search_bg = app.theme.search_match_bg;
        let selection_blue = app.theme.blue;
        let cursor_bg = app.theme.text_bright;
        for span in spans.iter_mut().skip(1) {
            let bg_color = span.style.bg;
            if bg_color != Some(search_bg)
                && bg_color != Some(selection_blue)
                && bg_color != Some(cursor_bg)
            {
                span.style = span.style.bg(row_bg);
            }
        }
    }

    (Line::from(spans), edit_col)
}

/// Get the abbreviated ID for a subtask (e.g., ".1", ".2.1")
fn abbreviated_id(task: &Task) -> Option<String> {
    let id = task.id.as_deref()?;
    // Find the last segment after the prefix-NUM, e.g., "EFF-014.2.1" → ".2.1"
    let dash_pos = id.find('-')?;
    let after_prefix = &id[dash_pos + 1..];
    let dot_pos = after_prefix.find('.')?;
    Some(after_prefix[dot_pos..].to_string())
}

/// Render the bulk inline editor line (tags:/deps: label + edit buffer + cursor).
/// Returns the line and the column offset of the edit text start (for autocomplete anchor).
fn render_bulk_editor_line<'a>(app: &'a App, label: &str, width: usize) -> (Line<'a>, u16) {
    let bg = app.theme.background;
    let mut spans: Vec<Span> = Vec::new();

    // Indent + label
    spans.push(Span::styled("  ", Style::default().bg(bg)));
    spans.push(Span::styled(
        format!("{} ", label),
        Style::default().fg(app.theme.dim).bg(bg),
    ));

    let prefix_width: usize = spans.iter().map(|s| s.content.chars().count()).sum();
    let edit_col = prefix_width as u16;

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
        let cursor_char = &buf[cursor_pos..cursor_pos + 1];
        spans.push(Span::styled(cursor_char.to_string(), cursor_style));
        let after = &buf[cursor_pos + 1..];
        if !after.is_empty() {
            spans.push(Span::styled(after.to_string(), title_style));
        }
    } else {
        spans.push(Span::styled(" ".to_string(), cursor_style));
    }

    // Fill remaining width
    let content_width: usize = spans.iter().map(|s| s.content.chars().count()).sum();
    if content_width < width {
        spans.push(Span::styled(
            " ".repeat(width - content_width),
            Style::default().bg(bg),
        ));
    }

    (Line::from(spans), edit_col)
}

/// Render the parked section separator
fn render_parked_separator<'a>(app: &'a App, width: usize, is_cursor: bool) -> Line<'a> {
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
fn render_bulk_standin<'a>(app: &'a App, count: usize, width: usize) -> Line<'a> {
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

/// Truncate spans to fit within `max_width` characters.
fn truncate_spans(spans: &mut Vec<Span<'_>>, max_width: usize) {
    let mut total = 0usize;
    let mut truncate_at = spans.len();

    for (i, span) in spans.iter().enumerate() {
        let span_width = span.content.chars().count();
        if total + span_width > max_width {
            truncate_at = i;
            // Truncate this span's content to fit
            let remaining = max_width.saturating_sub(total);
            if remaining > 0 {
                let truncated: String = span.content.chars().take(remaining).collect();
                spans[i] = Span::styled(truncated, span.style);
                truncate_at = i + 1;
            }
            break;
        }
        total += span_width;
    }

    spans.truncate(truncate_at);
}
