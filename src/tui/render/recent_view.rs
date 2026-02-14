use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::model::task::{Task, TaskState};
use crate::tui::app::{App, PendingMoveKind};
use crate::tui::input::build_recent_entries;
use crate::util::unicode;

use super::push_highlighted_spans;

/// Render the recent completed tasks view with tree structure
pub fn render_recent_view(frame: &mut Frame, app: &mut App, area: Rect) {
    let entries = build_recent_entries(app);

    if entries.is_empty() {
        let empty = Paragraph::new(" No completed tasks")
            .style(Style::default().fg(app.theme.dim).bg(app.theme.background));
        frame.render_widget(empty, area);
        return;
    }

    // Clamp cursor
    let task_count = entries.len();
    let cursor = app.recent_cursor.min(task_count.saturating_sub(1));
    app.recent_cursor = cursor;
    let visible_height = area.height as usize;

    let search_re = app.active_search_re();
    let mut lines: Vec<Line> = Vec::new();
    let mut current_date = String::new();
    let mut cursor_line: Option<usize> = None;

    for (flat_idx, entry) in entries.iter().enumerate() {
        // Date header (group by date)
        if entry.resolved != current_date && !entry.resolved.is_empty() {
            current_date.clone_from(&entry.resolved);
            if !lines.is_empty() {
                lines.push(Line::from(""));
            }
            lines.push(Line::from(Span::styled(
                format!(" {}", current_date),
                Style::default()
                    .fg(app.theme.text)
                    .bg(app.theme.background)
                    .add_modifier(Modifier::BOLD),
            )));
        }

        let is_cursor = flat_idx == cursor;
        if is_cursor {
            cursor_line = Some(lines.len());
        }
        let bg = if is_cursor {
            app.theme.selection_bg
        } else {
            app.theme.background
        };

        // Check if this task has a pending ToBacklog move (show as reopened)
        let has_pending_reopen = app.pending_moves.iter().any(|pm| {
            pm.kind == PendingMoveKind::ToBacklog
                && pm.track_id == entry.track_id
                && pm.task_id == entry.id
        });

        let has_subtasks = !entry.task.subtasks.is_empty();
        let is_expanded = has_subtasks && app.recent_expanded.contains(&entry.id);

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

        // Expand/collapse indicator or check mark
        if has_subtasks {
            let indicator = if is_expanded {
                "\u{25BC} "
            } else {
                "\u{25B6} "
            };
            spans.push(Span::styled(
                indicator,
                Style::default().fg(app.theme.dim).bg(bg),
            ));
        } else {
            spans.push(Span::styled("  ", Style::default().bg(bg)));
        }

        // State bracket for the task
        if has_pending_reopen {
            spans.push(Span::styled(
                "[ ] ",
                Style::default()
                    .fg(app.theme.state_color(TaskState::Todo))
                    .bg(bg),
            ));
        } else {
            spans.push(Span::styled(
                "[x] ",
                Style::default()
                    .fg(app.theme.state_color(TaskState::Done))
                    .bg(bg),
            ));
        }

        if !entry.id.is_empty() {
            let id_style = Style::default().fg(app.theme.text).bg(bg);
            let hl_style = Style::default()
                .fg(app.theme.search_match_fg)
                .bg(app.theme.search_match_bg)
                .add_modifier(Modifier::BOLD);
            push_highlighted_spans(
                &mut spans,
                &format!("{} ", entry.id),
                id_style,
                hl_style,
                search_re.as_ref(),
            );
        }

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
        let track_suffix_width = entry.track_name.len() + 2;
        let available = (area.width as usize).saturating_sub(prefix_width + track_suffix_width + 1);
        let display_title = super::truncate_with_ellipsis(&entry.title, available);
        push_highlighted_spans(
            &mut spans,
            &display_title,
            title_style,
            hl_style,
            search_re.as_ref(),
        );

        // Track origin (right-justified with 1-space buffer)
        let content_width: usize = spans
            .iter()
            .map(|s| unicode::display_width(&s.content))
            .sum();
        let track_label = &entry.track_name;
        let track_label_width = track_label.len();
        let w = area.width as usize;
        // Position: right edge minus track name minus 1 space buffer
        let track_start = w.saturating_sub(track_label_width + 1);
        if content_width < track_start {
            spans.push(Span::styled(
                " ".repeat(track_start - content_width),
                Style::default().bg(bg),
            ));
        }
        spans.push(Span::styled(
            track_label.clone(),
            Style::default().fg(app.theme.dim).bg(bg),
        ));
        // 1-space right buffer
        let final_width: usize = spans
            .iter()
            .map(|s| unicode::display_width(&s.content))
            .sum();
        if final_width < w {
            spans.push(Span::styled(
                " ".repeat(w - final_width),
                Style::default().bg(bg),
            ));
        }

        lines.push(Line::from(spans));

        // Render children if expanded
        if is_expanded {
            render_subtask_tree(
                &entry.task.subtasks,
                &mut lines,
                app,
                area,
                bg,
                search_re.as_ref(),
                &[],
                is_cursor,
            );
        }
    }

    // Auto-adjust scroll to keep cursor visible
    let mut scroll = app.recent_scroll;
    if let Some(cl) = cursor_line {
        if cl < scroll {
            scroll = cl;
        } else if cl >= scroll + visible_height {
            scroll = cl.saturating_sub(visible_height - 1);
        }
    }
    app.recent_scroll = scroll;

    // Apply scroll
    let visible_lines: Vec<Line> = lines
        .into_iter()
        .skip(scroll)
        .take(visible_height)
        .collect();

    let paragraph = Paragraph::new(visible_lines).style(Style::default().bg(app.theme.background));
    frame.render_widget(paragraph, area);
}

