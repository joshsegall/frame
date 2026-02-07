use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::tui::app::App;

/// Render the inbox view (read-only display for Phase 4)
pub fn render_inbox_view(frame: &mut Frame, app: &App, area: Rect) {
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

    let cursor = app.inbox_cursor;
    let scroll = app.inbox_scroll;
    let visible_height = area.height as usize;

    // Build all display lines with their item indices
    let mut display_lines: Vec<(Option<usize>, Line)> = Vec::new();

    for (i, item) in inbox.items.iter().enumerate() {
        let is_cursor = i == cursor;
        let bg = if is_cursor {
            app.theme.highlight
        } else {
            app.theme.background
        };

        // Blank line before each item (except first)
        if i > 0 {
            display_lines.push((None, Line::from("")));
        }

        // Number + title + tags
        let mut spans: Vec<Span> = Vec::new();
        let num_style = Style::default().fg(app.theme.dim).bg(bg);
        spans.push(Span::styled(format!(" {:>2}  ", i + 1), num_style));

        let title_style = if is_cursor {
            Style::default()
                .fg(app.theme.text_bright)
                .bg(bg)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(app.theme.text_bright).bg(bg)
        };
        spans.push(Span::styled(item.title.clone(), title_style));

        // Tags
        if !item.tags.is_empty() {
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
                        Style::default().fg(app.theme.dim).bg(app.theme.background),
                    ),
                ];
                display_lines.push((Some(i), Line::from(body_spans)));
            }
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
