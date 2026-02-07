use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::tui::app::{App, Mode};

/// Render the status row (bottom of screen)
pub fn render_status_row(frame: &mut Frame, app: &App, area: Rect) {
    let bg = app.theme.background;
    let width = area.width as usize;

    let line = match app.mode {
        Mode::Navigate => {
            // Empty in navigate mode (clean, like vim normal mode)
            // But if we have an active search, show it dimmed
            if let Some(ref pattern) = app.last_search {
                let mut spans = vec![Span::styled(
                    format!("/{}", pattern),
                    Style::default().fg(app.theme.dim).bg(bg),
                )];
                let hint = "n/N next/prev";
                let content_width: usize = spans.iter().map(|s| s.content.chars().count()).sum();
                let hint_width = hint.chars().count();
                if content_width + hint_width < width {
                    let padding = width - content_width - hint_width;
                    spans.push(Span::styled(" ".repeat(padding), Style::default().bg(bg)));
                    spans.push(Span::styled(
                        hint,
                        Style::default().fg(app.theme.dim).bg(bg),
                    ));
                }
                Line::from(spans)
            } else {
                Line::from(Span::styled(" ".repeat(width), Style::default().bg(bg)))
            }
        }
        Mode::Search => {
            // Search prompt: /pattern▌
            let mut spans = vec![
                Span::styled(
                    format!("/{}", app.search_input),
                    Style::default().fg(app.theme.text_bright).bg(bg),
                ),
                Span::styled("\u{258C}", Style::default().fg(app.theme.highlight).bg(bg)), // ▌ cursor
            ];
            let hint = "Enter search  Esc cancel";
            let content_width: usize = spans.iter().map(|s| s.content.chars().count()).sum();
            let hint_width = hint.chars().count();
            if content_width + hint_width < width {
                let padding = width - content_width - hint_width;
                spans.push(Span::styled(" ".repeat(padding), Style::default().bg(bg)));
                spans.push(Span::styled(
                    hint,
                    Style::default().fg(app.theme.dim).bg(bg),
                ));
            }
            Line::from(spans)
        }
    };

    let paragraph = Paragraph::new(line).style(Style::default().bg(bg));
    frame.render_widget(paragraph, area);
}