/// Render subtask tree lines recursively
#[allow(clippy::too_many_arguments)]
fn render_subtask_tree<'a>(
    tasks: &[Task],
    lines: &mut Vec<Line<'a>>,
    app: &App,
    area: Rect,
    _parent_bg: ratatui::style::Color,
    search_re: Option<&regex::Regex>,
    ancestor_last: &[bool],
    _parent_is_cursor: bool,
) {
    let bg = app.theme.background;
    let count = tasks.len();

    for (i, task) in tasks.iter().enumerate() {
        let is_last = i == count - 1;
        let mut spans: Vec<Span> = Vec::new();

        // Leading space
        spans.push(Span::styled(" ", Style::default().bg(bg)));
        // Extra indentation for expand/collapse column
        spans.push(Span::styled("  ", Style::default().bg(bg)));

        // Tree lines from ancestors
        for &ancestor_is_last in ancestor_last {
            let connector = if ancestor_is_last {
                "   "
            } else {
                "\u{2502}  "
            };
            spans.push(Span::styled(
                connector,
                Style::default().fg(app.theme.dim).bg(bg),
            ));
        }

        // Current branch connector
        let branch = if is_last {
            "\u{2514}\u{2500} "
        } else {
            "\u{251C}\u{2500} "
        };
        spans.push(Span::styled(
            branch,
            Style::default().fg(app.theme.dim).bg(bg),
        ));

        // State bracket showing actual state
        let state_str = match task.state {
            TaskState::Todo => "[ ] ",
            TaskState::Active => "[>] ",
            TaskState::Blocked => "[-] ",
            TaskState::Done => "[x] ",
            TaskState::Parked => "[~] ",
        };
        spans.push(Span::styled(
            state_str,
            Style::default()
                .fg(app.theme.state_color(task.state))
                .bg(bg),
        ));

        // ID
        if let Some(ref id) = task.id {
            let id_style = Style::default().fg(app.theme.dim).bg(bg);
            let hl_style = Style::default()
                .fg(app.theme.search_match_fg)
                .bg(app.theme.search_match_bg)
                .add_modifier(Modifier::BOLD);
            push_highlighted_spans(
                &mut spans,
                &format!("{} ", id),
                id_style,
                hl_style,
                search_re,
            );
        }

        // Title
        let title_style = Style::default().fg(app.theme.dim).bg(bg);
        let hl_style = Style::default()
            .fg(app.theme.search_match_fg)
            .bg(app.theme.search_match_bg)
            .add_modifier(Modifier::BOLD);
        let prefix_width: usize = spans
            .iter()
            .map(|s| unicode::display_width(&s.content))
            .sum();
        let available = (area.width as usize).saturating_sub(prefix_width + 1);
        let display_title = super::truncate_with_ellipsis(&task.title, available);
        push_highlighted_spans(&mut spans, &display_title, title_style, hl_style, search_re);

        lines.push(Line::from(spans));

        // Recurse into children (always shown when parent is expanded)
        if !task.subtasks.is_empty() {
            let mut new_ancestor_last = ancestor_last.to_vec();
            new_ancestor_last.push(is_last);
            render_subtask_tree(
                &task.subtasks,
                lines,
                app,
                area,
                bg,
                search_re,
                &new_ancestor_last,
                false,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::render::test_helpers::*;
    use insta::assert_snapshot;

    #[test]
    fn recent_empty() {
        let mut app = app_with_track("# Test\n\n## Backlog\n\n- [ ] `T-1` A task\n\n## Done\n");
        app.view = crate::tui::app::View::Recent;
        let output = render_to_string(TERM_W, TERM_H, |frame, area| {
            render_recent_view(frame, &mut app, area);
        });
        assert_snapshot!(output);
    }

    #[test]
    fn recent_with_done_tasks() {
        let md = "\
# Test

## Backlog

## Done

- [x] `T-1` Finished task
  - resolved: 2025-05-14
- [x] `T-2` Another done
  - resolved: 2025-05-12
";
        let mut app = app_with_track(md);
        app.view = crate::tui::app::View::Recent;
        let output = render_to_string(TERM_W, TERM_H, |frame, area| {
            render_recent_view(frame, &mut app, area);
        });
        assert_snapshot!(output);
    }
}
