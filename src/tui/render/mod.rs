pub mod tab_bar;

use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::Style;
use ratatui::widgets::{Block, Paragraph};
use ratatui::Frame;

use super::app::{App, View};

/// Main render function — dispatches to sub-renderers
pub fn render(frame: &mut Frame, app: &App) {
    let area = frame.area();

    // Background fill
    let bg_style = Style::default().bg(app.theme.background);
    frame.render_widget(Block::default().style(bg_style), area);

    // Layout: tab bar (2 rows) | content | status row (1 row)
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2), // tab bar + separator
            Constraint::Min(1),   // content area
            Constraint::Length(1), // status row
        ])
        .split(area);

    // Render tab bar
    tab_bar::render_tab_bar(frame, app, chunks[0]);

    // Render content area (placeholder for now)
    render_content_placeholder(frame, app, chunks[1]);

    // Status row is empty in NAVIGATE mode
}

fn render_content_placeholder(frame: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let text = match &app.view {
        View::Track(idx) => {
            if let Some(track_id) = app.active_track_ids.get(*idx) {
                let name = app.track_name(track_id);
                format!("Track: {} ({})", name, track_id)
            } else {
                "No track selected".to_string()
            }
        }
        View::Tracks => "Tracks view".to_string(),
        View::Inbox => {
            let count = app.inbox_count();
            format!("Inbox ({} items)", count)
        }
        View::Recent => "Recent view".to_string(),
    };

    let paragraph = Paragraph::new(text)
        .style(Style::default().fg(app.theme.text).bg(app.theme.background));
    frame.render_widget(paragraph, area);
}
