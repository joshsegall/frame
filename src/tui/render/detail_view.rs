use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::model::{Metadata, Task, TaskState};
use crate::ops::task_ops;
use crate::tui::app::{App, DetailRegion, Mode, View};

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
    if let Some(ref mut ds) = app.detail_state {
        ds.regions = regions.clone();
        // Clamp region to valid range
        if !regions.contains(&ds.region) {
            ds.region = regions.first().copied().unwrap_or(DetailRegion::Title);
        }
    }

    let detail_state = app.detail_state.as_ref();
    let current_region = detail_state.map(|ds| ds.region).unwrap_or(DetailRegion::Title);
    let editing = detail_state.is_some_and(|ds| ds.editing);

    let bg = app.theme.background;
    let text_style = Style::default().fg(app.theme.text).bg(bg);
    let bright_style = Style::default().fg(app.theme.text_bright).bg(bg);
    let dim_style = Style::default().fg(app.theme.dim).bg(bg);
    let region_indicator_style = Style::default()
        .fg(app.theme.highlight)
        .bg(bg);

    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut active_region_line: Option<usize> = None;
    let mut edit_anchor_col: Option<u16> = None;
    let mut edit_anchor_line: Option<usize> = None;

    // Blank line at top for breathing room
    lines.push(Line::from(""));

    // --- Title region ---
    {
        let is_active = current_region == DetailRegion::Title;
        if is_active {
            active_region_line = Some(lines.len());
        }
        let state_color = app.theme.state_color(task.state);
        let state_sym = state_symbol(task.state);
        let mut spans: Vec<Span> = Vec::new();

        // Region indicator
        spans.push(region_indicator(is_active, region_indicator_style, bg));

        // State symbol
        spans.push(Span::styled(
            state_sym,
            Style::default().fg(state_color).bg(bg).add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(" ", Style::default().bg(bg)));

        // ID
        if let Some(ref id) = task.id {
            spans.push(Span::styled(
                format!("{} ", id),
                text_style,
            ));
        }

        // Title
        if is_active && editing && app.mode == Mode::Edit {
            // Render edit buffer with cursor
            render_edit_inline(&mut spans, app, bright_style);
        } else {
            spans.push(Span::styled(
                task.title.clone(),
                bright_style.add_modifier(Modifier::BOLD),
            ));
        }

        lines.push(Line::from(spans));
    }

    // Blank line before metadata fields
    lines.push(Line::from(""));

    // --- Tags region ---
    {
        let is_active = current_region == DetailRegion::Tags;
        if is_active {
            active_region_line = Some(lines.len());
        }
        let mut spans: Vec<Span> = Vec::new();
        spans.push(region_indicator(is_active, region_indicator_style, bg));
        spans.push(Span::styled("tags: ", dim_style));

        if is_active && editing && app.mode == Mode::Edit {
            edit_anchor_col = Some(spans.iter().map(|s| s.content.chars().count() as u16).sum());
            edit_anchor_line = Some(lines.len());
            render_edit_inline(&mut spans, app, bright_style);
        } else if task.tags.is_empty() {
            spans.push(Span::styled("(none)", dim_style));
        } else {
            for (i, tag) in task.tags.iter().enumerate() {
                let tag_color = app.theme.tag_color(tag);
                let tag_style = Style::default().fg(tag_color).bg(bg);
                if i > 0 {
                    spans.push(Span::styled(" ", Style::default().bg(bg)));
                }
                spans.push(Span::styled(format!("#{}", tag), tag_style));
            }
        }

        lines.push(Line::from(spans));
    }

    // --- Added region ---
    for meta in &task.metadata {
        if let Metadata::Added(date) = meta {
            let is_active = current_region == DetailRegion::Added;
            if is_active {
                active_region_line = Some(lines.len());
            }
            let mut spans: Vec<Span> = Vec::new();
            spans.push(region_indicator(is_active, region_indicator_style, bg));
            spans.push(Span::styled("added: ", dim_style));
            spans.push(Span::styled(date.clone(), text_style));
            lines.push(Line::from(spans));
            break;
        }
    }

    // --- Deps region ---
    {
        let is_active = current_region == DetailRegion::Deps;
        if is_active {
            active_region_line = Some(lines.len());
        }
        let deps = collect_deps(task);

        if is_active && editing && app.mode == Mode::Edit {
            let mut spans: Vec<Span> = Vec::new();
            spans.push(region_indicator(is_active, region_indicator_style, bg));
            spans.push(Span::styled("dep: ", dim_style));
            edit_anchor_col = Some(spans.iter().map(|s| s.content.chars().count() as u16).sum());
            edit_anchor_line = Some(lines.len());
            render_edit_inline(&mut spans, app, bright_style);
            lines.push(Line::from(spans));
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
                spans.push(Span::styled(dep_id.clone(), text_style));
                if let Some(state) = dep_state {
                    spans.push(Span::styled(
                        format!(" {}", state_symbol_short(state)),
                        dep_style,
                    ));
                }
            }
            lines.push(Line::from(spans));
        } else if is_active {
            let mut spans: Vec<Span> = Vec::new();
            spans.push(region_indicator(is_active, region_indicator_style, bg));
            spans.push(Span::styled("dep: ", dim_style));
            spans.push(Span::styled("(none)", dim_style));
            lines.push(Line::from(spans));
        }
    }

    // --- Spec region ---
    {
        let is_active = current_region == DetailRegion::Spec;
        if is_active {
            active_region_line = Some(lines.len());
        }
        let spec = task.metadata.iter().find_map(|m| {
            if let Metadata::Spec(s) = m { Some(s.clone()) } else { None }
        });

        if is_active && editing && app.mode == Mode::Edit {
            let mut spans: Vec<Span> = Vec::new();
            spans.push(region_indicator(is_active, region_indicator_style, bg));
            spans.push(Span::styled("spec: ", dim_style));
            edit_anchor_col = Some(spans.iter().map(|s| s.content.chars().count() as u16).sum());
            edit_anchor_line = Some(lines.len());
            render_edit_inline(&mut spans, app, bright_style);
            lines.push(Line::from(spans));
        } else if let Some(spec_val) = &spec {
            let mut spans: Vec<Span> = Vec::new();
            spans.push(region_indicator(is_active, region_indicator_style, bg));
            spans.push(Span::styled("spec: ", dim_style));
            spans.push(Span::styled(
                spec_val.clone(),
                Style::default().fg(app.theme.cyan).bg(bg),
            ));
            lines.push(Line::from(spans));
        } else if is_active {
            let mut spans: Vec<Span> = Vec::new();
            spans.push(region_indicator(is_active, region_indicator_style, bg));
            spans.push(Span::styled("spec: ", dim_style));
            spans.push(Span::styled("(none)", dim_style));
            lines.push(Line::from(spans));
        }
    }

    // --- Refs region ---
    {
        let is_active = current_region == DetailRegion::Refs;
        if is_active {
            active_region_line = Some(lines.len());
        }
        let refs = collect_refs(task);

        if is_active && editing && app.mode == Mode::Edit {
            let mut spans: Vec<Span> = Vec::new();
            spans.push(region_indicator(is_active, region_indicator_style, bg));
            spans.push(Span::styled("ref: ", dim_style));
            edit_anchor_col = Some(spans.iter().map(|s| s.content.chars().count() as u16).sum());
            edit_anchor_line = Some(lines.len());
            render_edit_inline(&mut spans, app, bright_style);
            lines.push(Line::from(spans));
        } else if !refs.is_empty() {
            for (i, ref_path) in refs.iter().enumerate() {
                let mut spans: Vec<Span> = Vec::new();
                spans.push(region_indicator(is_active && i == 0, region_indicator_style, bg));
                if i == 0 {
                    spans.push(Span::styled("ref: ", dim_style));
                } else {
                    spans.push(Span::styled("     ", dim_style));
                }
                spans.push(Span::styled(
                    ref_path.clone(),
                    Style::default().fg(app.theme.cyan).bg(bg),
                ));
                lines.push(Line::from(spans));
            }
        } else if is_active {
            let mut spans: Vec<Span> = Vec::new();
            spans.push(region_indicator(is_active, region_indicator_style, bg));
            spans.push(Span::styled("ref: ", dim_style));
            spans.push(Span::styled("(none)", dim_style));
            lines.push(Line::from(spans));
        }
    }

    // Blank line before note
    lines.push(Line::from(""));

    // --- Note region ---
    {
        let is_active = current_region == DetailRegion::Note;
        if is_active {
            active_region_line = Some(lines.len());
        }
        let note = task.metadata.iter().find_map(|m| {
            if let Metadata::Note(n) = m { Some(n.clone()) } else { None }
        });

        if is_active && editing && app.mode == Mode::Edit {
            // Multi-line editing mode
            let mut header_spans: Vec<Span> = Vec::new();
            header_spans.push(region_indicator(true, region_indicator_style, bg));
            header_spans.push(Span::styled("note:", dim_style));
            lines.push(Line::from(header_spans));

            // Render the multi-line edit buffer
            let note_indent = "   "; // align with field labels
            if let Some(ref ds) = app.detail_state {
                let edit_lines: Vec<&str> = ds.edit_buffer.split('\n').collect();
                for (line_idx, edit_line) in edit_lines.iter().enumerate() {
                    let mut spans: Vec<Span> = Vec::new();
                    spans.push(Span::styled(note_indent.to_string(), Style::default().bg(bg)));
                    if line_idx == ds.edit_cursor_line {
                        // Render with cursor highlighting current character
                        let col = ds.edit_cursor_col.min(edit_line.len());
                        let before = &edit_line[..col];
                        let cursor_style = Style::default()
                            .fg(app.theme.background)
                            .bg(app.theme.text_bright);
                        if !before.is_empty() {
                            spans.push(Span::styled(before.to_string(), bright_style));
                        }
                        if col < edit_line.len() {
                            let cursor_char = &edit_line[col..col + 1];
                            spans.push(Span::styled(cursor_char.to_string(), cursor_style));
                            let after = &edit_line[col + 1..];
                            if !after.is_empty() {
                                spans.push(Span::styled(after.to_string(), bright_style));
                            }
                        } else {
                            spans.push(Span::styled(" ".to_string(), cursor_style));
                        }
                    } else {
                        spans.push(Span::styled(edit_line.to_string(), bright_style));
                    }
                    lines.push(Line::from(spans));
                }
            }
        } else if let Some(note_text) = &note {
            let mut header_spans: Vec<Span> = Vec::new();
            header_spans.push(region_indicator(is_active, region_indicator_style, bg));
            header_spans.push(Span::styled("note:", dim_style));
            lines.push(Line::from(header_spans));

            let note_indent = "   "; // align with field labels
            let mut in_code_block = false;
            for note_line in note_text.lines() {
                let trimmed = note_line.trim();
                if trimmed.starts_with("```") {
                    in_code_block = !in_code_block;
                }

                let mut spans: Vec<Span> = Vec::new();
                spans.push(Span::styled(note_indent.to_string(), Style::default().bg(bg)));
                if in_code_block || trimmed.starts_with("```") {
                    spans.push(Span::styled(
                        note_line.to_string(),
                        Style::default().fg(app.theme.dim).bg(bg),
                    ));
                } else {
                    spans.push(Span::styled(note_line.to_string(), text_style));
                }
                lines.push(Line::from(spans));
            }
        } else if is_active {
            let mut spans: Vec<Span> = Vec::new();
            spans.push(region_indicator(is_active, region_indicator_style, bg));
            spans.push(Span::styled("note: ", dim_style));
            spans.push(Span::styled("(empty)", dim_style));
            lines.push(Line::from(spans));
        }
    }

    // --- Subtasks region ---
    if !task.subtasks.is_empty() {
        lines.push(Line::from(""));
        let is_active = current_region == DetailRegion::Subtasks;
        if is_active {
            active_region_line = Some(lines.len());
        }

        let mut header_spans: Vec<Span> = Vec::new();
        header_spans.push(region_indicator(is_active, region_indicator_style, bg));
        header_spans.push(Span::styled(
            "Subtasks",
            bright_style.add_modifier(Modifier::BOLD),
        ));
        lines.push(Line::from(header_spans));

        render_subtask_tree(&mut lines, app, &task.subtasks, 1, bg);
    }

    // Handle scrolling using tracked region line index
    let visible_height = area.height as usize;

    if let (Some(ds), Some(rl)) = (&mut app.detail_state, active_region_line) {
        if rl < ds.scroll_offset {
            ds.scroll_offset = rl;
        } else if rl >= ds.scroll_offset + visible_height {
            ds.scroll_offset = rl.saturating_sub(visible_height - 1);
        }
    }

    let scroll = app
        .detail_state
        .as_ref()
        .map(|ds| ds.scroll_offset)
        .unwrap_or(0);

    let paragraph = Paragraph::new(lines)
        .style(Style::default().bg(bg))
        .scroll((scroll as u16, 0));
    frame.render_widget(paragraph, area);

    // Set autocomplete anchor for detail view edits
    if let (Some(prefix_w), Some(line_idx)) = (edit_anchor_col, edit_anchor_line) {
        let word_offset = app.autocomplete.as_ref()
            .map(|ac| ac.word_start_in_buffer(&app.edit_buffer) as u16)
            .unwrap_or(0);
        let screen_y = area.y + line_idx.saturating_sub(scroll) as u16;
        let screen_x = area.x + prefix_w + word_offset;
        app.autocomplete_anchor = Some((screen_x, screen_y));
    }
}

