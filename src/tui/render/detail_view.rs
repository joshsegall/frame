use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use regex::Regex;

use crate::model::{Metadata, Task, TaskState};
use crate::ops::task_ops;
use crate::tui::app::{App, DetailRegion, Mode, ReturnView, View, flatten_subtask_ids};
use crate::tui::input::{multiline_selection_range, selection_cols_for_line};
use crate::tui::theme::Theme;

use super::push_highlighted_spans;

/// Render the detail view for a single task
pub fn render_detail_view(frame: &mut Frame, app: &mut App, area: Rect) {
    let (track_id, task_id) = match &app.view {
        View::Detail { track_id, task_id } => (track_id.clone(), task_id.clone()),
        _ => return,
    };

    let track = match App::find_track_in_project(&app.project, &track_id) {
        Some(t) => t,
        None => {
            let empty = Paragraph::new(" Task not found")
                .style(Style::default().fg(app.theme.dim).bg(app.theme.background));
            frame.render_widget(empty, area);
            return;
        }
    };

    let task = match task_ops::find_task_in_track(track, &task_id) {
        Some(t) => t,
        None => {
            let empty = Paragraph::new(" Task not found")
                .style(Style::default().fg(app.theme.dim).bg(app.theme.background));
            frame.render_widget(empty, area);
            return;
        }
    };

    // Rebuild regions from current task state
    let regions = App::build_detail_regions(task);
    // Rebuild flat_subtask_ids
    let flat_subtask_ids = flatten_subtask_ids(task);
    if let Some(ref mut ds) = app.detail_state {
        ds.regions = regions.clone();
        ds.regions_populated = regions
            .iter()
            .map(|r| App::is_detail_region_populated(task, *r))
            .collect();
        // Clamp region to valid range
        if !regions.contains(&ds.region) {
            ds.region = regions.first().copied().unwrap_or(DetailRegion::Title);
        }
        ds.flat_subtask_ids = flat_subtask_ids;
        // Clamp subtask_cursor
        if !ds.flat_subtask_ids.is_empty() {
            ds.subtask_cursor = ds.subtask_cursor.min(ds.flat_subtask_ids.len() - 1);
        } else {
            ds.subtask_cursor = 0;
        }
    }

    let is_flashing = app.is_flashing(&task_id);

    let detail_state = app.detail_state.as_ref();
    let current_region = detail_state
        .map(|ds| ds.region)
        .unwrap_or(DetailRegion::Title);
    let editing = detail_state.is_some_and(|ds| ds.editing);
    let selected_subtask_id = detail_state.and_then(|ds| {
        if ds.region == DetailRegion::Subtasks {
            ds.flat_subtask_ids.get(ds.subtask_cursor).cloned()
        } else {
            None
        }
    });

    let bg = app.theme.background;
    let text_style = Style::default().fg(app.theme.text).bg(bg);
    let bright_style = Style::default().fg(app.theme.text_bright).bg(bg);
    let dim_style = Style::default().fg(app.theme.dim).bg(bg);
    let region_indicator_style = Style::default().fg(app.theme.highlight).bg(bg);

    // Search highlighting
    let search_re = app.active_search_re();
    let highlight_style = Style::default()
        .fg(app.theme.search_match_fg)
        .bg(app.theme.search_match_bg)
        .add_modifier(Modifier::BOLD);

    let width = area.width as usize;

    // -----------------------------------------------------------------------
    // Build HEADER lines (fixed, non-scrolling): blank + breadcrumb + title + blank separator
    // -----------------------------------------------------------------------
    let mut header_lines: Vec<Line<'static>> = Vec::new();
    let mut header_active_line: Option<usize> = None;
    // Always track title line range for flash (independent of cursor region)
    #[allow(unused_assignments)]
    let mut title_line_start: usize = 0;
    #[allow(unused_assignments)]
    let mut title_line_end: usize = 0;

    // Blank line at top for breathing room
    header_lines.push(Line::from(""));

    // Breadcrumb trail: always visible, showing origin > [stack...] > current
    {
        let mut crumb_spans: Vec<Span> = Vec::new();
        crumb_spans.push(Span::styled("   ", Style::default().bg(bg)));

        // First crumb: origin view label
        let origin_label = match app
            .detail_state
            .as_ref()
            .map(|ds| &ds.return_view)
            .unwrap_or(&ReturnView::Track(0))
        {
            ReturnView::Recent => "Recent".to_string(),
            ReturnView::Track(idx) => {
                let tid = app.active_track_ids.get(*idx).cloned().unwrap_or_default();
                app.track_prefix(&tid).unwrap_or(&tid).to_string()
            }
        };
        crumb_spans.push(Span::styled(origin_label, dim_style));

        // Middle crumbs: from detail_stack
        for (_stack_track, stack_task) in app.detail_stack.iter() {
            crumb_spans.push(Span::styled(" > ", dim_style));
            crumb_spans.push(Span::styled(stack_task.clone(), dim_style));
        }

        // Last crumb: current task ID (bright)
        crumb_spans.push(Span::styled(" > ", dim_style));
        crumb_spans.push(Span::styled(
            task_id.clone(),
            Style::default().fg(app.theme.text).bg(bg),
        ));

        header_lines.push(Line::from(crumb_spans));
        header_lines.push(Line::from(""));
    }

    // --- Title region (in header) ---
    {
        let is_active = current_region == DetailRegion::Title;
        if is_active {
            header_active_line = Some(header_lines.len());
        }
        let state_color = app.theme.state_color(task.state);
        let state_sym = state_symbol(task.state);
        let mut spans: Vec<Span> = Vec::new();

        // Region indicator
        spans.push(region_indicator(is_active, region_indicator_style, bg));

        // State symbol
        spans.push(Span::styled(
            state_sym,
            Style::default()
                .fg(state_color)
                .bg(bg)
                .add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(" ", Style::default().bg(bg)));

        // ID
        if let Some(ref id) = task.id {
            push_highlighted_spans(
                &mut spans,
                &format!("{} ", id),
                text_style,
                highlight_style,
                search_re.as_ref(),
            );
        }

        // Title
        if is_active && editing && app.mode == Mode::Edit {
            // Render edit buffer with cursor
            let (aw, hs) = render_edit_inline_scrolled(&mut spans, app, bright_style, width);
            app.last_edit_available_width = aw;
            app.edit_h_scroll = hs;
            title_line_start = header_lines.len();
            title_line_end = title_line_start;
            header_lines.push(Line::from(spans));
        } else {
            push_highlighted_spans(
                &mut spans,
                &task.title,
                bright_style.add_modifier(Modifier::BOLD),
                highlight_style,
                search_re.as_ref(),
            );
            let wrapped = wrap_styled_spans(spans, width, 3, bg);
            let start = header_lines.len();
            header_lines.extend(wrapped);
            title_line_start = start;
            title_line_end = header_lines.len().saturating_sub(1);
            if is_active {
                header_active_line = Some(start);
            }
        }
    }

    // Blank line before metadata fields (end of header)
    header_lines.push(Line::from(""));

    // -----------------------------------------------------------------------
    // Build BODY lines (scrollable): tags through subtasks
    // -----------------------------------------------------------------------
    let mut body_lines: Vec<Line<'static>> = Vec::new();
    let mut body_active_line: Option<usize> = None;
    let mut edit_anchor_col: Option<u16> = None;
    let mut edit_anchor_line: Option<usize> = None; // index into body_lines
    #[allow(unused_assignments)]
    let mut note_header_idx: usize = 0; // index into body_lines
    // Track body line ranges per region for targeted flash
    let mut region_line_ranges: std::collections::HashMap<DetailRegion, (usize, usize)> =
        std::collections::HashMap::new();

    // --- Tags region ---
    {
        let region_start = body_lines.len();
        let is_active = current_region == DetailRegion::Tags;
        if is_active {
            body_active_line = Some(body_lines.len());
        }
        let mut spans: Vec<Span> = Vec::new();
        spans.push(region_indicator(is_active, region_indicator_style, bg));
        spans.push(Span::styled("tags: ", dim_style));

        if is_active && editing && app.mode == Mode::Edit {
            edit_anchor_col = Some(spans.iter().map(|s| s.content.chars().count() as u16).sum());
            edit_anchor_line = Some(body_lines.len());
            let (aw, hs) = render_edit_inline_scrolled(&mut spans, app, bright_style, width);
            app.last_edit_available_width = aw;
            app.edit_h_scroll = hs;
            body_lines.push(Line::from(spans));
        } else if task.tags.is_empty() {
            spans.push(Span::styled("(none)", dim_style));
            body_lines.push(Line::from(spans));
        } else {
            for (i, tag) in task.tags.iter().enumerate() {
                let tag_color = app.theme.tag_color(tag);
                let tag_style = Style::default().fg(tag_color).bg(bg);
                if i > 0 {
                    spans.push(Span::styled(" ", Style::default().bg(bg)));
                }
                push_highlighted_spans(
                    &mut spans,
                    &format!("#{}", tag),
                    tag_style,
                    highlight_style,
                    search_re.as_ref(),
                );
            }
            let wrapped = wrap_styled_spans(spans, width, 9, bg);
            let start = body_lines.len();
            body_lines.extend(wrapped);
            if is_active {
                body_active_line = Some(start);
            }
        }
        if body_lines.len() > region_start {
            region_line_ranges.insert(DetailRegion::Tags, (region_start, body_lines.len() - 1));
        }
    }

    // --- Added region ---
    for meta in &task.metadata {
        if let Metadata::Added(date) = meta {
            let is_active = current_region == DetailRegion::Added;
            if is_active {
                body_active_line = Some(body_lines.len());
            }
            let spans: Vec<Span> = vec![
                region_indicator(is_active, region_indicator_style, bg),
                Span::styled("added: ", dim_style),
                Span::styled(date.clone(), text_style),
            ];
            body_lines.push(Line::from(spans));
            break;
        }
    }

    // --- Deps region ---
    {
        let region_start = body_lines.len();
        let is_active = current_region == DetailRegion::Deps;
        if is_active {
            body_active_line = Some(body_lines.len());
        }
        let deps = collect_deps(task);

        if is_active && editing && app.mode == Mode::Edit {
            let mut spans: Vec<Span> = Vec::new();
            spans.push(region_indicator(is_active, region_indicator_style, bg));
            spans.push(Span::styled("dep: ", dim_style));
            edit_anchor_col = Some(spans.iter().map(|s| s.content.chars().count() as u16).sum());
            edit_anchor_line = Some(body_lines.len());
            let (aw, hs) = render_edit_inline_scrolled(&mut spans, app, bright_style, width);
            app.last_edit_available_width = aw;
            app.edit_h_scroll = hs;
            body_lines.push(Line::from(spans));
        } else if !deps.is_empty() {
            let mut spans: Vec<Span> = Vec::new();
            spans.push(region_indicator(is_active, region_indicator_style, bg));
            spans.push(Span::styled("dep: ", dim_style));

            for (i, dep_id) in deps.iter().enumerate() {
                if i > 0 {
                    spans.push(Span::styled(", ", dim_style));
                }
                // Look up dep state for inline symbol
                let dep_state = find_task_state_across_tracks(app, dep_id);
                let dep_style = if let Some(state) = dep_state {
                    Style::default().fg(app.theme.state_color(state)).bg(bg)
                } else {
                    text_style
                };
                push_highlighted_spans(
                    &mut spans,
                    dep_id,
                    text_style,
                    highlight_style,
                    search_re.as_ref(),
                );
                if let Some(state) = dep_state {
                    spans.push(Span::styled(
                        format!(" {}", state_symbol_short(state)),
                        dep_style,
                    ));
                }
            }
            let wrapped = wrap_styled_spans(spans, width, 8, bg);
            let start = body_lines.len();
            body_lines.extend(wrapped);
            if is_active {
                body_active_line = Some(start);
            }
        } else if is_active {
            let spans: Vec<Span> = vec![
                region_indicator(is_active, region_indicator_style, bg),
                Span::styled("dep: ", dim_style),
                Span::styled("(none)", dim_style),
            ];
            body_lines.push(Line::from(spans));
        }
        if body_lines.len() > region_start {
            region_line_ranges.insert(DetailRegion::Deps, (region_start, body_lines.len() - 1));
        }
    }

    // --- Spec region ---
    {
        let region_start = body_lines.len();
        let is_active = current_region == DetailRegion::Spec;
        if is_active {
            body_active_line = Some(body_lines.len());
        }
        let spec = task.metadata.iter().find_map(|m| {
            if let Metadata::Spec(s) = m {
                Some(s.clone())
            } else {
                None
            }
        });

        if is_active && editing && app.mode == Mode::Edit {
            let mut spans: Vec<Span> = Vec::new();
            spans.push(region_indicator(is_active, region_indicator_style, bg));
            spans.push(Span::styled("spec: ", dim_style));
            edit_anchor_col = Some(spans.iter().map(|s| s.content.chars().count() as u16).sum());
            edit_anchor_line = Some(body_lines.len());
            let (aw, hs) = render_edit_inline_scrolled(&mut spans, app, bright_style, width);
            app.last_edit_available_width = aw;
            app.edit_h_scroll = hs;
            body_lines.push(Line::from(spans));
        } else if let Some(spec_val) = &spec {
            let mut spans: Vec<Span> = vec![
                region_indicator(is_active, region_indicator_style, bg),
                Span::styled("spec: ", dim_style),
            ];
            let cyan_style = Style::default().fg(app.theme.cyan).bg(bg);
            push_highlighted_spans(
                &mut spans,
                spec_val,
                cyan_style,
                highlight_style,
                search_re.as_ref(),
            );
            let wrapped = wrap_styled_spans(spans, width, 9, bg);
            let start = body_lines.len();
            body_lines.extend(wrapped);
            if is_active {
                body_active_line = Some(start);
            }
        } else if is_active {
            let spans: Vec<Span> = vec![
                region_indicator(is_active, region_indicator_style, bg),
                Span::styled("spec: ", dim_style),
                Span::styled("(none)", dim_style),
            ];
            body_lines.push(Line::from(spans));
        }
        if body_lines.len() > region_start {
            region_line_ranges.insert(DetailRegion::Spec, (region_start, body_lines.len() - 1));
        }
    }

    // --- Refs region ---
    {
        let region_start = body_lines.len();
        let is_active = current_region == DetailRegion::Refs;
        if is_active {
            body_active_line = Some(body_lines.len());
        }
        let refs = collect_refs(task);

        if is_active && editing && app.mode == Mode::Edit {
            let mut spans: Vec<Span> = Vec::new();
            spans.push(region_indicator(is_active, region_indicator_style, bg));
            spans.push(Span::styled("ref: ", dim_style));
            edit_anchor_col = Some(spans.iter().map(|s| s.content.chars().count() as u16).sum());
            edit_anchor_line = Some(body_lines.len());
            let (aw, hs) = render_edit_inline_scrolled(&mut spans, app, bright_style, width);
            app.last_edit_available_width = aw;
            app.edit_h_scroll = hs;
            body_lines.push(Line::from(spans));
        } else if !refs.is_empty() {
            let start = body_lines.len();
            for (i, ref_path) in refs.iter().enumerate() {
                let mut spans: Vec<Span> = Vec::new();
                spans.push(region_indicator(
                    is_active && i == 0,
                    region_indicator_style,
                    bg,
                ));
                if i == 0 {
                    spans.push(Span::styled("ref: ", dim_style));
                } else {
                    spans.push(Span::styled("     ", dim_style));
                }
                let cyan_style = Style::default().fg(app.theme.cyan).bg(bg);
                push_highlighted_spans(
                    &mut spans,
                    ref_path,
                    cyan_style,
                    highlight_style,
                    search_re.as_ref(),
                );
                let wrapped = wrap_styled_spans(spans, width, 8, bg);
                body_lines.extend(wrapped);
            }
            if is_active {
                body_active_line = Some(start);
            }
        } else if is_active {
            let spans: Vec<Span> = vec![
                region_indicator(is_active, region_indicator_style, bg),
                Span::styled("ref: ", dim_style),
                Span::styled("(none)", dim_style),
            ];
            body_lines.push(Line::from(spans));
        }
        if body_lines.len() > region_start {
            region_line_ranges.insert(DetailRegion::Refs, (region_start, body_lines.len() - 1));
        }
    }

    // Blank line before note
    body_lines.push(Line::from(""));

    let mut note_gutter_width: usize = 3;

    // --- Note region ---
    {
        let is_active = current_region == DetailRegion::Note;
        note_header_idx = body_lines.len();
        if is_active {
            body_active_line = Some(note_header_idx);
        }
        let note = task.metadata.iter().find_map(|m| {
            if let Metadata::Note(n) = m {
                Some(n.clone())
            } else {
                None
            }
        });

        if is_active && editing && app.mode == Mode::Edit {
            // Compute gutter width from edit buffer line count
            let edit_line_count = app
                .detail_state
                .as_ref()
                .map(|ds| ds.edit_buffer.split('\n').count())
                .unwrap_or(1);
            let num_width = edit_line_count.max(1).to_string().len();
            let gutter_width = (num_width + 1).max(3);
            let num_display_width = gutter_width - 1;
            note_gutter_width = gutter_width;

            // Multi-line editing mode
            let header_prefix = format!("{}\u{258E} ", " ".repeat(gutter_width.saturating_sub(2)));
            let header_spans: Vec<Span> = vec![
                Span::styled(header_prefix, region_indicator_style),
                Span::styled("note:", dim_style),
            ];
            body_lines.push(Line::from(header_spans));

            // Render the multi-line edit buffer with horizontal scrolling
            let note_available = width.saturating_sub(gutter_width);
            let mut adjusted_h_scroll = 0usize;
            if let Some(ref ds) = app.detail_state {
                let cursor_style = Style::default()
                    .fg(app.theme.background)
                    .bg(app.theme.text_bright);
                let selection_style = Style::default()
                    .fg(app.theme.text_bright)
                    .bg(app.theme.blue);
                let dim_arrow_style = Style::default().fg(app.theme.dim).bg(bg);

                // Auto-adjust note_h_scroll to keep cursor column visible
                let mut h_scroll = ds.note_h_scroll;
                let cursor_col = ds.edit_cursor_col;
                if note_available > 0 {
                    let margin = 10.min(note_available / 3);
                    // Account for cursor-at-end needing one extra column
                    let edit_lines_vec: Vec<&str> = ds.edit_buffer.split('\n').collect();
                    let cursor_line_len = edit_lines_vec
                        .get(ds.edit_cursor_line)
                        .map_or(0, |l| l.len());
                    let content_end = if cursor_col >= cursor_line_len {
                        cursor_line_len + 1
                    } else {
                        cursor_line_len
                    };
                    if cursor_col >= h_scroll + note_available.saturating_sub(margin) {
                        h_scroll =
                            cursor_col.saturating_sub(note_available.saturating_sub(margin + 1));
                    }
                    h_scroll =
                        h_scroll.min(content_end.saturating_sub(note_available.saturating_sub(1)));
                    if cursor_col < h_scroll + margin {
                        h_scroll = cursor_col.saturating_sub(margin);
                    }
                }
                // Persist adjusted h_scroll (written back after this block)
                adjusted_h_scroll = h_scroll;

                // Compute selection range (absolute offsets) if any
                let sel_range = multiline_selection_range(ds);

                let edit_lines: Vec<&str> = ds.edit_buffer.split('\n').collect();
                for (line_idx, edit_line) in edit_lines.iter().enumerate() {
                    let mut spans: Vec<Span> = Vec::new();

                    let has_cursor = line_idx == ds.edit_cursor_line;
                    if has_cursor {
                        body_active_line = Some(body_lines.len());
                    }

                    let line_chars: Vec<char> = edit_line.chars().collect();
                    let total_line_chars = line_chars.len();
                    let clipped_left = h_scroll > 0 && total_line_chars > 0;
                    let left_indicator = if clipped_left { 1 } else { 0 };
                    let avail_after_left = note_available.saturating_sub(left_indicator);
                    let clipped_right = h_scroll + avail_after_left < total_line_chars;
                    let right_indicator = if clipped_right { 1 } else { 0 };
                    let view_chars = avail_after_left.saturating_sub(right_indicator);
                    let view_start = h_scroll.min(total_line_chars);
                    let view_end = (view_start + view_chars).min(total_line_chars);

                    // Line number + indent (may show left arrow)
                    let line_num_str =
                        format!("{:>width$}", line_idx + 1, width = num_display_width);
                    if clipped_left {
                        spans.push(Span::styled(line_num_str, text_style));
                        spans.push(Span::styled("\u{25C2}", dim_arrow_style)); // ◂
                    } else {
                        spans.push(Span::styled(format!("{} ", line_num_str), text_style));
                    }

                    let line_sel = sel_range.and_then(|(s, e)| {
                        selection_cols_for_line(&ds.edit_buffer, s, e, line_idx)
                    });

                    // Render the visible viewport of this line
                    if let Some((sc, ec)) = line_sel {
                        // Selection on this line — render char by char within viewport
                        for (i, ch) in line_chars
                            .iter()
                            .enumerate()
                            .skip(view_start)
                            .take(view_end - view_start)
                        {
                            let s = if i >= sc && i < ec {
                                selection_style
                            } else {
                                bright_style
                            };
                            spans.push(Span::styled(ch.to_string(), s));
                        }
                        // Blank line in selection: show a one-column indicator
                        if sc == ec && total_line_chars == 0 && !has_cursor {
                            spans.push(Span::styled(" ", selection_style));
                        }
                        if has_cursor && cursor_col >= total_line_chars && cursor_col >= view_start
                        {
                            spans.push(Span::styled(" ", cursor_style));
                        }
                    } else if has_cursor {
                        // Cursor on this line, no selection
                        let col = cursor_col.min(total_line_chars);
                        for (i, ch) in line_chars
                            .iter()
                            .enumerate()
                            .skip(view_start)
                            .take(view_end - view_start)
                        {
                            if i == col {
                                spans.push(Span::styled(ch.to_string(), cursor_style));
                            } else {
                                spans.push(Span::styled(ch.to_string(), bright_style));
                            }
                        }
                        if col >= total_line_chars && col >= view_start {
                            spans.push(Span::styled(" ", cursor_style));
                        }
                    } else {
                        // No selection, no cursor — render slice
                        if view_start < view_end {
                            let slice: String = line_chars[view_start..view_end].iter().collect();
                            spans.push(Span::styled(slice, bright_style));
                        }
                    }

                    // Right clip indicator
                    if clipped_right {
                        spans.push(Span::styled("\u{25B8}", dim_arrow_style)); // ▸
                    }

                    body_lines.push(Line::from(spans));
                }
            }
            // Write back adjusted h_scroll (after immutable borrow of detail_state is released)
            if let Some(ds_mut) = app.detail_state.as_mut() {
                ds_mut.note_h_scroll = adjusted_h_scroll;
            }
        } else if let Some(note_text) = &note {
            // Compute gutter width from note line count
            let line_count = note_text.lines().count();
            let num_width = line_count.max(1).to_string().len();
            let gutter_width = (num_width + 1).max(3);
            let num_display_width = gutter_width - 1;
            note_gutter_width = gutter_width;

            let header_prefix = if is_active {
                format!("{}\u{258E} ", " ".repeat(gutter_width.saturating_sub(2)))
            } else {
                " ".repeat(gutter_width)
            };
            let header_spans: Vec<Span> = vec![
                Span::styled(
                    header_prefix,
                    if is_active {
                        region_indicator_style
                    } else {
                        Style::default().bg(bg)
                    },
                ),
                Span::styled("note:", dim_style),
            ];
            body_lines.push(Line::from(header_spans));

            let line_num_style = Style::default().fg(app.theme.dim).bg(bg);
            let mut in_code_block = false;
            for (line_num, note_line) in note_text.lines().enumerate() {
                let trimmed = note_line.trim();
                if trimmed.starts_with("```") {
                    in_code_block = !in_code_block;
                }

                let num_str = format!("{:>width$} ", line_num + 1, width = num_display_width);
                if in_code_block || trimmed.starts_with("```") {
                    let code_style = Style::default().fg(app.theme.dim).bg(bg);
                    let mut spans: Vec<Span> = vec![Span::styled(num_str, line_num_style)];
                    push_highlighted_spans(
                        &mut spans,
                        note_line,
                        code_style,
                        highlight_style,
                        search_re.as_ref(),
                    );
                    body_lines.push(Line::from(spans));
                } else {
                    // Wrap content separately to keep line number as a single span
                    let mut content_spans: Vec<Span> = Vec::new();
                    push_highlighted_spans(
                        &mut content_spans,
                        note_line,
                        text_style,
                        highlight_style,
                        search_re.as_ref(),
                    );
                    let content_width = width.saturating_sub(gutter_width);
                    let wrapped = wrap_styled_spans(content_spans, content_width, 0, bg);
                    for (i, wrapped_line) in wrapped.into_iter().enumerate() {
                        let prefix = if i == 0 {
                            Span::styled(num_str.clone(), line_num_style)
                        } else {
                            Span::styled(" ".repeat(gutter_width), Style::default().bg(bg))
                        };
                        let mut line_spans = vec![prefix];
                        line_spans.extend(wrapped_line.spans);
                        body_lines.push(Line::from(line_spans));
                    }
                }
            }
        } else if is_active {
            let spans: Vec<Span> = vec![
                region_indicator(is_active, region_indicator_style, bg),
                Span::styled("note: ", dim_style),
                Span::styled("(empty)", dim_style),
            ];
            body_lines.push(Line::from(spans));
        }
    }
    let note_content_end_idx = body_lines.len().saturating_sub(1);
    if note_content_end_idx >= note_header_idx {
        region_line_ranges.insert(DetailRegion::Note, (note_header_idx, note_content_end_idx));
    }

    // --- Subtasks region ---
    if !task.subtasks.is_empty() {
        body_lines.push(Line::from(""));
        let is_active = current_region == DetailRegion::Subtasks;
        if is_active && selected_subtask_id.is_none() {
            // Only set header as active if no subtask is selected
            body_active_line = Some(body_lines.len());
        }

        let mut header_spans: Vec<Span> = Vec::new();
        let header_indicator = is_active && selected_subtask_id.is_none();
        header_spans.push(region_indicator(
            header_indicator,
            region_indicator_style,
            bg,
        ));
        header_spans.push(Span::styled(
            "Subtasks",
            bright_style.add_modifier(Modifier::BOLD),
        ));
        body_lines.push(Line::from(header_spans));

        let subtask_selected_line = render_subtask_tree(
            &mut body_lines,
            app,
            &task.subtasks,
            1,
            width,
            bg,
            selected_subtask_id.as_deref(),
            search_re.as_ref(),
            highlight_style,
        );
        // When a subtask is selected, use its line for scroll tracking
        if is_active && let Some(sl) = subtask_selected_line {
            body_active_line = Some(sl);
        }
    }

    // -----------------------------------------------------------------------
    // Apply flash/highlight/note decorations
    // -----------------------------------------------------------------------

    let (flash_bg, flash_border) = match app.flash_state {
        Some(state) => state_flash_colors(state, &app.theme),
        None => UNDO_FLASH_COLORS,
    };

    // Flash highlights the edited region: body region for field edits, header for state changes
    if is_flashing {
        if let Some(region) = app.flash_detail_region {
            if let Some(&(start, end)) = region_line_ranges.get(&region) {
                apply_flash_to_lines(&mut body_lines, start, end, flash_bg, flash_border, width);
            } else {
                // Region not visible — fall back to header
                apply_flash_to_lines(
                    &mut header_lines,
                    title_line_start,
                    title_line_end,
                    flash_bg,
                    flash_border,
                    width,
                );
            }
        } else {
            apply_flash_to_lines(
                &mut header_lines,
                title_line_start,
                title_line_end,
                flash_bg,
                flash_border,
                width,
            );
        }
    }

    // Store total lines, note header, and note content end for input handler use
    // (body-relative indices, since that's what the input handler scrolls through)
    if let Some(ds) = app.detail_state.as_mut() {
        ds.total_lines = body_lines.len();
        ds.note_header_line = Some(note_header_idx);
        ds.note_content_end = note_content_end_idx;
    }

    // If note_view_line is set, override body_active_line with the virtual cursor
    // and move the region indicator to that line (only within note content bounds)
    if let Some(ds) = app.detail_state.as_ref()
        && let Some(vl) = ds.note_view_line
    {
        let clamped = vl.min(note_content_end_idx);
        body_active_line = Some(clamped);
        // Replace the leading gutter with the cursor indicator on the target line
        let cursor_indicator = format!(
            "{}\u{258E} ",
            " ".repeat(note_gutter_width.saturating_sub(2))
        );
        if let Some(line) = body_lines.get_mut(clamped) {
            let mut new_spans: Vec<Span<'static>> = Vec::new();
            new_spans.push(Span::styled(
                cursor_indicator.clone(),
                region_indicator_style,
            ));
            // Skip the first span if it's the gutter (line number or blank padding)
            let mut skipped_prefix = false;
            for span in line.spans.iter() {
                if !skipped_prefix {
                    let content_len = span.content.chars().count();
                    if content_len == note_gutter_width {
                        skipped_prefix = true;
                        continue;
                    }
                }
                new_spans.push(span.clone());
            }
            *line = Line::from(new_spans);
        }
        // Remove indicator from the note header line (if different)
        if clamped != note_header_idx
            && let Some(line) = body_lines.get_mut(note_header_idx)
        {
            let mut new_spans: Vec<Span<'static>> = Vec::new();
            for span in line.spans.iter() {
                if span.content.contains('\u{258E}') {
                    new_spans.push(Span::styled(
                        " ".repeat(note_gutter_width),
                        Style::default().bg(bg),
                    ));
                } else {
                    new_spans.push(span.clone());
                }
            }
            *line = Line::from(new_spans);
        }
    }

    // Apply active-region highlight: selection_bg on the cursor line, pad to full width
    let is_editing = app.detail_state.as_ref().is_some_and(|ds| ds.editing);
    let on_subtask_region =
        current_region == DetailRegion::Subtasks && selected_subtask_id.is_some();
    if !is_flashing && !is_editing && !on_subtask_region {
        // Apply to header (Title region) or body (all other regions)
        let (target_lines, target_line) = if current_region == DetailRegion::Title {
            (
                &mut header_lines as &mut Vec<Line<'static>>,
                header_active_line,
            )
        } else {
            (&mut body_lines as &mut Vec<Line<'static>>, body_active_line)
        };
        if let Some(rl) = target_line {
            let sel_bg = app.theme.selection_bg;
            if let Some(line) = target_lines.get_mut(rl) {
                let mut new_spans: Vec<Span<'static>> = Vec::new();
                for span in line.spans.drain(..) {
                    let content = span.content.into_owned();
                    let is_indicator = content.contains('\u{258E}');
                    let is_search_match = span.style.bg == Some(app.theme.search_match_bg);
                    let new_style = if is_search_match {
                        // Preserve search highlight styling on the selection line
                        span.style
                    } else if is_indicator {
                        Style::default().fg(app.theme.selection_border).bg(sel_bg)
                    } else {
                        // Brighten text on the selection line
                        let fg = if span.style.fg == Some(app.theme.dim)
                            || span.style.fg == Some(app.theme.text)
                        {
                            app.theme.text_bright
                        } else {
                            span.style.fg.unwrap_or(app.theme.text_bright)
                        };
                        let mut s = Style::default().fg(fg).bg(sel_bg);
                        if span.style.add_modifier.contains(Modifier::BOLD) {
                            s = s.add_modifier(Modifier::BOLD);
                        }
                        s
                    };
                    new_spans.push(Span::styled(content, new_style));
                }
                let content_width: usize =
                    new_spans.iter().map(|s| s.content.chars().count()).sum();
                if content_width < width {
                    new_spans.push(Span::styled(
                        " ".repeat(width - content_width),
                        Style::default().bg(sel_bg),
                    ));
                }
                *line = Line::from(new_spans);
            }
        }
    }

    // When Note region is active (not editing), brighten note body text
    let is_note_active = current_region == DetailRegion::Note && !is_editing;
    if is_note_active {
        // Brighten all note body lines (from header+1 to note_content_end)
        let note_body_start = note_header_idx + 1;
        for line_idx in note_body_start..=note_content_end_idx {
            // Skip the cursor line (already highlighted above)
            if body_active_line == Some(line_idx) {
                continue;
            }
            if let Some(line) = body_lines.get_mut(line_idx) {
                let mut new_spans: Vec<Span<'static>> = Vec::new();
                for (span_idx, span) in line.spans.drain(..).enumerate() {
                    let content = span.content.into_owned();
                    if span_idx == 0 {
                        // Keep line number gutter dim
                        new_spans.push(Span::styled(content, span.style));
                    } else {
                        let fg = if span.style.fg == Some(app.theme.text)
                            || span.style.fg == Some(app.theme.dim)
                        {
                            app.theme.text_bright
                        } else {
                            span.style.fg.unwrap_or(app.theme.text_bright)
                        };
                        new_spans.push(Span::styled(content, span.style.fg(fg)));
                    }
                }
                *line = Line::from(new_spans);
            }
        }
    }

    // -----------------------------------------------------------------------
    // Layout: split area into header (fixed) and body (scrollable)
    // -----------------------------------------------------------------------
    let header_height = header_lines.len().min(area.height as usize);
    let header_area = Rect::new(area.x, area.y, area.width, header_height as u16);
    let body_area = Rect::new(
        area.x,
        area.y + header_height as u16,
        area.width,
        area.height.saturating_sub(header_height as u16),
    );

    // Render header (no scroll)
    let header_paragraph = Paragraph::new(header_lines).style(Style::default().bg(bg));
    frame.render_widget(header_paragraph, header_area);

    // Handle body scrolling using tracked body region line index with margin-aware logic
    let body_visible_height = body_area.height as usize;
    let body_total_lines = body_lines.len();

    let is_note_editing = app
        .detail_state
        .as_ref()
        .is_some_and(|ds| ds.editing && ds.region == DetailRegion::Note);
    let scroll_margin = if is_note_editing { 4 } else { 2 };

    if let (Some(ds), Some(rl)) = (&mut app.detail_state, body_active_line) {
        let top_bound = ds.scroll_offset + scroll_margin;
        let bottom_bound = ds
            .scroll_offset
            .saturating_add(body_visible_height.saturating_sub(scroll_margin + 1));
        if rl < top_bound {
            ds.scroll_offset = rl.saturating_sub(scroll_margin);
        } else if rl > bottom_bound {
            ds.scroll_offset = (rl + scroll_margin + 1).saturating_sub(body_visible_height);
        }
    }

    let scroll = app
        .detail_state
        .as_ref()
        .map(|ds| ds.scroll_offset)
        .unwrap_or(0);

    let body_paragraph = Paragraph::new(body_lines)
        .style(Style::default().bg(bg))
        .scroll((scroll as u16, 0));
    frame.render_widget(body_paragraph, body_area);

    // Vertical scroll indicators (in body area)
    let dim_indicator_style = Style::default().fg(app.theme.dim).bg(bg);
    if scroll > 0 && body_area.height > 0 {
        let arrow = Paragraph::new("\u{25B2}").style(dim_indicator_style); // ▲
        frame.render_widget(
            arrow,
            Rect::new(body_area.right().saturating_sub(2), body_area.y, 1, 1),
        );
    }
    if scroll + body_visible_height < body_total_lines && body_area.height > 0 {
        let arrow = Paragraph::new("\u{25BC}").style(dim_indicator_style); // ▼
        frame.render_widget(
            arrow,
            Rect::new(
                body_area.right().saturating_sub(2),
                body_area.bottom().saturating_sub(1),
                1,
                1,
            ),
        );
    }

    // Set autocomplete anchor for detail view edits (body-relative)
    if let (Some(prefix_w), Some(line_idx)) = (edit_anchor_col, edit_anchor_line) {
        let word_offset = app
            .autocomplete
            .as_ref()
            .map(|ac| ac.word_start_in_buffer(&app.edit_buffer) as u16)
            .unwrap_or(0);
        let screen_y = body_area.y + line_idx.saturating_sub(scroll) as u16;
        let screen_x = body_area.x + prefix_w + word_offset;
        app.autocomplete_anchor = Some((screen_x, screen_y));
    } else if app.mode == Mode::Triage {
        // Cross-track move from detail view: anchor autocomplete to title in header area
        let screen_y = header_area.y + header_active_line.unwrap_or(0) as u16;
        let screen_x = header_area.x + 4;
        app.autocomplete_anchor = Some((screen_x, screen_y));
    }
}

/// Wrap styled spans across multiple lines, respecting word boundaries.
/// `continuation_indent` is the number of spaces to prepend on wrapped continuation lines.
fn wrap_styled_spans(
    spans: Vec<Span<'static>>,
    max_width: usize,
    continuation_indent: usize,
    bg: ratatui::style::Color,
) -> Vec<Line<'static>> {
    if max_width == 0 {
        return vec![Line::from(spans)];
    }

    let mut result_lines: Vec<Vec<Span<'static>>> = Vec::new();
    let mut current_line: Vec<Span<'static>> = Vec::new();
    let mut col: usize = 0; // current column position on this line

    for span in spans {
        let style = span.style;
        let text: &str = &span.content;

        if text.is_empty() {
            current_line.push(span);
            continue;
        }

        // Process this span's text character by character, splitting at word boundaries
        let mut remaining = text;
        while !remaining.is_empty() {
            // Find the next word boundary (whitespace vs non-whitespace transition)
            let chunk_end = if remaining.starts_with(char::is_whitespace) {
                // Whitespace chunk
                remaining
                    .find(|c: char| !c.is_whitespace())
                    .unwrap_or(remaining.len())
            } else {
                // Word chunk
                remaining
                    .find(char::is_whitespace)
                    .unwrap_or(remaining.len())
            };
            let chunk = &remaining[..chunk_end];
            let chunk_chars = chunk.chars().count();

            if col + chunk_chars <= max_width
                || col == 0
                || (col == continuation_indent && result_lines.is_empty())
            {
                // Fits on current line, or we're at the start (must place something)
                current_line.push(Span::styled(chunk.to_string(), style));
                col += chunk_chars;
            } else if chunk.starts_with(char::is_whitespace) {
                // Whitespace at wrap point — skip it and start new line
                result_lines.push(std::mem::take(&mut current_line));
                let indent_str = " ".repeat(continuation_indent);
                current_line.push(Span::styled(indent_str, Style::default().bg(bg)));
                col = continuation_indent;
                // Don't push the whitespace chunk
            } else {
                // Word doesn't fit — check if breaking mid-word is better
                let remaining_space = max_width.saturating_sub(col);
                let blank_fraction = if max_width > 0 {
                    remaining_space as f64 / max_width as f64
                } else {
                    0.0
                };

                if blank_fraction > 0.2 && remaining_space > 0 {
                    // Break mid-word: fill remaining space, continue on next line
                    let mut byte_pos = 0;
                    let mut chars_placed = 0;
                    for c in chunk.chars() {
                        if chars_placed >= remaining_space {
                            break;
                        }
                        byte_pos += c.len_utf8();
                        chars_placed += 1;
                    }
                    if byte_pos > 0 {
                        current_line.push(Span::styled(chunk[..byte_pos].to_string(), style));
                    }
                    // Start new line with remainder
                    result_lines.push(std::mem::take(&mut current_line));
                    let indent_str = " ".repeat(continuation_indent);
                    current_line.push(Span::styled(indent_str, Style::default().bg(bg)));
                    col = continuation_indent;
                    let rest = &chunk[byte_pos..];
                    if !rest.is_empty() {
                        current_line.push(Span::styled(rest.to_string(), style));
                        col += chunk_chars - chars_placed;
                    }
                } else {
                    // Word-wrap: start new line with this word
                    result_lines.push(std::mem::take(&mut current_line));
                    let indent_str = " ".repeat(continuation_indent);
                    current_line.push(Span::styled(indent_str, Style::default().bg(bg)));
                    col = continuation_indent;
                    current_line.push(Span::styled(chunk.to_string(), style));
                    col += chunk_chars;
                }
            }
            remaining = &remaining[chunk_end..];
        }
    }

    // Push the last line
    if !current_line.is_empty() {
        result_lines.push(current_line);
    }

    if result_lines.is_empty() {
        vec![Line::from("")]
    } else {
        result_lines.into_iter().map(Line::from).collect()
    }
}

/// Render the edit buffer inline with horizontal scrolling for the detail view.
/// Auto-adjusts h_scroll to keep the cursor visible.
/// Returns `(available_width, adjusted_h_scroll)` for the caller to persist on App.
fn render_edit_inline_scrolled(
    spans: &mut Vec<Span<'static>>,
    app: &App,
    style: Style,
    total_width: usize,
) -> (u16, usize) {
    let prefix_width: usize = spans.iter().map(|s| s.content.chars().count()).sum();
    let available = total_width.saturating_sub(prefix_width);

    if available == 0 {
        return (available as u16, app.edit_h_scroll);
    }

    let buf = &app.edit_buffer;
    let buf_chars: Vec<char> = buf.chars().collect();
    let total_chars = buf_chars.len();
    let cursor_char_pos = buf[..app.edit_cursor.min(buf.len())].chars().count();

    // Auto-adjust h_scroll to keep cursor visible.
    // When cursor is at the end, the cursor block needs one extra column.
    let content_end = if cursor_char_pos >= total_chars {
        total_chars + 1
    } else {
        total_chars
    };
    let mut h_scroll = app.edit_h_scroll;
    let margin = 10.min(available / 3);
    if cursor_char_pos >= h_scroll + available.saturating_sub(margin) {
        h_scroll = cursor_char_pos.saturating_sub(available.saturating_sub(margin + 1));
    }
    h_scroll = h_scroll.min(content_end.saturating_sub(available.saturating_sub(1)));
    if cursor_char_pos < h_scroll + margin {
        h_scroll = cursor_char_pos.saturating_sub(margin);
    }

    let clipped_left = h_scroll > 0;
    // Account for indicator characters in available space
    let view_start = h_scroll;
    let indicator_overhead = if clipped_left { 1 } else { 0 };
    let effective_available = available.saturating_sub(indicator_overhead);
    let clipped_right = view_start + effective_available < total_chars;
    let effective_available = if clipped_right {
        effective_available.saturating_sub(1)
    } else {
        effective_available
    };
    let view_end = (view_start + effective_available).min(total_chars);

    let dim_arrow_style = Style::default().fg(app.theme.dim).bg(app.theme.background);

    // Left clip indicator
    if clipped_left {
        spans.push(Span::styled("\u{25C2}", dim_arrow_style)); // ◂
    }

    // Build the visible portion with cursor/selection
    let cursor_style = Style::default()
        .fg(app.theme.background)
        .bg(app.theme.text_bright);
    let selection_style = Style::default()
        .fg(app.theme.text_bright)
        .bg(app.theme.blue);

    // Convert selection range from byte to char positions
    let sel_char_range = app.edit_selection_range().and_then(|(sb, se)| {
        if sb == se {
            return None;
        }
        let sc = app.edit_buffer[..sb].chars().count();
        let ec = app.edit_buffer[..se].chars().count();
        Some((sc, ec))
    });

    if let Some((sel_start, sel_end)) = sel_char_range {
        // Render with selection in the visible window
        for (i, ch) in buf_chars
            .iter()
            .enumerate()
            .skip(view_start)
            .take(view_end - view_start)
        {
            let s = if i >= sel_start && i < sel_end {
                selection_style
            } else {
                style
            };
            spans.push(Span::styled(ch.to_string(), s));
        }
        // Cursor block at end if cursor is past content
        if cursor_char_pos >= total_chars
            && cursor_char_pos >= view_start
            && cursor_char_pos < view_start + effective_available + 1
        {
            spans.push(Span::styled(" ", cursor_style));
        }
    } else {
        // No selection: render with cursor highlight
        for (i, ch) in buf_chars
            .iter()
            .enumerate()
            .skip(view_start)
            .take(view_end - view_start)
        {
            if i == cursor_char_pos {
                spans.push(Span::styled(ch.to_string(), cursor_style));
            } else {
                spans.push(Span::styled(ch.to_string(), style));
            }
        }
        // Cursor block at end if cursor is at/past end of visible content
        if cursor_char_pos >= total_chars && cursor_char_pos >= view_start {
            spans.push(Span::styled(" ", cursor_style));
        }
    }

    // Right clip indicator
    if clipped_right {
        spans.push(Span::styled("\u{25B8}", dim_arrow_style)); // ▸
    }

    (available as u16, h_scroll)
}

/// Render subtask tree for the detail view.
/// Returns the line index of the selected subtask (if any).
#[allow(clippy::too_many_arguments)]
fn render_subtask_tree(
    lines: &mut Vec<Line<'static>>,
    app: &App,
    tasks: &[Task],
    depth: usize,
    width: usize,
    bg: ratatui::style::Color,
    selected_subtask_id: Option<&str>,
    search_re: Option<&Regex>,
    highlight_style: Style,
) -> Option<usize> {
    let selection_bg = app.theme.selection_bg;
    let mut selected_line: Option<usize> = None;

    for (i, task) in tasks.iter().enumerate() {
        let is_last = i == tasks.len() - 1;
        let state_color = app.theme.state_color(task.state);

        let is_selected =
            selected_subtask_id.is_some() && task.id.as_deref() == selected_subtask_id;
        let is_subtask_flashing = task.id.as_deref().is_some_and(|id| app.is_flashing(id));

        let (flash_bg, flash_border) = if is_subtask_flashing {
            match app.flash_state {
                Some(state) => state_flash_colors(state, &app.theme),
                None => UNDO_FLASH_COLORS,
            }
        } else {
            (bg, bg) // unused, but avoids Option
        };

        let row_bg = if is_subtask_flashing {
            flash_bg
        } else if is_selected {
            selection_bg
        } else {
            bg
        };
        let row_dim_style = Style::default().fg(app.theme.dim).bg(row_bg);

        let mut spans: Vec<Span> = Vec::new();

        // Selection indicator (▎) or space
        if is_subtask_flashing {
            spans.push(Span::styled(
                "\u{258E}",
                Style::default().fg(flash_border).bg(row_bg),
            ));
        } else if is_selected {
            spans.push(Span::styled(
                "\u{258E}",
                Style::default().fg(app.theme.selection_border).bg(row_bg),
            ));
        } else {
            spans.push(Span::styled(" ", Style::default().bg(row_bg)));
        }

        // Indent
        for _ in 0..depth {
            spans.push(Span::styled("  ", row_dim_style));
        }

        // Tree char
        let tree_char = if is_last { "\u{2514}" } else { "\u{251C}" };
        spans.push(Span::styled(tree_char, row_dim_style));
        spans.push(Span::styled(" ", row_dim_style));

        // State symbol
        let state_style = if is_selected {
            Style::default()
                .fg(state_color)
                .bg(row_bg)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(state_color).bg(row_bg)
        };
        spans.push(Span::styled(state_symbol(task.state), state_style));
        spans.push(Span::styled(" ", Style::default().bg(row_bg)));

        // Abbreviated ID
        if let Some(ref id) = task.id {
            let abbrev = abbreviated_id(id);
            let id_style = if is_selected {
                Style::default().fg(app.theme.selection_id).bg(row_bg)
            } else {
                Style::default().fg(app.theme.text).bg(row_bg)
            };
            push_highlighted_spans(
                &mut spans,
                &format!("{} ", abbrev),
                id_style,
                highlight_style,
                search_re,
            );
        }

        // Title
        let title_style = if task.state == TaskState::Done {
            Style::default().fg(app.theme.dim).bg(row_bg)
        } else if is_selected {
            Style::default()
                .fg(app.theme.text_bright)
                .bg(row_bg)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(app.theme.text_bright).bg(row_bg)
        };
        push_highlighted_spans(
            &mut spans,
            &task.title,
            title_style,
            highlight_style,
            search_re,
        );

        // Tags
        if !task.tags.is_empty() {
            spans.push(Span::styled("  ", Style::default().bg(row_bg)));
            for (j, tag) in task.tags.iter().enumerate() {
                let tag_color = app.theme.tag_color(tag);
                let tag_style = if task.state == TaskState::Done {
                    Style::default().fg(app.theme.dim).bg(row_bg)
                } else {
                    Style::default().fg(tag_color).bg(row_bg)
                };
                if j > 0 {
                    spans.push(Span::styled(" ", Style::default().bg(row_bg)));
                }
                push_highlighted_spans(
                    &mut spans,
                    &format!("#{}", tag),
                    tag_style,
                    highlight_style,
                    search_re,
                );
            }
        }

        // Pad to full width
        let content_width: usize = spans.iter().map(|s| s.content.chars().count()).sum();
        if content_width < width {
            spans.push(Span::styled(
                " ".repeat(width - content_width),
                Style::default().bg(row_bg),
            ));
        }

        if is_selected {
            selected_line = Some(lines.len());
        }

        lines.push(Line::from(spans));

        // Recurse into sub-subtasks
        if !task.subtasks.is_empty() {
            let child_result = render_subtask_tree(
                lines,
                app,
                &task.subtasks,
                depth + 1,
                width,
                bg,
                selected_subtask_id,
                search_re,
                highlight_style,
            );
            if selected_line.is_none() {
                selected_line = child_result;
            }
        }
    }

    selected_line
}

/// Region indicator: a small accent mark on the left for the active region
fn region_indicator(
    is_active: bool,
    active_style: Style,
    bg: ratatui::style::Color,
) -> Span<'static> {
    if is_active {
        Span::styled(" \u{258E} ", active_style)
    } else {
        Span::styled("   ", Style::default().bg(bg))
    }
}

