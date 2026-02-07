use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::app::{App, Mode, View};

/// Handle a key event in the current mode
pub fn handle_key(app: &mut App, key: KeyEvent) {
    match app.mode {
        Mode::Navigate => handle_navigate(app, key),
    }
}

fn handle_navigate(app: &mut App, key: KeyEvent) {
    match (key.modifiers, key.code) {
        // Quit: Ctrl+Q
        (m, KeyCode::Char('q')) if m.contains(KeyModifiers::CONTROL) => {
            app.should_quit = true;
        }

        // Tab switching: 1-9 for active tracks
        (KeyModifiers::NONE, KeyCode::Char(c @ '1'..='9')) => {
            let idx = (c as usize) - ('1' as usize);
            if idx < app.active_track_ids.len() {
                app.view = View::Track(idx);
            }
        }

        // Tab/Shift+Tab: next/prev tab
        (KeyModifiers::NONE, KeyCode::Tab) => {
            switch_tab(app, 1);
        }
        (KeyModifiers::SHIFT, KeyCode::BackTab) => {
            switch_tab(app, -1);
        }

        // View switching
        (KeyModifiers::NONE, KeyCode::Char('i')) => {
            app.view = View::Inbox;
        }
        (KeyModifiers::NONE, KeyCode::Char('r')) => {
            app.view = View::Recent;
        }
        (KeyModifiers::NONE, KeyCode::Char('0') | KeyCode::Char('`')) => {
            app.view = View::Tracks;
        }

        _ => {}
    }
}

/// Switch to the next/prev tab. Direction: 1 = forward, -1 = backward.
fn switch_tab(app: &mut App, direction: i32) {
    let total_tracks = app.active_track_ids.len();
    // All views in order: Track(0)..Track(N-1), Tracks, Inbox, Recent
    let total_views = total_tracks + 3;

    let current_idx = match &app.view {
        View::Track(i) => *i,
        View::Tracks => total_tracks,
        View::Inbox => total_tracks + 1,
        View::Recent => total_tracks + 2,
    };

    let new_idx = (current_idx as i32 + direction).rem_euclid(total_views as i32) as usize;

    app.view = if new_idx < total_tracks {
        View::Track(new_idx)
    } else {
        match new_idx - total_tracks {
            0 => View::Tracks,
            1 => View::Inbox,
            _ => View::Recent,
        }
    };
}