/// Render the edit buffer inline with a cursor that highlights the current character
fn render_edit_inline(spans: &mut Vec<Span<'static>>, app: &App, style: Style) {
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
            // Render with selection highlight
            // Before selection
            if sel_start > 0 {
                spans.push(Span::styled(buf[..sel_start].to_string(), style));
            }
            // Selection
            spans.push(Span::styled(buf[sel_start..sel_end].to_string(), selection_style));
            // After selection
            if sel_end < buf.len() {
                spans.push(Span::styled(buf[sel_end..].to_string(), style));
            }
            // Cursor at end if needed
            if cursor_pos >= buf.len() {
                spans.push(Span::styled(" ".to_string(), cursor_style));
            }
            return;
        }
    }

    // No selection: render with cursor highlight
    let before = &buf[..cursor_pos];
    if !before.is_empty() {
        spans.push(Span::styled(before.to_string(), style));
    }
    if cursor_pos < buf.len() {
        let cursor_char = &buf[cursor_pos..cursor_pos + 1];
        spans.push(Span::styled(cursor_char.to_string(), cursor_style));
        let after = &buf[cursor_pos + 1..];
        if !after.is_empty() {
            spans.push(Span::styled(after.to_string(), style));
        }
    } else {
        spans.push(Span::styled(" ".to_string(), cursor_style));
    }
}

