mod command;
mod common;
mod confirm;
mod edit;
mod move_mode;
mod navigate;
mod popups;
mod recent;
mod search;
mod select;
mod tracks;
mod triage;

use crossterm::event::{KeyCode, KeyEvent};

use super::app::{App, DetailRegion, Mode};

// Import all submodule functions into this module's namespace
// so that submodules can access cross-module functions via `use super::*;`
#[allow(unused_imports)]
use command::*;
#[allow(unused_imports)]
use common::*;
#[allow(unused_imports)]
use confirm::*;
#[allow(unused_imports)]
use edit::*;
#[allow(unused_imports)]
use move_mode::*;
#[allow(unused_imports)]
use navigate::*;
#[allow(unused_imports)]
use popups::*;
#[allow(unused_imports)]
use recent::*;
#[allow(unused_imports)]
use search::*;
#[allow(unused_imports)]
use select::*;
#[allow(unused_imports)]
use tracks::*;
#[allow(unused_imports)]
use triage::*;

// Re-export public items
pub use common::{multiline_selection_range, selection_cols_for_line};
pub use recent::{RecentEntry, build_recent_entries};

/// Handle a key event in the current mode
pub fn handle_key(app: &mut App, key: KeyEvent) {
    // Ignore bare modifier key presses (Shift, Ctrl, Alt, etc.)
    if matches!(key.code, KeyCode::Modifier(_)) {
        return;
    }
    app.show_startup_hints = false;

    // Recovery log overlay intercepts all input
    if app.show_recovery_log {
        handle_recovery_overlay(app, key);
        return;
    }

    // Results overlay intercepts all input
    if app.show_results_overlay {
        handle_results_overlay(app, key);
        return;
    }

    let key = normalize_key(key);
    match &app.mode {
        Mode::Navigate => handle_navigate(app, key),
        Mode::Search => handle_search(app, key),
        Mode::Edit => handle_edit(app, key),
        Mode::Move => handle_move(app, key),
        Mode::Triage => handle_triage(app, key),
        Mode::Confirm => handle_confirm(app, key),
        Mode::Select => handle_select(app, key),
        Mode::Command => handle_command(app, key),
    }
}

/// Handle a bracketed paste event (terminal sends pasted text as a single string).
/// Only active in Edit mode — inserts at cursor with a single undo snapshot.
pub fn handle_paste(app: &mut App, text: &str) {
    if app.mode != Mode::Edit || text.is_empty() {
        return;
    }

    // Check if we're in multi-line note editing
    let is_detail_multiline = app
        .detail_state
        .as_ref()
        .is_some_and(|ds| ds.editing && ds.region == DetailRegion::Note)
        && app.edit_target.is_none();

    if is_detail_multiline {
        // Capture current cursor into the undo stack before mutating.
        // Cursor movements don't create snapshots, so the last entry may
        // have a stale position. snapshot()'s dedup updates it in place.
        snapshot_multiline(app);
        // Multi-line: insert text as-is (preserving newlines)
        if let Some(ds) = &mut app.detail_state {
            delete_multiline_selection(ds);
            let offset =
                multiline_pos_to_offset(&ds.edit_buffer, ds.edit_cursor_line, ds.edit_cursor_col);
            ds.edit_buffer.insert_str(offset, text);
            let new_offset = offset + text.len();
            let (new_line, new_col) = offset_to_multiline_pos(&ds.edit_buffer, new_offset);
            ds.edit_cursor_line = new_line;
            ds.edit_cursor_col = new_col;
        }
        snapshot_multiline(app);
    } else {
        // Capture current cursor into the undo stack before mutating.
        // Cursor movements don't create snapshots, so the last entry may
        // have a stale position. snapshot()'s dedup updates it in place.
        if let Some(eh) = &mut app.edit_history {
            eh.snapshot(&app.edit_buffer, app.edit_cursor, 0);
        }
        // Single-line: replace newlines with spaces, insert at cursor
        let clean = text.replace('\n', " ").replace('\r', "");
        app.delete_selection();
        app.edit_buffer.insert_str(app.edit_cursor, &clean);
        app.edit_cursor += clean.len();
        app.edit_selection_anchor = None;
        if let Some(eh) = &mut app.edit_history {
            eh.snapshot(&app.edit_buffer, app.edit_cursor, 0);
        }
        update_autocomplete_filter(app);
    }
}
