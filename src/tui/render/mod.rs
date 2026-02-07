pub mod help_overlay;
pub mod inbox_view;
pub mod recent_view;
pub mod status_row;
pub mod tab_bar;
pub mod track_view;
pub mod tracks_view;

use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::Style;
use ratatui::widgets::Block;

use super::app::{App, View};

/// Main render function — dispatches to sub-renderers
pub fn render(frame: &mut Frame, app: &mut App) {
    let area = frame.area();

    // Background fill
    let bg_style = Style::default().bg(app.theme.background);
    frame.render_widget(Block::default().style(bg_style), area);

    // Layout: tab bar (2 rows) | content | status row (1 row)
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2), // tab bar + separator
            Constraint::Min(1),    // content area
            Constraint::Length(1), // status row
        ])
        .split(area);

    // Render tab bar
    tab_bar::render_tab_bar(frame, app, chunks[0]);

    // Render content area (clone view to avoid borrow conflict)
    let view = app.view.clone();
    match &view {
        View::Track(_) => track_view::render_track_view(frame, app, chunks[1]),
        View::Tracks => tracks_view::render_tracks_view(frame, app, chunks[1]),
        View::Inbox => inbox_view::render_inbox_view(frame, app, chunks[1]),
        View::Recent => recent_view::render_recent_view(frame, app, chunks[1]),
    }

    // Help overlay (rendered on top of everything)
    if app.show_help {
        help_overlay::render_help_overlay(frame, app, frame.area());
    }

    // Status row
    status_row::render_status_row(frame, app, chunks[2]);
}
