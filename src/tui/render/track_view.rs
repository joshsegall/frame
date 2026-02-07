use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use regex::Regex;

use crate::model::{SectionKind, Task, TaskState};
use crate::tui::app::{App, FlatItem};

use super::push_highlighted_spans;

/// State symbols for each task state
fn state_symbol(state: TaskState) -> &'static str {
    match state {
        TaskState::Todo => "\u{25CB}",    // ○
        TaskState::Active => "\u{25D0}",  // ◐
        TaskState::Blocked => "\u{2298}", // ⊘
        TaskState::Done => "\u{2713}",    // ✓
        TaskState::Parked => "\u{25C7}",  // ◇
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
    let flat_items = app.build_flat_items(&track_id);
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
        let empty = Paragraph::new(" No tasks")
            .style(Style::default().fg(app.theme.dim).bg(app.theme.background));
        frame.render_widget(empty, area);
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
            } => {
                if let Some(task) = resolve_task(track, *section, path) {
                    let line = render_task_line(
                        app,
                        task,
                        &TaskLineInfo {
                            depth: *depth,
                            has_children: *has_children,
                            is_expanded: *is_expanded,
                            is_last_sibling: *is_last_sibling,
                            ancestor_last,
                        },
                        is_cursor,
                        area.width as usize,
                        search_re.as_ref(),
                    );
                    lines.push(line);
                }
            }
            FlatItem::ParkedSeparator => {
                lines.push(render_parked_separator(app, area.width as usize, is_cursor));
            }
        }
    }

    let paragraph = Paragraph::new(lines).style(Style::default().bg(app.theme.background));
    frame.render_widget(paragraph, area);
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

/// Render a single task line with all decorations
fn render_task_line<'a>(
    app: &'a App,
    task: &Task,
    info: &TaskLineInfo<'_>,
    is_cursor: bool,
    width: usize,
    search_re: Option<&Regex>,
) -> Line<'a> {
    let mut spans: Vec<Span> = Vec::new();
    let bg = app.theme.background;
    let dim_style = Style::default().fg(app.theme.dim).bg(bg);
    let state_color = app.theme.state_color(task.state);

    // Build prefix based on depth
    if info.depth == 0 {
        // Top-level: [expand][state] ID Title  tags
        let expand_char = if info.has_children {
            if info.is_expanded {
                "\u{25BE}"
            } else {
                "\u{25B8}"
            } // ▾ / ▸
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
        spans.push(Span::styled(" ", dim_style));

        // Expand indicator for subtasks with children
        if info.has_children {
            let expand_char = if info.is_expanded {
                "\u{25BE}"
            } else {
                "\u{25B8}"
            };
            spans.push(Span::styled(expand_char, dim_style));
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
        let id_style = if task.state == TaskState::Done {
            Style::default().fg(app.theme.dim).bg(bg)
        } else {
            Style::default().fg(app.theme.text).bg(bg)
        };
        spans.push(Span::styled(id_text, id_style));
    }

    // Title (with search highlighting)
    let title_style = if is_cursor {
        Style::default()
            .fg(app.theme.text_bright)
            .bg(bg)
            .add_modifier(Modifier::BOLD)
    } else if task.state == TaskState::Done {
        Style::default().fg(app.theme.dim).bg(bg)
    } else {
        Style::default().fg(app.theme.text_bright).bg(bg)
    };
    let highlight_style = title_style.bg(app.theme.purple);
    push_highlighted_spans(
        &mut spans,
        &task.title,
        title_style,
        highlight_style,
        search_re,
    );

    // Tags
    if !task.tags.is_empty() {
        spans.push(Span::styled("  ", Style::default().bg(bg)));
        for (i, tag) in task.tags.iter().enumerate() {
            let tag_color = app.theme.tag_color(tag);
            let tag_style = if task.state == TaskState::Done {
                Style::default().fg(app.theme.dim).bg(bg)
            } else {
                Style::default().fg(tag_color).bg(bg)
            };
            if i > 0 {
                spans.push(Span::styled(" ", Style::default().bg(bg)));
            }
            spans.push(Span::styled(format!("#{}", tag), tag_style));
        }
    }

    // Highlight cursor line
    if is_cursor {
        let content_width: usize = spans.iter().map(|s| s.content.chars().count()).sum();
        if content_width < width {
            spans.push(Span::styled(
                " ".repeat(width - content_width),
                Style::default().bg(app.theme.highlight),
            ));
        }
        // Re-style all spans with highlight background
        for span in &mut spans {
            span.style = span.style.bg(app.theme.highlight);
        }
    }

    Line::from(spans)
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

/// Render the parked section separator
fn render_parked_separator<'a>(app: &'a App, width: usize, is_cursor: bool) -> Line<'a> {
    let bg = if is_cursor {
        app.theme.highlight
    } else {
        app.theme.background
    };
    let style = Style::default().fg(app.theme.dim).bg(bg);

    let label = " Parked ";
    let dashes_before = 2;
    let dashes_after = width.saturating_sub(label.len() + dashes_before + 2);

    let line_text = format!(
        " {}{}{}",
        "\u{2500}".repeat(dashes_before),
        label,
        "\u{2500}".repeat(dashes_after.max(2))
    );

    Line::from(Span::styled(line_text, style))
}
