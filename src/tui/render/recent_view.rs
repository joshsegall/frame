use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::model::{Metadata, SectionKind};
use crate::tui::app::App;

/// Render the recent completed tasks view (read-only for Phase 4)
pub fn render_recent_view(frame: &mut Frame, app: &App, area: Rect) {
    // Collect all done tasks across active tracks, with their resolved dates
    let mut done_tasks: Vec<RecentTask> = Vec::new();

    for (track_id, track) in &app.project.tracks {
        let track_name = app.track_name(track_id);
        for task in track.section_tasks(SectionKind::Done) {
            let resolved = task
                .metadata
                .iter()
                .find_map(|m| {
                    if let Metadata::Resolved(d) = m {
                        Some(d.clone())
                    } else {
                        None
                    }
                })
                .unwrap_or_default();
            done_tasks.push(RecentTask {
                id: task.id.clone().unwrap_or_default(),
                title: task.title.clone(),
                resolved,
                track_name: track_name.to_string(),
            });
        }
    }

    // Sort by resolved date, most recent first
    done_tasks.sort_by(|a, b| b.resolved.cmp(&a.resolved));

    if done_tasks.is_empty() {
        let empty = Paragraph::new(" No completed tasks")
            .style(Style::default().fg(app.theme.dim).bg(app.theme.background));
        frame.render_widget(empty, area);
        return;
    }

    let cursor = app.recent_cursor;
    let scroll = app.recent_scroll;
    let visible_height = area.height as usize;

    let mut lines: Vec<Line> = Vec::new();
    let mut current_date = String::new();

    for (flat_idx, task) in done_tasks.iter().enumerate() {
        // Date header (group by date)
        if task.resolved != current_date && !task.resolved.is_empty() {
            current_date.clone_from(&task.resolved);
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
        let bg = if is_cursor {
            app.theme.highlight
        } else {
            app.theme.background
        };

        let mut spans: Vec<Span> = Vec::new();

        // Check mark + ID + Title
        spans.push(Span::styled(
            " \u{2713} ",
            Style::default().fg(app.theme.dim).bg(bg),
        ));

        if !task.id.is_empty() {
            spans.push(Span::styled(
                format!("{} ", task.id),
                Style::default().fg(app.theme.dim).bg(bg),
            ));
        }

        let title_style = if is_cursor {
            Style::default().fg(app.theme.text_bright).bg(bg)
        } else {
            Style::default().fg(app.theme.dim).bg(bg)
        };
        spans.push(Span::styled(task.title.clone(), title_style));

        // Track origin
        spans.push(Span::styled(
            format!("  {}", task.track_name),
            Style::default().fg(app.theme.dim).bg(bg),
        ));

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

        lines.push(Line::from(spans));
    }

    // Apply scroll
    let visible_lines: Vec<Line> = lines
        .into_iter()
        .skip(scroll)
        .take(visible_height)
        .collect();

    let paragraph = Paragraph::new(visible_lines).style(Style::default().bg(app.theme.background));
    frame.render_widget(paragraph, area);
}

struct RecentTask {
    id: String,
    title: String,
    resolved: String,
    track_name: String,
}