/// State symbols for task states (markdown checkbox style, matching track view)
fn state_symbol(state: TaskState) -> &'static str {
    match state {
        TaskState::Todo => "[ ]",
        TaskState::Active => "[>]",
        TaskState::Blocked => "[-]",
        TaskState::Done => "[x]",
        TaskState::Parked => "[~]",
    }
}

/// Short state symbol for dep display
fn state_symbol_short(state: TaskState) -> &'static str {
    match state {
        TaskState::Todo => "[ ]",
        TaskState::Active => "[>]",
        TaskState::Blocked => "[-]",
        TaskState::Done => "[x]",
        TaskState::Parked => "[~]",
    }
}

/// Get abbreviated ID (e.g., "EFF-014.2" -> ".2")
fn abbreviated_id(id: &str) -> &str {
    if let Some(dash_pos) = id.find('-') {
        let after_prefix = &id[dash_pos + 1..];
        if let Some(dot_pos) = after_prefix.find('.') {
            return &after_prefix[dot_pos..];
        }
    }
    id
}

/// Collect dep IDs from a task
fn collect_deps(task: &Task) -> Vec<String> {
    let mut deps = Vec::new();
    for meta in &task.metadata {
        if let Metadata::Dep(d) = meta {
            deps.extend(d.clone());
        }
    }
    deps
}

