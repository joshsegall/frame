use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use crate::tui::app::App;

/// Render the results overlay (centered popup for check/clean results)
pub fn render_results_overlay(frame: &mut Frame, app: &App, area: Rect) {
    let bg = app.theme.background;
    let bright = app.theme.text_bright;
    let dim = app.theme.dim;

    // Size: centered, taking most of the screen
    let margin_x = 4u16.min(area.width / 8);
    let margin_y = 2u16.min(area.height / 8);
    let popup_area = Rect::new(
        area.x + margin_x,
        area.y + margin_y,
        area.width.saturating_sub(margin_x * 2),
        area.height.saturating_sub(margin_y * 2),
    );

    frame.render_widget(Clear, popup_area);

    let title = format!(" {} ", app.results_overlay_title);
    let block = Block::default()
        .title(Span::styled(
            title,
            Style::default()
                .fg(bright)
                .bg(bg)
                .add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(dim).bg(bg))
        .style(Style::default().bg(bg));

    let inner = block.inner(popup_area);
    frame.render_widget(block, popup_area);

    if app.results_overlay_lines.is_empty() {
        let empty = Paragraph::new(Line::from(Span::styled(
            "No results.",
            Style::default().fg(dim).bg(bg),
        )))
        .style(Style::default().bg(bg));
        frame.render_widget(empty, inner);
        return;
    }

    let visible_height = inner.height as usize;
    let total_lines = app.results_overlay_lines.len();
    let scroll = app
        .results_overlay_scroll
        .min(total_lines.saturating_sub(visible_height));

    let lines: Vec<Line> = app
        .results_overlay_lines
        .iter()
        .skip(scroll)
        .take(visible_height)
        .cloned()
        .collect();

    let paragraph = Paragraph::new(lines).style(Style::default().bg(bg));
    frame.render_widget(paragraph, inner);

    // Scroll indicator
    if total_lines > visible_height {
        let indicator = format!(
            " {}/{} ",
            scroll + 1,
            total_lines.saturating_sub(visible_height) + 1
        );
        let indicator_style = Style::default()
            .fg(Color::Black)
            .bg(dim)
            .add_modifier(Modifier::BOLD);
        let indicator_width = indicator.len() as u16;
        let indicator_x = popup_area.x + popup_area.width.saturating_sub(indicator_width + 1);
        let indicator_y = popup_area.y + popup_area.height - 1;
        if indicator_x < popup_area.x + popup_area.width && indicator_y < area.y + area.height {
            let indicator_area = Rect::new(indicator_x, indicator_y, indicator_width, 1);
            let indicator_widget =
                Paragraph::new(Line::from(Span::styled(indicator, indicator_style)));
            frame.render_widget(indicator_widget, indicator_area);
        }
    }
}