/// Render subtask tree for the detail view
fn render_subtask_tree(
    lines: &mut Vec<Line<'static>>,
    app: &App,
    tasks: &[Task],
    depth: usize,
    bg: ratatui::style::Color,
) {
    let dim_style = Style::default().fg(app.theme.dim).bg(bg);

    for (i, task) in tasks.iter().enumerate() {
        let is_last = i == tasks.len() - 1;
        let state_color = app.theme.state_color(task.state);
        let mut spans: Vec<Span> = Vec::new();

        // Indent
        spans.push(Span::styled(" ", Style::default().bg(bg)));
        for _ in 0..depth {
            spans.push(Span::styled("  ", dim_style));
        }

        // Tree char
        let tree_char = if is_last { "\u{2514}" } else { "\u{251C}" };
        spans.push(Span::styled(tree_char, dim_style));
        spans.push(Span::styled(" ", dim_style));

        // State symbol
        spans.push(Span::styled(
            state_symbol(task.state),
            Style::default().fg(state_color).bg(bg),
        ));
        spans.push(Span::styled(" ", Style::default().bg(bg)));

        // Abbreviated ID
        if let Some(ref id) = task.id {
            let abbrev = abbreviated_id(id);
            spans.push(Span::styled(
                format!("{} ", abbrev),
                Style::default().fg(app.theme.text).bg(bg),
            ));
        }

        // Title
        let title_style = if task.state == TaskState::Done {
            Style::default().fg(app.theme.dim).bg(bg)
        } else {
            Style::default().fg(app.theme.text_bright).bg(bg)
        };
        spans.push(Span::styled(task.title.clone(), title_style));

        // Tags
        if !task.tags.is_empty() {
            spans.push(Span::styled("  ", Style::default().bg(bg)));
            for (j, tag) in task.tags.iter().enumerate() {
                let tag_color = app.theme.tag_color(tag);
                let tag_style = if task.state == TaskState::Done {
                    Style::default().fg(app.theme.dim).bg(bg)
                } else {
                    Style::default().fg(tag_color).bg(bg)
                };
                if j > 0 {
                    spans.push(Span::styled(" ", Style::default().bg(bg)));
                }
                spans.push(Span::styled(format!("#{}", tag), tag_style));
            }
        }

        lines.push(Line::from(spans));

        // Recurse into sub-subtasks
        if !task.subtasks.is_empty() {
            render_subtask_tree(lines, app, &task.subtasks, depth + 1, bg);
        }
    }
}

/// Region indicator: a small accent mark on the left for the active region
fn region_indicator(is_active: bool, active_style: Style, bg: ratatui::style::Color) -> Span<'static> {
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

/// Find the state of a task by ID across all tracks
fn find_task_state_across_tracks(app: &App, task_id: &str) -> Option<TaskState> {
    for (_, track) in &app.project.tracks {
        if let Some(task) = task_ops::find_task_in_track(track, task_id) {
            return Some(task.state);
        }
    }
    None
}