/// Collect ref paths from a task
fn collect_refs(task: &Task) -> Vec<String> {
    let mut refs = Vec::new();
    for meta in &task.metadata {
        if let Metadata::Ref(r) = meta {
            refs.extend(r.clone());
        }
    }
    refs
}

/// Undo flash colors: bright orange bg + orange border (distinct from parked yellow)
pub const UNDO_FLASH_COLORS: (ratatui::style::Color, ratatui::style::Color) = (
    ratatui::style::Color::Rgb(0x50, 0x30, 0x10), // dark orange bg
    ratatui::style::Color::Rgb(0xFF, 0xA0, 0x30), // orange border
);

/// Apply flash highlight to a range of lines (modifies bg and border indicator).
fn apply_flash_to_lines(
    lines: &mut [Line<'static>],
    start_idx: usize,
    end_idx: usize,
    flash_bg: ratatui::style::Color,
    flash_border: ratatui::style::Color,
    width: usize,
) {
    for line_idx in start_idx..=end_idx {
        if let Some(line) = lines.get_mut(line_idx) {
            let mut new_spans: Vec<Span<'static>> = Vec::new();
            for (i, span) in line.spans.drain(..).enumerate() {
                if i == 0 && line_idx == start_idx && span.content.contains('\u{258E}') {
                    new_spans.push(Span::styled(
                        span.content.into_owned(),
                        Style::default().fg(flash_border).bg(flash_bg),
                    ));
                } else {
                    new_spans.push(Span::styled(
                        span.content.into_owned(),
                        span.style.bg(flash_bg),
                    ));
                }
            }
            let content_width: usize = new_spans.iter().map(|s| s.content.chars().count()).sum();
            if content_width < width {
                new_spans.push(Span::styled(
                    " ".repeat(width - content_width),
                    Style::default().bg(flash_bg),
                ));
            }
            *line = Line::from(new_spans);
        }
    }
}

/// Get state-specific flash colors (background, border) for a task state.
pub fn state_flash_colors(
    state: TaskState,
    theme: &Theme,
) -> (ratatui::style::Color, ratatui::style::Color) {
    use ratatui::style::Color;
    match state {
        TaskState::Active => (Color::Rgb(0x5A, 0x1A, 0x48), theme.highlight), // pink bg, pink border
        TaskState::Blocked => (Color::Rgb(0x55, 0x1A, 0x1A), theme.red),      // red bg, red border
        TaskState::Parked => (Color::Rgb(0x4A, 0x3A, 0x15), theme.yellow), // amber bg, yellow border
        TaskState::Todo => (Color::Rgb(0x3A, 0x1A, 0x58), theme.purple), // purple bg, purple border
        TaskState::Done => (Color::Rgb(0x1A, 0x2A, 0x55), theme.blue),   // blue bg, blue border
    }
}

/// Find the state of a task by ID across all tracks
fn find_task_state_across_tracks(app: &App, task_id: &str) -> Option<TaskState> {
    for (_, track) in &app.project.tracks {
        if let Some(task) = task_ops::find_task_in_track(track, task_id) {
            return Some(task.state);
        }
    }
    None
}
