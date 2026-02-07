use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use regex::Regex;
use std::io::Write;
use std::process::{Command, Stdio};

use crate::model::task::Metadata;
use crate::model::SectionKind;
use crate::ops::search::{search_inbox, search_tasks};
use crate::ops::task_ops::{self, InsertPosition};

use super::app::{App, AutocompleteKind, AutocompleteState, DetailRegion, DetailState, EditHistory, EditTarget, FlatItem, Mode, MoveState, View, resolve_task_from_flat};
use super::undo::{Operation, UndoNavTarget};

// ---------------------------------------------------------------------------
// Clipboard helpers (macOS pbcopy/pbpaste, Linux xclip)
// ---------------------------------------------------------------------------

fn clipboard_set(text: &str) {
    #[cfg(target_os = "macos")]
    let result = Command::new("pbcopy")
        .stdin(Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            if let Some(stdin) = child.stdin.as_mut() {
                stdin.write_all(text.as_bytes())?;
            }
            child.wait()
        });
    #[cfg(target_os = "linux")]
    let result = Command::new("xclip")
        .args(["-selection", "clipboard"])
        .stdin(Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            if let Some(stdin) = child.stdin.as_mut() {
                stdin.write_all(text.as_bytes())?;
            }
            child.wait()
        });
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    let result: Result<(), std::io::Error> = Ok(());
    let _ = result;
}

fn clipboard_get() -> Option<String> {
    #[cfg(target_os = "macos")]
    let output = Command::new("pbpaste").output().ok();
    #[cfg(target_os = "linux")]
    let output = Command::new("xclip")
        .args(["-selection", "clipboard", "-o"])
        .output()
        .ok();
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    let output: Option<std::process::Output> = None;
    output.and_then(|o| {
        if o.status.success() {
            String::from_utf8(o.stdout).ok()
        } else {
            None
        }
    })
}

// ---------------------------------------------------------------------------
// Multi-line selection helpers
// ---------------------------------------------------------------------------

/// Convert (line, col) to absolute byte offset in a multi-line buffer.
fn multiline_pos_to_offset(text: &str, line: usize, col: usize) -> usize {
    let mut offset = 0;
    for (i, l) in text.split('\n').enumerate() {
        if i == line {
            return offset + col.min(l.len());
        }
        offset += l.len() + 1;
    }
    text.len()
}

/// Convert absolute byte offset to (line, col) in a multi-line buffer.
fn offset_to_multiline_pos(text: &str, offset: usize) -> (usize, usize) {
    let mut remaining = offset;
    for (i, line) in text.split('\n').enumerate() {
        if remaining <= line.len() {
            return (i, remaining);
        }
        remaining -= line.len() + 1;
    }
    let line_count = text.split('\n').count();
    let last_len = text.split('\n').last().map_or(0, |l| l.len());
    (line_count.saturating_sub(1), last_len)
}

/// Get multi-line selection range as (start_offset, end_offset) in absolute terms.
pub fn multiline_selection_range(ds: &DetailState) -> Option<(usize, usize)> {
    let (anchor_line, anchor_col) = ds.multiline_selection_anchor?;
    let anchor_off = multiline_pos_to_offset(&ds.edit_buffer, anchor_line, anchor_col);
    let cursor_off = multiline_pos_to_offset(&ds.edit_buffer, ds.edit_cursor_line, ds.edit_cursor_col);
    let start = anchor_off.min(cursor_off);
    let end = anchor_off.max(cursor_off);
    if start == end {
        return None;
    }
    Some((start, end))
}

/// Get the selected text in a multi-line buffer.
fn get_multiline_selection_text(ds: &DetailState) -> Option<String> {
    let (start, end) = multiline_selection_range(ds)?;
    Some(ds.edit_buffer[start..end].to_string())
}

/// Delete the selected text in a multi-line buffer, updating cursor position.
/// Returns the deleted text if there was a selection.
fn delete_multiline_selection(ds: &mut DetailState) -> Option<String> {
    let (start, end) = multiline_selection_range(ds)?;
    let deleted = ds.edit_buffer[start..end].to_string();
    ds.edit_buffer.drain(start..end);
    let (line, col) = offset_to_multiline_pos(&ds.edit_buffer, start);
    ds.edit_cursor_line = line;
    ds.edit_cursor_col = col;
    ds.multiline_selection_anchor = None;
    Some(deleted)
}

/// Get the selected column range (start_col, end_col) for a specific line in a multi-line selection.
/// Returns None if the line has no selection.
pub fn selection_cols_for_line(buffer: &str, sel_start: usize, sel_end: usize, target_line: usize) -> Option<(usize, usize)> {
    let mut offset = 0;
    for (i, line) in buffer.split('\n').enumerate() {
        if i == target_line {
            let line_end = offset + line.len();
            let vis_start = sel_start.max(offset);
            let vis_end = sel_end.min(line_end);
            if vis_start >= vis_end {
                return None;
            }
            return Some((vis_start - offset, vis_end - offset));
        }
        offset += line.len() + 1;
    }
    None
}

/// Handle a key event in the current mode
pub fn handle_key(app: &mut App, key: KeyEvent) {
    match &app.mode {
        Mode::Navigate => handle_navigate(app, key),
        Mode::Search => handle_search(app, key),
        Mode::Edit => handle_edit(app, key),
        Mode::Move => handle_move(app, key),
        Mode::Triage => handle_triage(app, key),
        Mode::Confirm => handle_confirm(app, key),
    }
}

/// Drain any pending watcher events for a specific track (already handled via mtime).
/// Reloads remaining pending paths for other files.
fn drain_pending_for_track(app: &mut App, handled_track_id: &str) {
    if app.pending_reload_paths.is_empty() {
        return;
    }
    let skip_file = app.track_file(handled_track_id).map(|f| f.to_string());
    let remaining: Vec<std::path::PathBuf> = std::mem::take(&mut app.pending_reload_paths)
        .into_iter()
        .filter(|p| {
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
            skip_file.as_deref() != Some(name)
        })
        .collect();
    if !remaining.is_empty() {
        app.reload_changed_files(&remaining);
    }
}

fn handle_navigate(app: &mut App, key: KeyEvent) {
    // Conflict popup intercepts Esc
    if app.conflict_text.is_some() {
        if matches!(key.code, KeyCode::Esc) {
            app.conflict_text = None;
        }
        return;
    }

    // Help overlay intercepts ? and Esc
    if app.show_help {
        match key.code {
            KeyCode::Char('?') | KeyCode::Esc => {
                app.show_help = false;
            }
            _ => {}
        }
        return;
    }

    // Clear any transient status message on keypress
    app.status_message = None;

    // QQ quit: second Q confirms, any other key cancels
    if app.quit_pending {
        if matches!(
            (key.modifiers, key.code),
            (KeyModifiers::SHIFT, KeyCode::Char('Q'))
        ) {
            app.should_quit = true;
            return;
        } else {
            app.quit_pending = false;
        }
    }

    match (key.modifiers, key.code) {
        // Quit: Ctrl+Q
        (m, KeyCode::Char('q')) if m.contains(KeyModifiers::CONTROL) => {
            app.should_quit = true;
        }

        // Quit: Q (first press shows confirmation)
        (KeyModifiers::SHIFT, KeyCode::Char('Q')) => {
            app.quit_pending = true;
            app.status_message = Some("press Q again to quit".to_string());
        }

        // Esc: pop detail stack, close detail view, or clear search
        (_, KeyCode::Esc) => {
            if let View::Detail { .. } = &app.view {
                if let Some((parent_track, parent_task)) = app.detail_stack.pop() {
                    // Return to parent detail view, focusing on Subtasks region
                    let return_idx = app.detail_state.as_ref().map(|ds| ds.return_view_idx).unwrap_or(0);
                    app.detail_state = None;
                    app.view = View::Detail {
                        track_id: parent_track.clone(),
                        task_id: parent_task.clone(),
                    };
                    // Rebuild regions and focus on Subtasks
                    let regions = if let Some(track) = App::find_track_in_project(&app.project, &parent_track) {
                        if let Some(task) = crate::ops::task_ops::find_task_in_track(track, &parent_task) {
                            App::build_detail_regions(task)
                        } else {
                            vec![DetailRegion::Title]
                        }
                    } else {
                        vec![DetailRegion::Title]
                    };
                    let region = if regions.contains(&DetailRegion::Subtasks) {
                        DetailRegion::Subtasks
                    } else {
                        regions.first().copied().unwrap_or(DetailRegion::Title)
                    };
                    app.detail_state = Some(super::app::DetailState {
                        region,
                        scroll_offset: 0,
                        regions,
                        return_view_idx: return_idx,
                        editing: false,
                        edit_buffer: String::new(),
                        edit_cursor_line: 0,
                        edit_cursor_col: 0,
                        edit_original: String::new(),
                        subtask_cursor: 0,
                        flat_subtask_ids: Vec::new(),
                        multiline_selection_anchor: None,
                    });
                } else {
                    // Stack empty — return to track view
                    let return_idx = app.detail_state.as_ref().map(|ds| ds.return_view_idx).unwrap_or(0);
                    app.view = View::Track(return_idx);
                    app.close_detail_fully();
                }
            } else if app.last_search.is_some() {
                app.last_search = None;
                app.search_match_idx = 0;
                app.search_wrap_message = None;
                app.search_match_count = None;
                app.search_zero_confirmed = false;
            }
        }

        // Help overlay
        (KeyModifiers::NONE, KeyCode::Char('?')) => {
            app.show_help = true;
        }

        // Search: /
        (KeyModifiers::NONE, KeyCode::Char('/')) => {
            app.mode = Mode::Search;
            app.search_input.clear();
            app.search_draft.clear();
            app.search_history_index = None;
            app.search_wrap_message = None;
            app.search_match_count = None;
            app.search_zero_confirmed = false;
        }

        // n: note edit in detail view, or search next
        (KeyModifiers::NONE, KeyCode::Char('n')) => {
            if matches!(app.view, View::Detail { .. }) {
                detail_jump_to_region_and_edit(app, DetailRegion::Note);
            } else if app.last_search.is_some() {
                search_next(app, 1);
            }
        }
        (KeyModifiers::SHIFT, KeyCode::Char('N')) => {
            if app.last_search.is_some() {
                search_next(app, -1);
            }
        }

        // Tab switching: 1-9 for active tracks
        (KeyModifiers::NONE, KeyCode::Char(c @ '1'..='9')) => {
            let idx = (c as usize) - ('1' as usize);
            if idx < app.active_track_ids.len() {
                app.close_detail_fully();
                app.view = View::Track(idx);
            }
        }

        // Tab/Shift+Tab: next/prev editable region (detail view) or next/prev tab
        (KeyModifiers::NONE, KeyCode::Tab) => {
            if matches!(app.view, View::Detail { .. }) {
                detail_jump_editable(app, 1);
            } else {
                switch_tab(app, 1);
            }
        }
        (KeyModifiers::SHIFT, KeyCode::BackTab) => {
            if matches!(app.view, View::Detail { .. }) {
                detail_jump_editable(app, -1);
            } else {
                switch_tab(app, -1);
            }
        }

        // View switching
        (KeyModifiers::NONE, KeyCode::Char('i')) => {
            app.close_detail_fully();
            app.view = View::Inbox;
        }
        (KeyModifiers::NONE, KeyCode::Char('r')) => {
            app.close_detail_fully();
            app.view = View::Recent;
        }
        (KeyModifiers::NONE, KeyCode::Char('0') | KeyCode::Char('`')) => {
            app.close_detail_fully();
            app.view = View::Tracks;
        }

        // Cursor movement: up/down
        (KeyModifiers::NONE, KeyCode::Up | KeyCode::Char('k')) => {
            move_cursor(app, -1);
        }
        (KeyModifiers::NONE, KeyCode::Down | KeyCode::Char('j')) => {
            move_cursor(app, 1);
        }

        // Jump to top: g, Cmd+Up, or Home
        (KeyModifiers::NONE, KeyCode::Char('g')) => {
            jump_to_top(app);
        }
        (m, KeyCode::Up) if m.contains(KeyModifiers::SUPER) => {
            jump_to_top(app);
        }
        (_, KeyCode::Home) => {
            jump_to_top(app);
        }

        // Jump to bottom: G, Cmd+Down, or End
        (KeyModifiers::SHIFT, KeyCode::Char('G')) => {
            jump_to_bottom(app);
        }
        (m, KeyCode::Down) if m.contains(KeyModifiers::SUPER) => {
            jump_to_bottom(app);
        }
        (_, KeyCode::End) => {
            jump_to_bottom(app);
        }

        // Enter: open detail view (track view), triage (inbox), reopen (recent), or edit region (detail view)
        (KeyModifiers::NONE, KeyCode::Enter) => {
            if matches!(app.view, View::Inbox) {
                inbox_begin_triage(app);
            } else if matches!(app.view, View::Recent) {
                reopen_recent_task(app);
            } else {
                handle_enter(app);
            }
        }

        // Expand/collapse (track view only)
        (KeyModifiers::NONE, KeyCode::Right | KeyCode::Char('l')) => {
            expand_or_enter(app);
        }
        (KeyModifiers::NONE, KeyCode::Left | KeyCode::Char('h')) => {
            collapse_or_parent(app);
        }

        // Task state changes (track view) or reopen (recent view)
        (KeyModifiers::NONE, KeyCode::Char(' ')) => {
            if matches!(app.view, View::Recent) {
                reopen_recent_task(app);
            } else {
                task_state_action(app, StateAction::Cycle);
            }
        }
        (KeyModifiers::NONE, KeyCode::Char('x')) => {
            if matches!(app.view, View::Inbox) {
                inbox_delete_item(app);
            } else {
                task_state_action(app, StateAction::Done);
            }
        }
        (KeyModifiers::NONE, KeyCode::Char('b')) => {
            task_state_action(app, StateAction::ToggleBlocked);
        }
        (KeyModifiers::NONE, KeyCode::Char('~')) => {
            task_state_action(app, StateAction::ToggleParked);
        }

        // Add task (track view) or add inbox item (inbox view)
        (KeyModifiers::NONE, KeyCode::Char('a')) => {
            if matches!(app.view, View::Inbox) {
                inbox_add_item(app);
            } else {
                add_task_action(app, AddPosition::Bottom);
            }
        }
        (KeyModifiers::NONE, KeyCode::Char('o') | KeyCode::Char('-')) => {
            if matches!(app.view, View::Inbox) {
                inbox_insert_after(app);
            } else {
                add_task_action(app, AddPosition::AfterCursor);
            }
        }
        (KeyModifiers::NONE, KeyCode::Char('p')) => {
            add_task_action(app, AddPosition::Top);
        }
        (KeyModifiers::SHIFT, KeyCode::Char('A')) => {
            add_subtask_action(app);
        }

        // Inline title edit (track view), edit (inbox view), or enter edit mode (detail view)
        (KeyModifiers::NONE, KeyCode::Char('e')) => {
            if matches!(app.view, View::Detail { .. }) {
                detail_enter_edit(app);
            } else if matches!(app.view, View::Inbox) {
                inbox_edit_title(app);
            } else {
                enter_title_edit(app);
            }
        }

        // Tag edit: # — detail view jump to tags region, inbox tag edit, or inline tag edit in track view
        (KeyModifiers::NONE, KeyCode::Char('#')) => {
            if matches!(app.view, View::Detail { .. }) {
                detail_jump_to_region_and_edit(app, DetailRegion::Tags);
            } else if matches!(app.view, View::Inbox) {
                inbox_edit_tags(app);
            } else {
                enter_tag_edit(app);
            }
        }
        (KeyModifiers::NONE, KeyCode::Char('@')) => {
            if matches!(app.view, View::Detail { .. }) {
                detail_jump_to_region_and_edit(app, DetailRegion::Refs);
            }
        }
        (KeyModifiers::NONE, KeyCode::Char('d')) => {
            if matches!(app.view, View::Detail { .. }) {
                detail_jump_to_region_and_edit(app, DetailRegion::Deps);
            }
        }

        // Toggle cc tag on task
        (KeyModifiers::NONE, KeyCode::Char('c')) => {
            toggle_cc_tag(app);
        }

        // Set cc-focus to current track
        (KeyModifiers::SHIFT, KeyCode::Char('C')) => {
            set_cc_focus_current(app);
        }

        // Move mode (track, tracks, or inbox view)
        (KeyModifiers::NONE, KeyCode::Char('m')) => {
            if matches!(app.view, View::Inbox) {
                inbox_enter_move_mode(app);
            } else {
                enter_move_mode(app);
            }
        }

        // Redo: Z, Ctrl+Y, or Ctrl+Shift+Z (must be checked BEFORE Ctrl+Z/z undo)
        (m, KeyCode::Char('y')) if m.contains(KeyModifiers::CONTROL) => {
            perform_redo(app);
        }
        (m, KeyCode::Char('z') | KeyCode::Char('Z'))
            if m.contains(KeyModifiers::CONTROL) && m.contains(KeyModifiers::SHIFT) =>
        {
            perform_redo(app);
        }
        (KeyModifiers::SHIFT, KeyCode::Char('Z')) => {
            perform_redo(app);
        }

        // Undo: u, z, or Ctrl+Z
        (KeyModifiers::NONE, KeyCode::Char('u') | KeyCode::Char('z')) => {
            perform_undo(app);
        }
        (m, KeyCode::Char('z')) if m.contains(KeyModifiers::CONTROL) => {
            perform_undo(app);
        }

        _ => {}
    }
}

fn handle_search(app: &mut App, key: KeyEvent) {
    match (key.modifiers, key.code) {
        // Cancel search
        (_, KeyCode::Esc) => {
            app.mode = Mode::Navigate;
            app.search_input.clear();
            app.search_history_index = None;
            // Recompute match count for last_search (mode is now Navigate)
            if let Some(re) = app.active_search_re() {
                app.search_match_count = Some(count_matches_for_pattern(app, &re));
            } else {
                app.search_match_count = None;
            }
        }

        // Execute search
        (_, KeyCode::Enter) => {
            if !app.search_input.is_empty() {
                let query = app.search_input.clone();
                // Add to history (dedup: remove previous occurrence, then push to front)
                app.search_history.retain(|s| s != &query);
                app.search_history.insert(0, query);
                app.search_history.truncate(200);

                app.last_search = Some(app.search_input.clone());
                execute_search_dir(app, 0);
                app.search_zero_confirmed = app.search_match_count == Some(0);
            }
            app.mode = Mode::Navigate;
            app.search_input.clear();
            app.search_history_index = None;
            app.search_wrap_message = None;
        }

        // History navigation: Up = older
        (_, KeyCode::Up) => {
            if !app.search_history.is_empty() {
                match app.search_history_index {
                    None => {
                        app.search_draft = app.search_input.clone();
                        app.search_history_index = Some(0);
                        app.search_input = app.search_history[0].clone();
                    }
                    Some(idx) => {
                        let next = idx + 1;
                        if next < app.search_history.len() {
                            app.search_history_index = Some(next);
                            app.search_input = app.search_history[next].clone();
                        }
                    }
                }
                update_match_count(app);
            }
        }

        // History navigation: Down = newer
        (_, KeyCode::Down) => {
            let changed = match app.search_history_index {
                None => false,
                Some(0) => {
                    app.search_history_index = None;
                    app.search_input = app.search_draft.clone();
                    true
                }
                Some(idx) => {
                    let prev = idx - 1;
                    app.search_history_index = Some(prev);
                    app.search_input = app.search_history[prev].clone();
                    true
                }
            };
            if changed {
                update_match_count(app);
            }
        }

        // Backspace
        (_, KeyCode::Backspace) => {
            app.search_input.pop();
            if app.search_history_index.is_some() {
                app.search_history_index = None;
                app.search_draft.clear();
            }
            update_match_count(app);
        }

        // Type character
        (KeyModifiers::NONE | KeyModifiers::SHIFT, KeyCode::Char(c)) => {
            app.search_input.push(c);
            if app.search_history_index.is_some() {
                app.search_history_index = None;
                app.search_draft.clear();
            }
            update_match_count(app);
        }

        _ => {}
    }
}

// ---------------------------------------------------------------------------
// Undo / Redo
// ---------------------------------------------------------------------------

fn perform_undo(app: &mut App) {
    let inbox = app.project.inbox.as_mut();
    if let Some(nav) = app.undo_stack.undo(&mut app.project.tracks, inbox) {
        apply_nav_side_effects(app, &nav, true);
        navigate_to_undo_target(app, &nav);
    }
}

fn perform_redo(app: &mut App) {
    let inbox = app.project.inbox.as_mut();
    if let Some(nav) = app.undo_stack.redo(&mut app.project.tracks, inbox) {
        apply_nav_side_effects(app, &nav, false);
        navigate_to_undo_target(app, &nav);
    }
}

/// Apply side effects that undo/redo can't handle internally (e.g., config changes, saves).
fn apply_nav_side_effects(app: &mut App, nav: &UndoNavTarget, is_undo: bool) {
    match nav {
        UndoNavTarget::Task { track_id, .. } => {
            let _ = app.save_track(track_id);
        }
        UndoNavTarget::TracksView { track_id } => {
            // TrackMove: reorder in config and save
            // Find the operation on the redo/undo stack (it was just moved there)
            let op = if is_undo {
                app.undo_stack.peek_last_redo()
            } else {
                app.undo_stack.peek_last_undo()
            };
            if let Some(Operation::TrackMove { old_index, new_index, .. }) = op {
                let target_index = if is_undo { *old_index } else { *new_index };
                let _ = crate::ops::track_ops::reorder_tracks(
                    &mut app.project.config,
                    track_id,
                    target_index,
                );
                app.active_track_ids = app
                    .project
                    .config
                    .tracks
                    .iter()
                    .filter(|t| t.state == "active")
                    .map(|t| t.id.clone())
                    .collect();
                save_config(app);
            }
        }
        UndoNavTarget::Inbox { .. } => {
            // Inbox operations: check if a track was also affected (triage undo)
            let triage_track_id = {
                let op = if is_undo {
                    app.undo_stack.peek_last_redo()
                } else {
                    app.undo_stack.peek_last_undo()
                };
                if let Some(Operation::InboxTriage { track_id, .. }) = op {
                    Some(track_id.clone())
                } else {
                    None
                }
            };
            if let Some(tid) = triage_track_id {
                let _ = app.save_track(&tid);
            }
            let _ = app.save_inbox();
        }
        UndoNavTarget::Recent { .. } => {
            // Reopen undo/redo: save the affected track
            let reopen_track_id = {
                let op = if is_undo {
                    app.undo_stack.peek_last_redo()
                } else {
                    app.undo_stack.peek_last_undo()
                };
                if let Some(Operation::Reopen { track_id, .. }) = op {
                    Some(track_id.clone())
                } else {
                    None
                }
            };
            if let Some(tid) = reopen_track_id {
                let _ = app.save_track(&tid);
            }
        }
    }
}

/// Navigate the UI to show the item affected by an undo/redo operation.
fn navigate_to_undo_target(app: &mut App, nav: &UndoNavTarget) {
    match nav {
        UndoNavTarget::Task {
            track_id,
            task_id,
            detail_region,
            task_removed,
            position_hint,
        } => {
            // Find the track index in active tracks
            let track_idx = match app.active_track_ids.iter().position(|id| id == track_id) {
                Some(idx) => idx,
                None => return, // Track not active (shelved) — undo still applied, just no navigation
            };

            // Check if we're already in detail view for the same task
            let stay_in_detail = matches!(
                &app.view,
                View::Detail { track_id: dt, task_id: di } if dt == track_id && di == task_id
            );

            // Close detail view only if navigating to a different task
            if matches!(app.view, View::Detail { .. }) && !stay_in_detail {
                app.close_detail_fully();
            }

            // Switch to the target track (unless staying in detail view)
            if !stay_in_detail {
                app.view = View::Track(track_idx);
            }

            if *task_removed {
                // Task was removed — clamp cursor to position_hint
                let flat_items = app.build_flat_items(track_id);
                let hint = position_hint.unwrap_or(0);
                let clamped = hint.min(flat_items.len().saturating_sub(1));
                let state = app.get_track_state(track_id);
                state.cursor = clamped;
            } else {
                // Move cursor to the affected task and flash it
                move_cursor_to_task(app, track_id, task_id);
                app.flash_task(task_id);

                // If the operation targets a detail region, navigate to it
                if let Some(region) = detail_region {
                    if !stay_in_detail {
                        let task_exists = App::find_track_in_project(&app.project, track_id)
                            .and_then(|track| task_ops::find_task_in_track(track, task_id))
                            .is_some();
                        if task_exists {
                            app.open_detail(track_id.clone(), task_id.clone());
                        }
                    }
                    if let Some(ref mut ds) = app.detail_state
                        && ds.regions.contains(region)
                    {
                        ds.region = *region;
                    }
                }
            }
        }
        UndoNavTarget::TracksView { track_id } => {
            // Close detail view if open
            if matches!(app.view, View::Detail { .. }) {
                app.close_detail_fully();
            }

            app.view = View::Tracks;

            // Move cursor to the track row
            let active_tracks: Vec<&str> = app
                .project
                .config
                .tracks
                .iter()
                .filter(|t| t.state == "active")
                .map(|t| t.id.as_str())
                .collect();
            if let Some(idx) = active_tracks.iter().position(|id| *id == track_id) {
                app.tracks_cursor = idx;
            }

            app.flash_track(track_id);
        }
        UndoNavTarget::Inbox { cursor } => {
            if matches!(app.view, View::Detail { .. }) {
                app.close_detail_fully();
            }
            app.view = View::Inbox;
            if let Some(idx) = cursor {
                let count = app.inbox_count();
                app.inbox_cursor = if count > 0 { (*idx).min(count - 1) } else { 0 };
            }
        }
        UndoNavTarget::Recent { cursor } => {
            if matches!(app.view, View::Detail { .. }) {
                app.close_detail_fully();
            }
            app.view = View::Recent;
            if let Some(c) = cursor {
                let count = count_recent_tasks(app);
                app.recent_cursor = if count > 0 { (*c).min(count - 1) } else { 0 };
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Task state changes
// ---------------------------------------------------------------------------

enum StateAction {
    Cycle,
    Done,
    ToggleBlocked,
    ToggleParked,
}

/// Apply a state change to the task under the cursor (track view only).
fn task_state_action(app: &mut App, action: StateAction) {
    let info = match app.cursor_task_id() {
        Some(info) => info,
        None => return,
    };
    let (track_id, task_id, _section) = info;

    let track = match app.find_track_mut(&track_id) {
        Some(t) => t,
        None => return,
    };
    let task = match task_ops::find_task_mut_in_track(track, &task_id) {
        Some(t) => t,
        None => return,
    };

    // Capture old state for undo
    let old_state = task.state;
    let old_resolved = task
        .metadata
        .iter()
        .find_map(|m| {
            if let Metadata::Resolved(d) = m {
                Some(d.clone())
            } else {
                None
            }
        });

    match action {
        StateAction::Cycle => task_ops::cycle_state(task),
        StateAction::Done => task_ops::set_done(task),
        StateAction::ToggleBlocked => task_ops::set_blocked(task),
        StateAction::ToggleParked => task_ops::set_parked(task),
    }

    let new_state = task.state;
    let new_resolved = task
        .metadata
        .iter()
        .find_map(|m| {
            if let Metadata::Resolved(d) = m {
                Some(d.clone())
            } else {
                None
            }
        });

    // Only push undo if state actually changed
    if old_state != new_state {
        app.undo_stack.push(Operation::StateChange {
            track_id: track_id.clone(),
            task_id: task_id.clone(),
            old_state,
            new_state,
            old_resolved,
            new_resolved,
        });
    }

    let _ = app.save_track(&track_id);
}

// ---------------------------------------------------------------------------
// CC tag / CC focus
// ---------------------------------------------------------------------------

/// Toggle the `cc` tag on the task under the cursor (track view only).
fn toggle_cc_tag(app: &mut App) {
    let (track_id, task_id, _) = match app.cursor_task_id() {
        Some(info) => info,
        None => return,
    };

    // Check if task already has cc tag (immutable borrow)
    let has_cc = App::find_track_in_project(&app.project, &track_id)
        .and_then(|t| task_ops::find_task_in_track(t, &task_id))
        .map(|t| t.tags.iter().any(|tag| tag == "cc"))
        .unwrap_or(false);

    let track = match app.find_track_mut(&track_id) {
        Some(t) => t,
        None => return,
    };

    if has_cc {
        let _ = task_ops::remove_tag(track, &task_id, "cc");
    } else {
        let _ = task_ops::add_tag(track, &task_id, "cc");
    }
    let _ = app.save_track(&track_id);
}

/// Set the current track as cc-focus (track view or tracks view).
fn set_cc_focus_current(app: &mut App) {
    let track_id = match &app.view {
        View::Track(idx) => match app.active_track_ids.get(*idx) {
            Some(id) => id.clone(),
            None => return,
        },
        View::Tracks => {
            let active_tracks: Vec<&str> = app
                .project
                .config
                .tracks
                .iter()
                .filter(|t| t.state == "active")
                .map(|t| t.id.as_str())
                .collect();
            match active_tracks.get(app.tracks_cursor) {
                Some(id) => id.to_string(),
                None => return,
            }
        }
        _ => return,
    };

    // Toggle: if already cc-focus, clear it; otherwise set it
    if app.project.config.agent.cc_focus.as_deref() == Some(&track_id) {
        app.project.config.agent.cc_focus = None;
    } else {
        app.project.config.agent.cc_focus = Some(track_id.clone());
    }

    save_config(app);
    app.status_message = match &app.project.config.agent.cc_focus {
        Some(id) => Some(format!("cc-focus \u{2192} {}", id)),
        None => Some("cc-focus cleared".to_string()),
    };
}

/// Save the project config to project.toml.
fn save_config(app: &mut App) {
    let config_text = match toml::to_string_pretty(&app.project.config) {
        Ok(s) => s,
        Err(_) => return,
    };
    let config_path = app.project.frame_dir.join("project.toml");
    let _ = std::fs::write(&config_path, config_text);
    app.last_save_at = Some(std::time::Instant::now());
}

// ---------------------------------------------------------------------------
// Add task
// ---------------------------------------------------------------------------

enum AddPosition {
    Top,
    Bottom,
    AfterCursor,
}

/// Add a new task and enter EDIT mode for its title (track view only).
fn add_task_action(app: &mut App, pos: AddPosition) {
    let track_id = match app.current_track_id() {
        Some(id) => id.to_string(),
        None => return,
    };
    let prefix = match app.track_prefix(&track_id) {
        Some(p) => p.to_string(),
        None => return,
    };

    // Save cursor position for restore on cancel
    let saved_cursor = app.track_states.get(&track_id).map(|s| s.cursor);

    let insert_pos = match pos {
        AddPosition::Top => InsertPosition::Top,
        AddPosition::Bottom => InsertPosition::Bottom,
        AddPosition::AfterCursor => {
            // Insert after the cursor's task (top-level only)
            match app.cursor_task_id() {
                Some((_, task_id, _)) => InsertPosition::After(task_id),
                None => InsertPosition::Bottom,
            }
        }
    };

    let track = match app.find_track_mut(&track_id) {
        Some(t) => t,
        None => return,
    };

    let task_id = match task_ops::add_task(track, String::new(), insert_pos, &prefix) {
        Ok(id) => id,
        Err(_) => return,
    };

    // Enter EDIT mode for the new task's title
    app.edit_buffer.clear();
    app.edit_cursor = 0;
    app.edit_target = Some(EditTarget::NewTask {
        task_id: task_id.clone(),
        track_id: track_id.clone(),
        parent_id: None,
    });
    app.pre_edit_cursor = saved_cursor;
    app.edit_history = Some(EditHistory::new("", 0, 0));
    app.mode = Mode::Edit;

    // Move cursor to the new task
    move_cursor_to_task(app, &track_id, &task_id);
}

/// Add a subtask to the selected task and enter EDIT mode.
fn add_subtask_action(app: &mut App) {
    let (track_id, parent_id, _) = match app.cursor_task_id() {
        Some(info) => info,
        None => return,
    };

    // Save cursor position for restore on cancel
    app.pre_edit_cursor = app.track_states.get(&track_id).map(|s| s.cursor);

    let track = match app.find_track_mut(&track_id) {
        Some(t) => t,
        None => return,
    };

    let sub_id = match task_ops::add_subtask(track, &parent_id, String::new()) {
        Ok(id) => id,
        Err(_) => return,
    };

    // Expand the parent so the new subtask is visible
    {
        let flat_items = app.build_flat_items(&track_id);
        let track = App::find_track_in_project(&app.project, &track_id);
        if let Some(track) = track {
            for item in &flat_items {
                if let FlatItem::Task { section, path, .. } = item {
                    if let Some(task) = resolve_task_from_flat(track, *section, path) {
                        if task.id.as_deref() == Some(&parent_id) {
                            let key =
                                crate::tui::app::task_expand_key(task, *section, path);
                            let state = app.get_track_state(&track_id);
                            state.expanded.insert(key);
                            break;
                        }
                    }
                }
            }
        }
    }

    // Enter EDIT mode
    app.edit_buffer.clear();
    app.edit_cursor = 0;
    app.edit_target = Some(EditTarget::NewTask {
        task_id: sub_id.clone(),
        track_id: track_id.clone(),
        parent_id: Some(parent_id),
    });
    app.edit_history = Some(EditHistory::new("", 0, 0));
    app.mode = Mode::Edit;

    // Move cursor to the new subtask
    move_cursor_to_task(app, &track_id, &sub_id);
}

// ---------------------------------------------------------------------------
// Inline title editing
// ---------------------------------------------------------------------------

/// Enter EDIT mode to edit the selected task's title.
fn enter_title_edit(app: &mut App) {
    let (track_id, task_id, _) = match app.cursor_task_id() {
        Some(info) => info,
        None => return,
    };

    let track = match App::find_track_in_project(&app.project, &track_id) {
        Some(t) => t,
        None => return,
    };

    let task = match task_ops::find_task_in_track(track, &task_id) {
        Some(t) => t,
        None => return,
    };

    let original_title = task.title.clone();
    app.edit_buffer = original_title.clone();
    app.edit_cursor = app.edit_buffer.len();
    app.pre_edit_cursor = None;
    app.edit_target = Some(EditTarget::ExistingTitle {
        task_id,
        track_id,
        original_title,
    });
    app.edit_history = Some(EditHistory::new(&app.edit_buffer, app.edit_cursor, 0));
    app.mode = Mode::Edit;
}

fn enter_tag_edit(app: &mut App) {
    let (track_id, task_id, _) = match app.cursor_task_id() {
        Some(info) => info,
        None => return,
    };

    let track = match App::find_track_in_project(&app.project, &track_id) {
        Some(t) => t,
        None => return,
    };

    let task = match task_ops::find_task_in_track(track, &task_id) {
        Some(t) => t,
        None => return,
    };

    let original_tags = task.tags.iter().map(|t| format!("#{}", t)).collect::<Vec<_>>().join(" ");
    app.edit_buffer = if original_tags.is_empty() { String::new() } else { format!("{} ", original_tags) };
    app.edit_cursor = app.edit_buffer.len();
    app.pre_edit_cursor = None;
    app.edit_target = Some(EditTarget::ExistingTags {
        task_id,
        track_id,
        original_tags,
    });
    app.edit_history = Some(EditHistory::new(&app.edit_buffer, app.edit_cursor, 0));

    // Activate tag autocomplete
    let candidates = app.collect_all_tags();
    if !candidates.is_empty() {
        let mut ac = AutocompleteState::new(AutocompleteKind::Tag, candidates);
        let filter_text = autocomplete_filter_text(&app.edit_buffer, AutocompleteKind::Tag);
        ac.filter(&filter_text);
        app.autocomplete = Some(ac);
    }

    app.mode = Mode::Edit;
}

// ---------------------------------------------------------------------------
// EDIT mode input
// ---------------------------------------------------------------------------

fn handle_edit(app: &mut App, key: KeyEvent) {
    // Check if we're in multi-line note editing in detail view
    let is_detail_multiline = app.detail_state.as_ref().is_some_and(|ds| {
        ds.editing && ds.region == DetailRegion::Note
    }) && app.edit_target.is_none();

    if is_detail_multiline {
        handle_detail_multiline_edit(app, key);
        return;
    }

    // Check if we're editing a detail region (single-line)
    let is_detail_edit = matches!(app.view, View::Detail { .. }) && app.detail_state.as_ref().is_some_and(|ds| ds.editing);

    // Handle autocomplete navigation when dropdown is visible
    let ac_visible = app.autocomplete.as_ref().is_some_and(|ac| ac.visible && !ac.filtered.is_empty());

    if ac_visible {
        match (key.modifiers, key.code) {
            // Navigate autocomplete entries
            (KeyModifiers::NONE, KeyCode::Up) => {
                if let Some(ac) = &mut app.autocomplete {
                    ac.move_up();
                }
                return;
            }
            (KeyModifiers::NONE, KeyCode::Down) => {
                if let Some(ac) = &mut app.autocomplete {
                    ac.move_down();
                }
                return;
            }
            // Select entry
            (KeyModifiers::NONE, KeyCode::Tab) => {
                autocomplete_accept(app);
                return;
            }
            // Dismiss autocomplete on Esc (hide, don't destroy — typing will re-show)
            (_, KeyCode::Esc) => {
                if let Some(ac) = &mut app.autocomplete {
                    ac.visible = false;
                }
                return;
            }
            // Enter: accept autocomplete selection and confirm edit
            (_, KeyCode::Enter) => {
                // Accept autocomplete if a candidate is selected AND the user is
                // actually completing a partial word (not just confirming an already-typed entry)
                if let Some(ac) = &app.autocomplete {
                    if let Some(entry) = ac.selected_entry() {
                        let filter = autocomplete_filter_text(&app.edit_buffer, ac.kind);
                        if filter != entry {
                            autocomplete_accept(app);
                        }
                    }
                }
                app.autocomplete = None;
                // Fall through to confirm
                if is_detail_edit {
                    confirm_detail_edit(app);
                } else {
                    confirm_edit(app);
                }
                return;
            }
            _ => {
                // For other keys, dismiss autocomplete and fall through to normal handling
                // (characters will re-trigger filtering below)
            }
        }
    }

    // Handle selection anchor for arrow keys as a pre-pass
    let has_shift = key.modifiers.contains(KeyModifiers::SHIFT);
    let is_arrow = matches!(key.code, KeyCode::Left | KeyCode::Right);
    if is_arrow {
        if has_shift {
            // Start or extend selection
            if app.edit_selection_anchor.is_none() {
                app.edit_selection_anchor = Some(app.edit_cursor);
            }
        } else {
            // Clear selection on non-shift movement
            app.edit_selection_anchor = None;
        }
    }

    match (key.modifiers, key.code) {
        // Confirm edit
        (_, KeyCode::Enter) => {
            app.autocomplete = None;
            app.edit_history = None;
            app.edit_selection_anchor = None;
            if is_detail_edit {
                confirm_detail_edit(app);
            } else {
                confirm_edit(app);
            }
        }
        // Cancel edit
        (_, KeyCode::Esc) => {
            app.autocomplete = None;
            app.edit_history = None;
            app.edit_selection_anchor = None;
            if is_detail_edit {
                cancel_detail_edit(app);
            } else {
                cancel_edit(app);
            }
        }
        // Select all (Ctrl+A)
        (m, KeyCode::Char('a')) if m.contains(KeyModifiers::CONTROL) => {
            app.edit_selection_anchor = Some(0);
            app.edit_cursor = app.edit_buffer.len();
        }
        // Copy (Ctrl+C)
        (m, KeyCode::Char('c')) if m.contains(KeyModifiers::CONTROL) => {
            if let Some(text) = app.get_selection_text() {
                clipboard_set(&text);
            }
        }
        // Cut (Ctrl+X)
        (m, KeyCode::Char('x')) if m.contains(KeyModifiers::CONTROL) => {
            if let Some(text) = app.get_selection_text() {
                clipboard_set(&text);
                app.delete_selection();
                if let Some(eh) = &mut app.edit_history {
                    eh.snapshot(&app.edit_buffer, app.edit_cursor, 0);
                }
                update_autocomplete_filter(app);
            }
        }
        // Paste (Ctrl+V)
        (m, KeyCode::Char('v')) if m.contains(KeyModifiers::CONTROL) => {
            if let Some(text) = clipboard_get() {
                app.delete_selection();
                app.edit_buffer.insert_str(app.edit_cursor, &text);
                app.edit_cursor += text.len();
                if let Some(eh) = &mut app.edit_history {
                    eh.snapshot(&app.edit_buffer, app.edit_cursor, 0);
                }
                update_autocomplete_filter(app);
            }
        }
        // Inline undo (Ctrl+Z)
        (m, KeyCode::Char('z')) if m.contains(KeyModifiers::CONTROL) => {
            app.edit_selection_anchor = None;
            if let Some(eh) = &mut app.edit_history {
                if let Some((buf, pos, _)) = eh.undo() {
                    app.edit_buffer = buf.to_string();
                    app.edit_cursor = pos;
                }
            }
            update_autocomplete_filter(app);
        }
        // Inline redo (Ctrl+Y or Ctrl+Shift+Z)
        (m, KeyCode::Char('y')) if m.contains(KeyModifiers::CONTROL) => {
            app.edit_selection_anchor = None;
            if let Some(eh) = &mut app.edit_history {
                if let Some((buf, pos, _)) = eh.redo() {
                    app.edit_buffer = buf.to_string();
                    app.edit_cursor = pos;
                }
            }
            update_autocomplete_filter(app);
        }
        (m, KeyCode::Char('Z')) if m.contains(KeyModifiers::CONTROL) && m.contains(KeyModifiers::SHIFT) => {
            app.edit_selection_anchor = None;
            if let Some(eh) = &mut app.edit_history {
                if let Some((buf, pos, _)) = eh.redo() {
                    app.edit_buffer = buf.to_string();
                    app.edit_cursor = pos;
                }
            }
            update_autocomplete_filter(app);
        }
        // Cursor movement: single character left/right (with or without Shift)
        (_, KeyCode::Left) if !key.modifiers.contains(KeyModifiers::ALT)
            && !key.modifiers.contains(KeyModifiers::CONTROL)
            && !key.modifiers.contains(KeyModifiers::SUPER) =>
        {
            if app.edit_cursor > 0 {
                app.edit_cursor -= 1;
            }
        }
        (_, KeyCode::Right) if !key.modifiers.contains(KeyModifiers::ALT)
            && !key.modifiers.contains(KeyModifiers::CONTROL)
            && !key.modifiers.contains(KeyModifiers::SUPER) =>
        {
            if app.edit_cursor < app.edit_buffer.len() {
                app.edit_cursor += 1;
            }
        }
        // Jump to start/end of line (Cmd/Super, with or without Shift)
        (m, KeyCode::Left) if m.contains(KeyModifiers::SUPER) => {
            app.edit_cursor = 0;
        }
        (m, KeyCode::Right) if m.contains(KeyModifiers::SUPER) => {
            app.edit_cursor = app.edit_buffer.len();
        }
        // Word movement (Alt or Ctrl, with or without Shift)
        (m, KeyCode::Left) if m.contains(KeyModifiers::ALT) || m.contains(KeyModifiers::CONTROL) => {
            app.edit_cursor = word_boundary_left(&app.edit_buffer, app.edit_cursor);
        }
        (m, KeyCode::Right) if m.contains(KeyModifiers::ALT) || m.contains(KeyModifiers::CONTROL) => {
            app.edit_cursor = word_boundary_right(&app.edit_buffer, app.edit_cursor);
        }
        // Backspace: delete selection or single char
        (KeyModifiers::NONE, KeyCode::Backspace) => {
            if !app.delete_selection() {
                if app.edit_cursor > 0 {
                    app.edit_buffer.remove(app.edit_cursor - 1);
                    app.edit_cursor -= 1;
                }
            }
            if let Some(eh) = &mut app.edit_history {
                eh.snapshot(&app.edit_buffer, app.edit_cursor, 0);
            }
            update_autocomplete_filter(app);
        }
        // Word backspace (Alt or Ctrl)
        (m, KeyCode::Backspace) if m.contains(KeyModifiers::ALT) || m.contains(KeyModifiers::CONTROL) => {
            if !app.delete_selection() {
                let new_pos = word_boundary_left(&app.edit_buffer, app.edit_cursor);
                app.edit_buffer.drain(new_pos..app.edit_cursor);
                app.edit_cursor = new_pos;
            }
            if let Some(eh) = &mut app.edit_history {
                eh.snapshot(&app.edit_buffer, app.edit_cursor, 0);
            }
            update_autocomplete_filter(app);
        }
        // Type character: replace selection if any
        (KeyModifiers::NONE | KeyModifiers::SHIFT, KeyCode::Char(c)) => {
            app.delete_selection();
            app.edit_buffer.insert(app.edit_cursor, c);
            app.edit_cursor += 1;
            if let Some(eh) = &mut app.edit_history {
                eh.snapshot(&app.edit_buffer, app.edit_cursor, 0);
            }
            update_autocomplete_filter(app);
        }
        _ => {}
    }
}

fn confirm_edit(app: &mut App) {
    let target = match app.edit_target.take() {
        Some(t) => t,
        None => {
            app.mode = Mode::Navigate;
            return;
        }
    };

    let title = app.edit_buffer.clone();
    app.mode = Mode::Navigate;
    app.pre_edit_cursor = None;

    match target {
        EditTarget::NewTask {
            task_id,
            track_id,
            parent_id,
        } => {
            // Use mtime to detect external changes (independent of watcher timing)
            let changed = app.track_changed_on_disk(&track_id);

            if title.trim().is_empty() {
                // Empty title: discard the placeholder
                if changed {
                    // Reload from disk — discards in-memory placeholder and picks up
                    // any external changes atomically
                    if let Some(disk_track) = app.read_track_from_disk(&track_id) {
                        app.replace_track(&track_id, disk_track);
                    }
                } else {
                    // No external changes — remove placeholder from memory and save
                    let track = match app.find_track_mut(&track_id) {
                        Some(t) => t,
                        None => return,
                    };
                    if let Some(ref pid) = parent_id {
                        if let Some(parent) = task_ops::find_task_mut_in_track(track, pid) {
                            parent.subtasks.retain(|t| t.id.as_deref() != Some(&task_id));
                            parent.mark_dirty();
                        }
                    } else {
                        remove_task_from_section(track, &task_id, SectionKind::Backlog);
                    }
                    let _ = app.save_track(&track_id);
                }
            } else if changed {
                // Non-empty title, file changed externally — merge
                let prefix = app.track_prefix(&track_id).unwrap_or("").to_string();
                if let Some(disk_track) = app.read_track_from_disk(&track_id) {
                    // For subtasks: check if parent still exists on disk
                    if let Some(ref pid) = parent_id
                        && task_ops::find_task_in_track(&disk_track, pid).is_none()
                    {
                        app.conflict_text = Some(title);
                        app.replace_track(&track_id, disk_track);
                        drain_pending_for_track(app, &track_id);
                        return;
                    }
                    // Replace in-memory with disk version, then add our new task on top
                    app.replace_track(&track_id, disk_track);

                    let track = match app.find_track_mut(&track_id) {
                        Some(t) => t,
                        None => return,
                    };
                    if let Some(ref pid) = parent_id {
                        let _ = task_ops::add_subtask(track, pid, title.clone());
                    } else {
                        let _ = task_ops::add_task(
                            track,
                            title.clone(),
                            InsertPosition::Bottom,
                            &prefix,
                        );
                    }

                    if let Some(ref pid) = parent_id {
                        app.undo_stack.push_sync_marker();
                        app.undo_stack.push(Operation::SubtaskAdd {
                            track_id: track_id.clone(),
                            parent_id: pid.clone(),
                            task_id: task_id.clone(),
                        });
                    } else {
                        app.undo_stack.push_sync_marker();
                        let pos_idx = App::find_track_in_project(&app.project, &track_id)
                            .and_then(|t| {
                                t.backlog()
                                    .iter()
                                    .position(|t| t.id.as_deref() == Some(&task_id))
                            })
                            .unwrap_or(0);
                        app.undo_stack.push(Operation::TaskAdd {
                            track_id: track_id.clone(),
                            task_id: task_id.clone(),
                            position_index: pos_idx,
                        });
                    }
                    let _ = app.save_track(&track_id);
                }
            } else {
                // No external changes — apply title to the in-memory placeholder and save
                let track = match app.find_track_mut(&track_id) {
                    Some(t) => t,
                    None => return,
                };
                let _ = task_ops::edit_title(track, &task_id, title.clone());

                if let Some(pid) = &parent_id {
                    app.undo_stack.push(Operation::SubtaskAdd {
                        track_id: track_id.clone(),
                        parent_id: pid.clone(),
                        task_id: task_id.clone(),
                    });
                } else {
                    let pos_idx = App::find_track_in_project(&app.project, &track_id)
                        .and_then(|t| {
                            t.backlog()
                                .iter()
                                .position(|t| t.id.as_deref() == Some(&task_id))
                        })
                        .unwrap_or(0);

                    app.undo_stack.push(Operation::TaskAdd {
                        track_id: track_id.clone(),
                        task_id: task_id.clone(),
                        position_index: pos_idx,
                    });
                }
                let _ = app.save_track(&track_id);
            }
            drain_pending_for_track(app, &track_id);
        }
        EditTarget::ExistingTitle {
            task_id,
            track_id,
            original_title,
        } => {
            if !title.trim().is_empty() && title != original_title {
                // Use mtime to detect external changes (independent of watcher timing)
                let changed = app.track_changed_on_disk(&track_id);

                if changed {
                    // File changed externally — read from disk and check for conflict
                    if let Some(disk_track) = app.read_track_from_disk(&track_id) {
                        let disk_task = task_ops::find_task_in_track(&disk_track, &task_id);
                        let is_conflict = match disk_task {
                            Some(dt) => dt.title != original_title,
                            None => true,
                        };

                        if is_conflict {
                            // Don't save — reload from disk, show conflict popup
                            app.conflict_text = Some(title);
                            app.replace_track(&track_id, disk_track);
                        } else {
                            // No conflict — merge: use disk version, apply edit, save
                            app.replace_track(&track_id, disk_track);
                            let track = match app.find_track_mut(&track_id) {
                                Some(t) => t,
                                None => return,
                            };
                            let _ = task_ops::edit_title(track, &task_id, title.clone());

                            app.undo_stack.push(Operation::TitleEdit {
                                track_id: track_id.clone(),
                                task_id,
                                old_title: original_title,
                                new_title: title,
                            });

                            let _ = app.save_track(&track_id);
                        }
                    }
                } else {
                    // No external changes — apply edit to in-memory state and save
                    let track = match app.find_track_mut(&track_id) {
                        Some(t) => t,
                        None => return,
                    };
                    let _ = task_ops::edit_title(track, &task_id, title.clone());

                    app.undo_stack.push(Operation::TitleEdit {
                        track_id: track_id.clone(),
                        task_id,
                        old_title: original_title,
                        new_title: title,
                    });

                    let _ = app.save_track(&track_id);
                }
            }
            drain_pending_for_track(app, &track_id);
        }
        EditTarget::ExistingTags {
            task_id,
            track_id,
            original_tags,
        } => {
            let new_value = app.edit_buffer.clone();
            let new_tags: Vec<String> = dedup_preserve_order(
                new_value
                    .split_whitespace()
                    .map(|s| s.strip_prefix('#').unwrap_or(s).to_string())
                    .filter(|s| !s.is_empty()),
            );

            let track = match app.find_track_mut(&track_id) {
                Some(t) => t,
                None => return,
            };
            if let Some(task) = task_ops::find_task_mut_in_track(track, &task_id) {
                task.tags = new_tags;
                task.mark_dirty();
            }
            let _ = app.save_track(&track_id);

            if new_value != original_tags {
                app.undo_stack.push(Operation::FieldEdit {
                    track_id: track_id.clone(),
                    task_id,
                    field: "tags".to_string(),
                    old_value: original_tags,
                    new_value,
                });
            }
            drain_pending_for_track(app, &track_id);
        }
        EditTarget::NewInboxItem { index } => {
            let title = app.edit_buffer.clone();
            if title.trim().is_empty() {
                // Empty title: discard the placeholder
                if let Some(inbox) = &mut app.project.inbox {
                    if index < inbox.items.len() {
                        inbox.items.remove(index);
                    }
                }
            } else {
                // Apply title to the inbox item
                if let Some(inbox) = &mut app.project.inbox {
                    if let Some(item) = inbox.items.get_mut(index) {
                        item.title = title;
                        item.dirty = true;
                    }
                    app.undo_stack.push(Operation::InboxAdd { index });
                }
                let _ = app.save_inbox();
            }
        }
        EditTarget::ExistingInboxTitle {
            index,
            original_title,
        } => {
            let new_title = app.edit_buffer.clone();
            if !new_title.trim().is_empty() && new_title != original_title {
                if let Some(inbox) = &mut app.project.inbox {
                    if let Some(item) = inbox.items.get_mut(index) {
                        item.title = new_title.clone();
                        item.dirty = true;
                    }
                }
                app.undo_stack.push(Operation::InboxTitleEdit {
                    index,
                    old_title: original_title,
                    new_title,
                });
                let _ = app.save_inbox();
            }
        }
        EditTarget::ExistingInboxTags {
            index,
            original_tags,
        } => {
            let new_value = app.edit_buffer.clone();
            let new_tags: Vec<String> = dedup_preserve_order(
                new_value
                    .split_whitespace()
                    .map(|s| s.strip_prefix('#').unwrap_or(s).to_string())
                    .filter(|s| !s.is_empty()),
            );
            let old_tags_vec: Vec<String> = original_tags
                .split_whitespace()
                .map(|s| s.strip_prefix('#').unwrap_or(s).to_string())
                .filter(|s| !s.is_empty())
                .collect();

            if let Some(inbox) = &mut app.project.inbox {
                if let Some(item) = inbox.items.get_mut(index) {
                    item.tags = new_tags.clone();
                    item.dirty = true;
                }
            }

            if new_tags != old_tags_vec {
                app.undo_stack.push(Operation::InboxTagsEdit {
                    index,
                    old_tags: old_tags_vec,
                    new_tags,
                });
            }
            let _ = app.save_inbox();
        }
    }
}

fn cancel_edit(app: &mut App) {
    let target = app.edit_target.take();
    let saved_cursor = app.pre_edit_cursor.take();
    app.mode = Mode::Navigate;
    app.autocomplete = None;

    match target {
        // If we were creating a new task, remove the placeholder
        Some(EditTarget::NewTask {
            task_id,
            track_id,
            parent_id,
        }) => {
            if app.track_changed_on_disk(&track_id) {
                // File changed externally — reload from disk (discards our in-memory placeholder
                // and picks up external changes atomically)
                if let Some(disk_track) = app.read_track_from_disk(&track_id) {
                    app.replace_track(&track_id, disk_track);
                }
                drain_pending_for_track(app, &track_id);
            } else {
                // No external changes — remove placeholder from memory and save
                let track = match app.find_track_mut(&track_id) {
                    Some(t) => t,
                    None => return,
                };
                if let Some(pid) = &parent_id {
                    if let Some(parent) = task_ops::find_task_mut_in_track(track, pid) {
                        parent.subtasks.retain(|t| t.id.as_deref() != Some(&task_id));
                        parent.mark_dirty();
                    }
                } else {
                    remove_task_from_section(track, &task_id, SectionKind::Backlog);
                }
                let _ = app.save_track(&track_id);
            }

            // Restore cursor to pre-edit position
            if let Some(cursor) = saved_cursor {
                let state = app.get_track_state(&track_id);
                state.cursor = cursor;
            }
        }
        // If we were creating a new inbox item, remove the placeholder
        Some(EditTarget::NewInboxItem { index }) => {
            if let Some(inbox) = &mut app.project.inbox {
                if index < inbox.items.len() {
                    inbox.items.remove(index);
                }
            }
            // Restore cursor
            if let Some(cursor) = saved_cursor {
                app.inbox_cursor = cursor;
            }
        }
        // For existing title/tags edit, cancel means revert (unchanged since we didn't write)
        _ => {}
    }
}

/// Move the cursor to a specific task by ID in a track view.
fn move_cursor_to_task(app: &mut App, track_id: &str, target_task_id: &str) {
    let flat_items = app.build_flat_items(track_id);
    let track = App::find_track_in_project(&app.project, track_id);
    if let Some(track) = track {
        for (i, item) in flat_items.iter().enumerate() {
            if let FlatItem::Task { section, path, .. } = item {
                if let Some(task) = resolve_task_from_flat(track, *section, path) {
                    if task.id.as_deref() == Some(target_task_id) {
                        let state = app.get_track_state(track_id);
                        state.cursor = i;
                        return;
                    }
                }
            }
        }
    }
}

/// Remove a task by ID from a specific section (hard remove, not mark-done).
fn remove_task_from_section(
    track: &mut crate::model::Track,
    task_id: &str,
    section: SectionKind,
) {
    if let Some(tasks) = track.section_tasks_mut(section) {
        tasks.retain(|t| t.id.as_deref() != Some(task_id));
    }
}

/// Find the byte offset of the previous word boundary
/// Deduplicate items while preserving first-occurrence order.
fn dedup_preserve_order(iter: impl Iterator<Item = String>) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    iter.filter(|s| seen.insert(s.clone())).collect()
}

fn word_boundary_left(s: &str, pos: usize) -> usize {
    if pos == 0 {
        return 0;
    }
    let bytes = s.as_bytes();
    let mut i = pos - 1;
    // Skip spaces
    while i > 0 && bytes[i] == b' ' {
        i -= 1;
    }
    // Skip word chars
    while i > 0 && bytes[i - 1] != b' ' {
        i -= 1;
    }
    i
}

/// Find the byte offset of the next word boundary
fn word_boundary_right(s: &str, pos: usize) -> usize {
    let len = s.len();
    if pos >= len {
        return len;
    }
    let bytes = s.as_bytes();
    let mut i = pos;
    // Skip current word
    while i < len && bytes[i] != b' ' {
        i += 1;
    }
    // Skip spaces
    while i < len && bytes[i] == b' ' {
        i += 1;
    }
    i
}

// ---------------------------------------------------------------------------
// MOVE mode
// ---------------------------------------------------------------------------

/// Enter MOVE mode for the task under the cursor (track view only).
fn enter_move_mode(app: &mut App) {
    match &app.view {
        View::Track(_) => {
            if let Some((track_id, task_id, section)) = app.cursor_task_id() {
                // Only allow moving top-level backlog tasks
                if section != SectionKind::Backlog {
                    return;
                }
                let track = match App::find_track_in_project(&app.project, &track_id) {
                    Some(t) => t,
                    None => return,
                };
                let backlog = track.backlog();
                let original_index = match backlog
                    .iter()
                    .position(|t| t.id.as_deref() == Some(&task_id))
                {
                    Some(i) => i,
                    None => return,
                };

                app.move_state = Some(MoveState::Task {
                    track_id,
                    task_id,
                    original_index,
                });
                app.mode = Mode::Move;
            }
        }
        View::Tracks => {
            // Find which active track the cursor is on
            let active_tracks: Vec<&str> = app
                .project
                .config
                .tracks
                .iter()
                .filter(|t| t.state == "active")
                .map(|t| t.id.as_str())
                .collect();
            let cursor = app.tracks_cursor;
            if cursor < active_tracks.len() {
                let track_id = active_tracks[cursor].to_string();
                app.move_state = Some(MoveState::Track {
                    track_id,
                    original_index: cursor,
                });
                app.mode = Mode::Move;
            }
        }
        _ => {}
    }
}

fn handle_move(app: &mut App, key: KeyEvent) {
    let is_track_move = matches!(&app.move_state, Some(MoveState::Track { .. }));
    let is_inbox_move = matches!(&app.move_state, Some(MoveState::InboxItem { .. }));

    match (key.modifiers, key.code) {
        // Confirm
        (_, KeyCode::Enter) => {
            if let Some(ms) = app.move_state.take() {
                match ms {
                    MoveState::Task {
                        track_id,
                        task_id,
                        original_index,
                    } => {
                        let new_index =
                            App::find_track_in_project(&app.project, &track_id)
                                .and_then(|t| {
                                    t.backlog()
                                        .iter()
                                        .position(|t| t.id.as_deref() == Some(&task_id))
                                })
                                .unwrap_or(0);
                        if new_index != original_index {
                            app.undo_stack.push(Operation::TaskMove {
                                track_id,
                                task_id,
                                old_index: original_index,
                                new_index,
                            });
                        }
                    }
                    MoveState::InboxItem { original_index } => {
                        let new_index = app.inbox_cursor;
                        if new_index != original_index {
                            app.undo_stack.push(Operation::InboxMove {
                                old_index: original_index,
                                new_index,
                            });
                        }
                    }
                    MoveState::Track { track_id, original_index } => {
                        // Persist track order to project.toml
                        save_track_order(app);
                        // Update active_track_ids to reflect new order
                        app.active_track_ids = app
                            .project
                            .config
                            .tracks
                            .iter()
                            .filter(|t| t.state == "active")
                            .map(|t| t.id.clone())
                            .collect();
                        // Push undo if position changed
                        let new_index = app
                            .project
                            .config
                            .tracks
                            .iter()
                            .filter(|t| t.state == "active")
                            .position(|t| t.id == track_id)
                            .unwrap_or(0);
                        if new_index != original_index {
                            app.undo_stack.push(Operation::TrackMove {
                                track_id,
                                old_index: original_index,
                                new_index,
                            });
                        }
                    }
                }
            }
            app.mode = Mode::Navigate;
        }
        // Cancel: restore original position
        (_, KeyCode::Esc) => {
            if let Some(ms) = app.move_state.take() {
                match ms {
                    MoveState::Task {
                        track_id,
                        task_id,
                        original_index,
                    } => {
                        if app.track_changed_on_disk(&track_id) {
                            // File changed externally — reload from disk instead of
                            // restoring stale in-memory state
                            if let Some(disk_track) = app.read_track_from_disk(&track_id) {
                                app.replace_track(&track_id, disk_track);
                            }
                            drain_pending_for_track(app, &track_id);
                        } else {
                            let track = match app.find_track_mut(&track_id) {
                                Some(t) => t,
                                None => {
                                    app.mode = Mode::Navigate;
                                    return;
                                }
                            };
                            let backlog =
                                match track.section_tasks_mut(SectionKind::Backlog) {
                                    Some(t) => t,
                                    None => {
                                        app.mode = Mode::Navigate;
                                        return;
                                    }
                                };
                            if let Some(cur_idx) = backlog
                                .iter()
                                .position(|t| t.id.as_deref() == Some(&task_id))
                            {
                                let task = backlog.remove(cur_idx);
                                let restore_idx = original_index.min(backlog.len());
                                backlog.insert(restore_idx, task);
                            }
                            let _ = app.save_track(&track_id);
                        }
                    }
                    MoveState::InboxItem { original_index } => {
                        // Restore original position
                        if let Some(inbox) = &mut app.project.inbox {
                            let cur = app.inbox_cursor;
                            if cur < inbox.items.len() {
                                let item = inbox.items.remove(cur);
                                let restore = original_index.min(inbox.items.len());
                                inbox.items.insert(restore, item);
                            }
                        }
                        let _ = app.save_inbox();
                        app.inbox_cursor = original_index;
                    }
                    MoveState::Track {
                        track_id,
                        original_index,
                    } => {
                        // Restore original track order
                        let _ = crate::ops::track_ops::reorder_tracks(
                            &mut app.project.config,
                            &track_id,
                            original_index,
                        );
                        app.active_track_ids = app
                            .project
                            .config
                            .tracks
                            .iter()
                            .filter(|t| t.state == "active")
                            .map(|t| t.id.clone())
                            .collect();
                        app.tracks_cursor = original_index;
                    }
                }
            }
            app.mode = Mode::Navigate;
        }
        // Move up
        (KeyModifiers::NONE, KeyCode::Up | KeyCode::Char('k')) => {
            if is_inbox_move {
                move_inbox_item(app, -1);
            } else if is_track_move {
                move_track_in_list(app, -1);
            } else {
                move_task_in_list(app, -1);
            }
        }
        // Move down
        (KeyModifiers::NONE, KeyCode::Down | KeyCode::Char('j')) => {
            if is_inbox_move {
                move_inbox_item(app, 1);
            } else if is_track_move {
                move_track_in_list(app, 1);
            } else {
                move_task_in_list(app, 1);
            }
        }
        // Move to top
        (KeyModifiers::NONE, KeyCode::Char('g')) => {
            if is_inbox_move {
                move_inbox_to_boundary(app, true);
            } else if is_track_move {
                move_track_to_boundary(app, true);
            } else {
                move_task_to_boundary(app, true);
            }
        }
        (m, KeyCode::Up) if m.contains(KeyModifiers::SUPER) => {
            if is_inbox_move {
                move_inbox_to_boundary(app, true);
            } else if is_track_move {
                move_track_to_boundary(app, true);
            } else {
                move_task_to_boundary(app, true);
            }
        }
        (_, KeyCode::Home) => {
            if is_inbox_move {
                move_inbox_to_boundary(app, true);
            } else if is_track_move {
                move_track_to_boundary(app, true);
            } else {
                move_task_to_boundary(app, true);
            }
        }
        // Move to bottom
        (KeyModifiers::SHIFT, KeyCode::Char('G')) => {
            if is_inbox_move {
                move_inbox_to_boundary(app, false);
            } else if is_track_move {
                move_track_to_boundary(app, false);
            } else {
                move_task_to_boundary(app, false);
            }
        }
        (m, KeyCode::Down) if m.contains(KeyModifiers::SUPER) => {
            if is_inbox_move {
                move_inbox_to_boundary(app, false);
            } else if is_track_move {
                move_track_to_boundary(app, false);
            } else {
                move_task_to_boundary(app, false);
            }
        }
        (_, KeyCode::End) => {
            if is_inbox_move {
                move_inbox_to_boundary(app, false);
            } else if is_track_move {
                move_track_to_boundary(app, false);
            } else {
                move_task_to_boundary(app, false);
            }
        }
        _ => {}
    }
}

/// Move the task one position up or down in the backlog.
fn move_task_in_list(app: &mut App, direction: i32) {
    let (track_id, task_id) = match &app.move_state {
        Some(MoveState::Task {
            track_id, task_id, ..
        }) => (track_id.clone(), task_id.clone()),
        _ => return,
    };

    // Check for external changes via mtime
    if app.track_changed_on_disk(&track_id)
        && let Some(disk_track) = app.read_track_from_disk(&track_id)
    {
        if task_ops::find_task_in_track(&disk_track, &task_id).is_none() {
            // Task deleted externally — abort move mode, show conflict
            app.conflict_text = Some(format!("Task {} was deleted externally", task_id));
            app.mode = Mode::Navigate;
            app.move_state = None;
            app.replace_track(&track_id, disk_track);
            drain_pending_for_track(app, &track_id);
            return;
        }
        // Replace in-memory with disk version, then continue with move
        app.replace_track(&track_id, disk_track);
        drain_pending_for_track(app, &track_id);
    }

    let track = match app.find_track_mut(&track_id) {
        Some(t) => t,
        None => return,
    };

    let backlog = match track.section_tasks_mut(SectionKind::Backlog) {
        Some(t) => t,
        None => return,
    };

    let cur_idx = match backlog
        .iter()
        .position(|t| t.id.as_deref() == Some(&task_id))
    {
        Some(i) => i,
        None => return,
    };

    let new_idx = (cur_idx as i32 + direction).clamp(0, backlog.len() as i32 - 1) as usize;
    if new_idx != cur_idx {
        let task = backlog.remove(cur_idx);
        backlog.insert(new_idx, task);
        let _ = app.save_track(&track_id);
        move_cursor_to_task(app, &track_id, &task_id);
    }
}

/// Move task to top or bottom of the backlog.
fn move_task_to_boundary(app: &mut App, to_top: bool) {
    let (track_id, task_id) = match &app.move_state {
        Some(MoveState::Task {
            track_id, task_id, ..
        }) => (track_id.clone(), task_id.clone()),
        _ => return,
    };

    // Check for external changes via mtime
    if app.track_changed_on_disk(&track_id)
        && let Some(disk_track) = app.read_track_from_disk(&track_id)
    {
        if task_ops::find_task_in_track(&disk_track, &task_id).is_none() {
            app.conflict_text = Some(format!("Task {} was deleted externally", task_id));
            app.mode = Mode::Navigate;
            app.move_state = None;
            app.replace_track(&track_id, disk_track);
            drain_pending_for_track(app, &track_id);
            return;
        }
        app.replace_track(&track_id, disk_track);
        drain_pending_for_track(app, &track_id);
    }

    let pos = if to_top {
        InsertPosition::Top
    } else {
        InsertPosition::Bottom
    };

    let track = match app.find_track_mut(&track_id) {
        Some(t) => t,
        None => return,
    };

    let _ = task_ops::move_task(track, &task_id, pos);
    let _ = app.save_track(&track_id);
    move_cursor_to_task(app, &track_id, &task_id);
}

/// Move an inbox item up or down.
fn move_inbox_item(app: &mut App, direction: i32) {
    let inbox = match &mut app.project.inbox {
        Some(inbox) => inbox,
        None => return,
    };

    let cur = app.inbox_cursor;
    let len = inbox.items.len();
    if len == 0 || cur >= len {
        return;
    }

    let new_idx = (cur as i32 + direction).clamp(0, len as i32 - 1) as usize;
    if new_idx != cur {
        inbox.items.swap(cur, new_idx);
        app.inbox_cursor = new_idx;
        let _ = app.save_inbox();
    }
}

/// Move an inbox item to the top or bottom.
fn move_inbox_to_boundary(app: &mut App, to_top: bool) {
    let inbox = match &mut app.project.inbox {
        Some(inbox) => inbox,
        None => return,
    };

    let cur = app.inbox_cursor;
    let len = inbox.items.len();
    if len == 0 || cur >= len {
        return;
    }

    let item = inbox.items.remove(cur);
    if to_top {
        inbox.items.insert(0, item);
        app.inbox_cursor = 0;
    } else {
        inbox.items.push(item);
        app.inbox_cursor = inbox.items.len() - 1;
    }
    let _ = app.save_inbox();
}

/// Move an active track up or down in the tracks list.
fn move_track_in_list(app: &mut App, direction: i32) {
    let (track_id, _) = match &app.move_state {
        Some(MoveState::Track { track_id, original_index }) => {
            (track_id.clone(), *original_index)
        }
        _ => return,
    };

    let active_tracks: Vec<String> = app
        .project
        .config
        .tracks
        .iter()
        .filter(|t| t.state == "active")
        .map(|t| t.id.clone())
        .collect();

    let cur_pos = match active_tracks.iter().position(|id| id == &track_id) {
        Some(i) => i,
        None => return,
    };

    let new_pos =
        (cur_pos as i32 + direction).clamp(0, active_tracks.len() as i32 - 1) as usize;
    if new_pos != cur_pos {
        let _ = crate::ops::track_ops::reorder_tracks(
            &mut app.project.config,
            &track_id,
            new_pos,
        );
        app.tracks_cursor = new_pos;
    }
}

/// Move track to top or bottom of active tracks.
fn move_track_to_boundary(app: &mut App, to_top: bool) {
    let (track_id, _) = match &app.move_state {
        Some(MoveState::Track { track_id, original_index }) => {
            (track_id.clone(), *original_index)
        }
        _ => return,
    };

    let active_count = app
        .project
        .config
        .tracks
        .iter()
        .filter(|t| t.state == "active")
        .count();

    let new_pos = if to_top { 0 } else { active_count - 1 };
    let _ = crate::ops::track_ops::reorder_tracks(
        &mut app.project.config,
        &track_id,
        new_pos,
    );
    app.tracks_cursor = new_pos;
}

/// Save the current track order to project.toml.
fn save_track_order(app: &mut App) {
    save_config(app);
}

// ---------------------------------------------------------------------------
// Cursor movement
// ---------------------------------------------------------------------------

/// Move cursor by delta in the current view
fn move_cursor(app: &mut App, delta: i32) {
    match &app.view {
        View::Track(idx) => {
            let track_id = match app.active_track_ids.get(*idx) {
                Some(id) => id.clone(),
                None => return,
            };
            let flat_items = app.build_flat_items(&track_id);
            let item_count = flat_items.len();
            if item_count == 0 {
                return;
            }

            let state = app.get_track_state(&track_id);
            let mut new_cursor = state.cursor as i32 + delta;
            new_cursor = new_cursor.clamp(0, item_count as i32 - 1);

            // Skip ParkedSeparator
            let new_cursor = new_cursor as usize;
            let new_cursor = skip_separator(&flat_items, new_cursor, delta);

            state.cursor = new_cursor;
        }
        View::Detail { .. } => {
            detail_move_region(app, delta);
        }
        View::Tracks => {
            let total = count_tracks(app);
            if total == 0 {
                return;
            }
            let mut new_cursor = app.tracks_cursor as i32 + delta;
            new_cursor = new_cursor.clamp(0, total as i32 - 1);
            app.tracks_cursor = new_cursor as usize;
        }
        View::Inbox => {
            let count = app.inbox_count();
            if count == 0 {
                return;
            }
            let mut new_cursor = app.inbox_cursor as i32 + delta;
            new_cursor = new_cursor.clamp(0, count as i32 - 1);
            app.inbox_cursor = new_cursor as usize;
        }
        View::Recent => {
            let count = count_recent_tasks(app);
            if count == 0 {
                return;
            }
            let mut new_cursor = app.recent_cursor as i32 + delta;
            new_cursor = new_cursor.clamp(0, count as i32 - 1);
            app.recent_cursor = new_cursor as usize;
        }
    }
}

/// Skip over ParkedSeparator items when navigating
fn skip_separator(items: &[FlatItem], cursor: usize, direction: i32) -> usize {
    if cursor >= items.len() {
        return cursor;
    }
    if matches!(items[cursor], FlatItem::ParkedSeparator) {
        let next = (cursor as i32 + direction).clamp(0, items.len() as i32 - 1) as usize;
        if next != cursor && !matches!(items[next], FlatItem::ParkedSeparator) {
            return next;
        }
        // If stuck, try the other direction
        let prev = (cursor as i32 - direction).clamp(0, items.len() as i32 - 1) as usize;
        if !matches!(items[prev], FlatItem::ParkedSeparator) {
            return prev;
        }
    }
    cursor
}

fn jump_to_top(app: &mut App) {
    match &app.view {
        View::Track(idx) => {
            let track_id = match app.active_track_ids.get(*idx) {
                Some(id) => id.clone(),
                None => return,
            };
            let state = app.get_track_state(&track_id);
            state.cursor = 0;
            state.scroll_offset = 0;
        }
        View::Detail { .. } => {
            if let Some(ds) = &mut app.detail_state {
                ds.region = ds.regions.first().copied().unwrap_or(DetailRegion::Title);
                ds.scroll_offset = 0;
            }
        }
        View::Tracks => {
            app.tracks_cursor = 0;
        }
        View::Inbox => {
            app.inbox_cursor = 0;
            app.inbox_scroll = 0;
        }
        View::Recent => {
            app.recent_cursor = 0;
            app.recent_scroll = 0;
        }
    }
}

fn jump_to_bottom(app: &mut App) {
    match &app.view {
        View::Track(idx) => {
            let track_id = match app.active_track_ids.get(*idx) {
                Some(id) => id.clone(),
                None => return,
            };
            let flat_items = app.build_flat_items(&track_id);
            let count = flat_items.len();
            if count == 0 {
                return;
            }
            let mut target = count - 1;
            // Skip separator at end
            target = skip_separator(&flat_items, target, -1);
            let state = app.get_track_state(&track_id);
            state.cursor = target;
        }
        View::Detail { .. } => {
            if let Some(ds) = &mut app.detail_state {
                ds.region = ds.regions.last().copied().unwrap_or(DetailRegion::Title);
                if ds.region == DetailRegion::Subtasks {
                    ds.subtask_cursor = 0;
                }
            }
        }
        View::Tracks => {
            let total = count_tracks(app);
            if total > 0 {
                app.tracks_cursor = total - 1;
            }
        }
        View::Inbox => {
            let count = app.inbox_count();
            if count > 0 {
                app.inbox_cursor = count - 1;
            }
        }
        View::Recent => {
            let count = count_recent_tasks(app);
            if count > 0 {
                app.recent_cursor = count - 1;
            }
        }
    }
}

/// Expand current node or move to first child (track view)
fn expand_or_enter(app: &mut App) {
    if let View::Track(idx) = &app.view {
        let track_id = match app.active_track_ids.get(*idx) {
            Some(id) => id.clone(),
            None => return,
        };
        let flat_items = app.build_flat_items(&track_id);
        let cursor = app.track_states.get(&track_id).map_or(0, |s| s.cursor);

        if cursor >= flat_items.len() {
            return;
        }

        if let FlatItem::Task {
            has_children,
            is_expanded,
            section,
            path,
            ..
        } = &flat_items[cursor]
        {
            if *has_children && !is_expanded {
                // Expand this node
                let track = match app.current_track() {
                    Some(t) => t,
                    None => return,
                };
                if let Some(task) = resolve_task_from_track(track, *section, path) {
                    let key = crate::tui::app::task_expand_key(task, *section, path);
                    let state = app.get_track_state(&track_id);
                    state.expanded.insert(key);
                }
            } else if *has_children && *is_expanded && cursor + 1 < flat_items.len() {
                // Already expanded: move to first child
                let state = app.get_track_state(&track_id);
                state.cursor = cursor + 1;
            }
        }
    }
}

/// Collapse current node or move to parent (track view)
fn collapse_or_parent(app: &mut App) {
    if let View::Track(idx) = &app.view {
        let track_id = match app.active_track_ids.get(*idx) {
            Some(id) => id.clone(),
            None => return,
        };
        let flat_items = app.build_flat_items(&track_id);
        let cursor = app.track_states.get(&track_id).map_or(0, |s| s.cursor);

        if cursor >= flat_items.len() {
            return;
        }

        if let FlatItem::Task {
            has_children: _,
            is_expanded,
            section,
            path,
            depth,
            ..
        } = &flat_items[cursor]
        {
            if *is_expanded {
                // Collapse this node
                let track = match app.current_track() {
                    Some(t) => t,
                    None => return,
                };
                if let Some(task) = resolve_task_from_track(track, *section, path) {
                    let key = crate::tui::app::task_expand_key(task, *section, path);
                    let state = app.get_track_state(&track_id);
                    state.expanded.remove(&key);
                }
            } else if *depth > 0 {
                // Move to parent: find the previous item at depth - 1
                let parent_depth = depth - 1;
                for i in (0..cursor).rev() {
                    if let FlatItem::Task { depth: d, .. } = &flat_items[i]
                        && *d == parent_depth
                    {
                        app.get_track_state(&track_id).cursor = i;
                        break;
                    }
                }
            }
        }
    }
}

fn resolve_task_from_track<'a>(
    track: &'a crate::model::Track,
    section: crate::model::SectionKind,
    path: &[usize],
) -> Option<&'a crate::model::Task> {
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

/// Count total tracks (for cursor navigation in tracks view)
fn count_tracks(app: &App) -> usize {
    app.project.config.tracks.len()
}

/// Count total done tasks across all tracks
fn count_recent_tasks(app: &App) -> usize {
    app.project
        .tracks
        .iter()
        .map(|(_, track)| track.section_tasks(crate::model::SectionKind::Done).len())
        .sum()
}

// ---------------------------------------------------------------------------
// Detail view functions
// ---------------------------------------------------------------------------

/// Handle Enter key: open detail view from track view, or enter edit / open subtask in detail view
fn handle_enter(app: &mut App) {
    match &app.view {
        View::Track(_) => {
            // Open detail view for the task under cursor
            if let Some((track_id, task_id, _)) = app.cursor_task_id() {
                app.open_detail(track_id, task_id);
            }
        }
        View::Detail { track_id, .. } => {
            let track_id = track_id.clone();
            let on_subtask = app.detail_state.as_ref().is_some_and(|ds| {
                ds.region == DetailRegion::Subtasks && !ds.flat_subtask_ids.is_empty()
            });
            if on_subtask {
                let subtask_id = app
                    .detail_state
                    .as_ref()
                    .and_then(|ds| ds.flat_subtask_ids.get(ds.subtask_cursor).cloned());
                if let Some(sub_id) = subtask_id {
                    app.open_detail(track_id, sub_id);
                }
            } else {
                detail_enter_edit(app);
            }
        }
        _ => {}
    }
}

/// Move between regions in the detail view (up/down)
fn detail_move_region(app: &mut App, delta: i32) {
    let ds = match &mut app.detail_state {
        Some(ds) => ds,
        None => return,
    };

    if ds.regions.is_empty() {
        return;
    }

    let current_idx = ds.regions.iter().position(|r| *r == ds.region).unwrap_or(0);

    // Special handling when on Subtasks region with subtasks
    if ds.region == DetailRegion::Subtasks && !ds.flat_subtask_ids.is_empty() {
        if delta > 0 {
            // Moving down within subtasks
            if ds.subtask_cursor + 1 < ds.flat_subtask_ids.len() {
                ds.subtask_cursor += 1;
                return;
            }
            // At last subtask, stay put
            return;
        } else {
            // Moving up within subtasks
            if ds.subtask_cursor > 0 {
                ds.subtask_cursor -= 1;
                return;
            }
            // At first subtask, move to previous region
            let new_idx = current_idx.saturating_sub(1);
            ds.region = ds.regions[new_idx];
            return;
        }
    }

    let new_idx = (current_idx as i32 + delta).clamp(0, ds.regions.len() as i32 - 1) as usize;
    ds.region = ds.regions[new_idx];

    // When entering Subtasks from another region via Down, reset subtask_cursor
    if ds.region == DetailRegion::Subtasks && delta > 0 {
        ds.subtask_cursor = 0;
    }
}

/// Jump to next/prev region in the detail view (Tab/Shift+Tab)
fn detail_jump_editable(app: &mut App, direction: i32) {
    let ds = match &mut app.detail_state {
        Some(ds) => ds,
        None => return,
    };

    if ds.regions.is_empty() {
        return;
    }

    let current_idx = ds.regions.iter().position(|r| *r == ds.region).unwrap_or(0);
    let len = ds.regions.len();

    if direction > 0 {
        let new_idx = (current_idx + 1) % len;
        ds.region = ds.regions[new_idx];
    } else {
        let new_idx = if current_idx == 0 { len - 1 } else { current_idx - 1 };
        ds.region = ds.regions[new_idx];
    }

    // Reset subtask_cursor when landing on Subtasks
    if ds.region == DetailRegion::Subtasks {
        ds.subtask_cursor = 0;
    }
}

/// Enter EDIT mode on the current region in the detail view
fn detail_enter_edit(app: &mut App) {
    let (track_id, task_id) = match &app.view {
        View::Detail { track_id, task_id } => (track_id.clone(), task_id.clone()),
        _ => return,
    };

    let region = match &app.detail_state {
        Some(ds) => ds.region,
        None => return,
    };

    // Don't allow editing non-editable regions
    if !region.is_editable() {
        return;
    }

    // Get current value for the region
    let track = match App::find_track_in_project(&app.project, &track_id) {
        Some(t) => t,
        None => return,
    };
    let task = match task_ops::find_task_in_track(track, &task_id) {
        Some(t) => t,
        None => return,
    };

    let (initial_value, is_multiline) = match region {
        DetailRegion::Title => (task.title.clone(), false),
        DetailRegion::Tags => {
            let tag_str = task.tags.iter().map(|t| format!("#{}", t)).collect::<Vec<_>>().join(" ");
            (tag_str, false)
        }
        DetailRegion::Deps => {
            let deps: Vec<String> = task.metadata.iter().flat_map(|m| {
                if let Metadata::Dep(d) = m { d.clone() } else { Vec::new() }
            }).collect();
            (deps.join(", "), false)
        }
        DetailRegion::Spec => {
            let spec = task.metadata.iter().find_map(|m| {
                if let Metadata::Spec(s) = m { Some(s.clone()) } else { None }
            }).unwrap_or_default();
            (spec, false)
        }
        DetailRegion::Refs => {
            let refs: Vec<String> = task.metadata.iter().flat_map(|m| {
                if let Metadata::Ref(r) = m { r.clone() } else { Vec::new() }
            }).collect();
            (refs.join(" "), false)
        }
        DetailRegion::Note => {
            let note = task.metadata.iter().find_map(|m| {
                if let Metadata::Note(n) = m { Some(n.clone()) } else { None }
            }).unwrap_or_default();
            (note, true)
        }
        _ => return,
    };

    if is_multiline {
        // Multi-line editing (note): use detail_state's edit fields
        if let Some(ds) = &mut app.detail_state {
            ds.editing = true;
            ds.edit_buffer = initial_value.clone();
            ds.edit_cursor_line = 0;
            ds.edit_cursor_col = 0;
            ds.edit_original = initial_value.clone();
        }
        app.edit_history = Some(EditHistory::new(&initial_value, 0, 0));
        app.mode = Mode::Edit;
    } else {
        // Single-line editing: use the existing edit_buffer/edit_cursor on App
        app.edit_buffer = initial_value.clone();
        app.edit_cursor = app.edit_buffer.len();
        app.edit_target = Some(EditTarget::ExistingTitle {
            task_id: task_id.clone(),
            track_id: track_id.clone(),
            original_title: initial_value,
        });
        if let Some(ds) = &mut app.detail_state {
            ds.editing = true;
            ds.edit_original = app.edit_buffer.clone();
        }

        // Activate autocomplete for appropriate regions
        activate_autocomplete_for_region(app, region);

        app.edit_history = Some(EditHistory::new(&app.edit_buffer, app.edit_cursor, 0));
        app.mode = Mode::Edit;
    }
}

/// Jump to a specific region and enter EDIT mode (for #, @, d, n shortcuts)
fn detail_jump_to_region_and_edit(app: &mut App, target_region: DetailRegion) {
    if let Some(ds) = &mut app.detail_state {
        ds.region = target_region;
    }
    detail_enter_edit(app);
}

/// Handle multi-line editing (note field) in detail view
fn handle_detail_multiline_edit(app: &mut App, key: KeyEvent) {
    // Selection pre-pass: manage multiline_selection_anchor for movement keys
    let has_shift = key.modifiers.contains(KeyModifiers::SHIFT);
    let is_movement = matches!(key.code, KeyCode::Left | KeyCode::Right | KeyCode::Up | KeyCode::Down);
    if is_movement {
        if let Some(ds) = &mut app.detail_state {
            if has_shift {
                if ds.multiline_selection_anchor.is_none() {
                    ds.multiline_selection_anchor = Some((ds.edit_cursor_line, ds.edit_cursor_col));
                }
            } else {
                ds.multiline_selection_anchor = None;
            }
        }
    }

    match (key.modifiers, key.code) {
        // Esc: finish editing (save)
        (_, KeyCode::Esc) => {
            if let Some(ds) = &mut app.detail_state {
                ds.multiline_selection_anchor = None;
            }
            app.edit_history = None;
            confirm_detail_multiline(app);
        }
        // Select all (Ctrl+A)
        (m, KeyCode::Char('a')) if m.contains(KeyModifiers::CONTROL) => {
            if let Some(ds) = &mut app.detail_state {
                ds.multiline_selection_anchor = Some((0, 0));
                let line_count = ds.edit_buffer.split('\n').count();
                let last_len = ds.edit_buffer.split('\n').last().map_or(0, |l| l.len());
                ds.edit_cursor_line = line_count.saturating_sub(1);
                ds.edit_cursor_col = last_len;
            }
        }
        // Copy (Ctrl+C)
        (m, KeyCode::Char('c')) if m.contains(KeyModifiers::CONTROL) => {
            if let Some(ds) = &app.detail_state {
                if let Some(text) = get_multiline_selection_text(ds) {
                    clipboard_set(&text);
                }
            }
        }
        // Cut (Ctrl+X)
        (m, KeyCode::Char('x')) if m.contains(KeyModifiers::CONTROL) => {
            if let Some(ds) = &mut app.detail_state {
                if let Some(text) = delete_multiline_selection(ds) {
                    clipboard_set(&text);
                }
            }
            snapshot_multiline(app);
        }
        // Paste (Ctrl+V)
        (m, KeyCode::Char('v')) if m.contains(KeyModifiers::CONTROL) => {
            if let Some(paste_text) = clipboard_get() {
                if let Some(ds) = &mut app.detail_state {
                    delete_multiline_selection(ds);
                    let offset = multiline_pos_to_offset(&ds.edit_buffer, ds.edit_cursor_line, ds.edit_cursor_col);
                    ds.edit_buffer.insert_str(offset, &paste_text);
                    let new_offset = offset + paste_text.len();
                    let (new_line, new_col) = offset_to_multiline_pos(&ds.edit_buffer, new_offset);
                    ds.edit_cursor_line = new_line;
                    ds.edit_cursor_col = new_col;
                }
                snapshot_multiline(app);
            }
        }
        // Inline undo (Ctrl+Z)
        (m, KeyCode::Char('z')) if m.contains(KeyModifiers::CONTROL) => {
            if let Some(ds) = &mut app.detail_state {
                ds.multiline_selection_anchor = None;
            }
            if let Some(eh) = &mut app.edit_history {
                if let Some((buf, col, line)) = eh.undo() {
                    let buf = buf.to_string();
                    if let Some(ds) = &mut app.detail_state {
                        ds.edit_buffer = buf;
                        ds.edit_cursor_col = col;
                        ds.edit_cursor_line = line;
                    }
                }
            }
        }
        // Inline redo (Ctrl+Y or Ctrl+Shift+Z)
        (m, KeyCode::Char('y')) if m.contains(KeyModifiers::CONTROL) => {
            if let Some(ds) = &mut app.detail_state {
                ds.multiline_selection_anchor = None;
            }
            if let Some(eh) = &mut app.edit_history {
                if let Some((buf, col, line)) = eh.redo() {
                    let buf = buf.to_string();
                    if let Some(ds) = &mut app.detail_state {
                        ds.edit_buffer = buf;
                        ds.edit_cursor_col = col;
                        ds.edit_cursor_line = line;
                    }
                }
            }
        }
        (m, KeyCode::Char('Z')) if m.contains(KeyModifiers::CONTROL) && m.contains(KeyModifiers::SHIFT) => {
            if let Some(ds) = &mut app.detail_state {
                ds.multiline_selection_anchor = None;
            }
            if let Some(eh) = &mut app.edit_history {
                if let Some((buf, col, line)) = eh.redo() {
                    let buf = buf.to_string();
                    if let Some(ds) = &mut app.detail_state {
                        ds.edit_buffer = buf;
                        ds.edit_cursor_col = col;
                        ds.edit_cursor_line = line;
                    }
                }
            }
        }
        // Enter: newline (delete selection first)
        (KeyModifiers::NONE, KeyCode::Enter) => {
            if let Some(ds) = &mut app.detail_state {
                delete_multiline_selection(ds);
                let mut edit_lines: Vec<String> = ds.edit_buffer.split('\n').map(String::from).collect();
                let line = ds.edit_cursor_line.min(edit_lines.len().saturating_sub(1));
                let col = ds.edit_cursor_col.min(edit_lines[line].len());

                // Split current line at cursor
                let rest = edit_lines[line][col..].to_string();
                edit_lines[line] = edit_lines[line][..col].to_string();
                edit_lines.insert(line + 1, rest);

                ds.edit_buffer = edit_lines.join("\n");
                ds.edit_cursor_line = line + 1;
                ds.edit_cursor_col = 0;
            }
            snapshot_multiline(app);
        }
        // Tab: insert 4 spaces (delete selection first)
        (KeyModifiers::NONE, KeyCode::Tab) => {
            if let Some(ds) = &mut app.detail_state {
                delete_multiline_selection(ds);
                let mut edit_lines: Vec<String> = ds.edit_buffer.split('\n').map(String::from).collect();
                let line = ds.edit_cursor_line.min(edit_lines.len().saturating_sub(1));
                let col = ds.edit_cursor_col.min(edit_lines[line].len());
                edit_lines[line].insert_str(col, "    ");
                ds.edit_buffer = edit_lines.join("\n");
                ds.edit_cursor_col = col + 4;
            }
            snapshot_multiline(app);
        }
        // Cursor movement: Left (plain or Shift for selection)
        (_, KeyCode::Left) if !key.modifiers.contains(KeyModifiers::ALT)
            && !key.modifiers.contains(KeyModifiers::CONTROL)
            && !key.modifiers.contains(KeyModifiers::SUPER) =>
        {
            if let Some(ds) = &mut app.detail_state {
                if ds.edit_cursor_col > 0 {
                    ds.edit_cursor_col -= 1;
                } else if ds.edit_cursor_line > 0 {
                    ds.edit_cursor_line -= 1;
                    let edit_lines: Vec<&str> = ds.edit_buffer.split('\n').collect();
                    ds.edit_cursor_col = edit_lines.get(ds.edit_cursor_line).map_or(0, |l| l.len());
                }
            }
        }
        // Cursor movement: Right (plain or Shift for selection)
        (_, KeyCode::Right) if !key.modifiers.contains(KeyModifiers::ALT)
            && !key.modifiers.contains(KeyModifiers::CONTROL)
            && !key.modifiers.contains(KeyModifiers::SUPER) =>
        {
            if let Some(ds) = &mut app.detail_state {
                let edit_lines: Vec<&str> = ds.edit_buffer.split('\n').collect();
                let line_len = edit_lines.get(ds.edit_cursor_line).map_or(0, |l| l.len());
                if ds.edit_cursor_col < line_len {
                    ds.edit_cursor_col += 1;
                } else if ds.edit_cursor_line + 1 < edit_lines.len() {
                    ds.edit_cursor_line += 1;
                    ds.edit_cursor_col = 0;
                }
            }
        }
        // Cursor movement: Up (plain or Shift for selection)
        (_, KeyCode::Up) if !key.modifiers.contains(KeyModifiers::ALT)
            && !key.modifiers.contains(KeyModifiers::CONTROL)
            && !key.modifiers.contains(KeyModifiers::SUPER) =>
        {
            if let Some(ds) = &mut app.detail_state {
                if ds.edit_cursor_line > 0 {
                    ds.edit_cursor_line -= 1;
                    let edit_lines: Vec<&str> = ds.edit_buffer.split('\n').collect();
                    let line_len = edit_lines.get(ds.edit_cursor_line).map_or(0, |l| l.len());
                    ds.edit_cursor_col = ds.edit_cursor_col.min(line_len);
                }
            }
        }
        // Cursor movement: Down (plain or Shift for selection)
        (_, KeyCode::Down) if !key.modifiers.contains(KeyModifiers::ALT)
            && !key.modifiers.contains(KeyModifiers::CONTROL)
            && !key.modifiers.contains(KeyModifiers::SUPER) =>
        {
            if let Some(ds) = &mut app.detail_state {
                let edit_lines: Vec<&str> = ds.edit_buffer.split('\n').collect();
                if ds.edit_cursor_line + 1 < edit_lines.len() {
                    ds.edit_cursor_line += 1;
                    let line_len = edit_lines.get(ds.edit_cursor_line).map_or(0, |l| l.len());
                    ds.edit_cursor_col = ds.edit_cursor_col.min(line_len);
                }
            }
        }
        // Home / Cmd+Left: start of line (with or without Shift)
        (m, KeyCode::Left) if m.contains(KeyModifiers::SUPER) => {
            if let Some(ds) = &mut app.detail_state {
                ds.edit_cursor_col = 0;
            }
        }
        // End / Cmd+Right: end of line (with or without Shift)
        (m, KeyCode::Right) if m.contains(KeyModifiers::SUPER) => {
            if let Some(ds) = &mut app.detail_state {
                let edit_lines: Vec<&str> = ds.edit_buffer.split('\n').collect();
                let line_len = edit_lines.get(ds.edit_cursor_line).map_or(0, |l| l.len());
                ds.edit_cursor_col = line_len;
            }
        }
        // Word movement (Alt or Ctrl, with or without Shift)
        (m, KeyCode::Left) if m.contains(KeyModifiers::ALT) || m.contains(KeyModifiers::CONTROL) => {
            if let Some(ds) = &mut app.detail_state {
                let edit_lines: Vec<&str> = ds.edit_buffer.split('\n').collect();
                let line = &edit_lines[ds.edit_cursor_line.min(edit_lines.len().saturating_sub(1))];
                ds.edit_cursor_col = word_boundary_left(line, ds.edit_cursor_col);
            }
        }
        (m, KeyCode::Right) if m.contains(KeyModifiers::ALT) || m.contains(KeyModifiers::CONTROL) => {
            if let Some(ds) = &mut app.detail_state {
                let edit_lines: Vec<&str> = ds.edit_buffer.split('\n').collect();
                let line = &edit_lines[ds.edit_cursor_line.min(edit_lines.len().saturating_sub(1))];
                ds.edit_cursor_col = word_boundary_right(line, ds.edit_cursor_col);
            }
        }
        // Word backspace (Alt or Ctrl): delete selection or word
        (m, KeyCode::Backspace) if m.contains(KeyModifiers::ALT) || m.contains(KeyModifiers::CONTROL) => {
            if let Some(ds) = &mut app.detail_state {
                if delete_multiline_selection(ds).is_none() {
                    let mut edit_lines: Vec<String> = ds.edit_buffer.split('\n').map(String::from).collect();
                    let line_idx = ds.edit_cursor_line.min(edit_lines.len().saturating_sub(1));
                    let col = ds.edit_cursor_col.min(edit_lines[line_idx].len());
                    let new_pos = word_boundary_left(&edit_lines[line_idx], col);
                    edit_lines[line_idx].drain(new_pos..col);
                    ds.edit_cursor_col = new_pos;
                    ds.edit_buffer = edit_lines.join("\n");
                }
            }
            snapshot_multiline(app);
        }
        // Backspace: delete selection or single char
        (KeyModifiers::NONE, KeyCode::Backspace) => {
            if let Some(ds) = &mut app.detail_state {
                if delete_multiline_selection(ds).is_none() {
                    let mut edit_lines: Vec<String> = ds.edit_buffer.split('\n').map(String::from).collect();
                    let line = ds.edit_cursor_line.min(edit_lines.len().saturating_sub(1));
                    let col = ds.edit_cursor_col.min(edit_lines[line].len());

                    if col > 0 {
                        edit_lines[line].remove(col - 1);
                        ds.edit_cursor_col = col - 1;
                    } else if line > 0 {
                        // Merge with previous line
                        let current_line = edit_lines.remove(line);
                        let prev_len = edit_lines[line - 1].len();
                        edit_lines[line - 1].push_str(&current_line);
                        ds.edit_cursor_line = line - 1;
                        ds.edit_cursor_col = prev_len;
                    }
                    ds.edit_buffer = edit_lines.join("\n");
                }
            }
            snapshot_multiline(app);
        }
        // Type character: delete selection first, then insert
        (KeyModifiers::NONE | KeyModifiers::SHIFT, KeyCode::Char(c)) => {
            if let Some(ds) = &mut app.detail_state {
                delete_multiline_selection(ds);
                let mut edit_lines: Vec<String> = ds.edit_buffer.split('\n').map(String::from).collect();
                let line = ds.edit_cursor_line.min(edit_lines.len().saturating_sub(1));
                let col = ds.edit_cursor_col.min(edit_lines[line].len());
                edit_lines[line].insert(col, c);
                ds.edit_buffer = edit_lines.join("\n");
                ds.edit_cursor_col = col + 1;
            }
            snapshot_multiline(app);
        }
        _ => {}
    }
}

/// Snapshot the current multiline edit state for inline undo/redo
fn snapshot_multiline(app: &mut App) {
    if let Some(ds) = &app.detail_state {
        if let Some(eh) = &mut app.edit_history {
            eh.snapshot(&ds.edit_buffer, ds.edit_cursor_col, ds.edit_cursor_line);
        }
    }
}

/// Confirm a detail view multi-line edit (note)
fn confirm_detail_multiline(app: &mut App) {
    let (track_id, task_id) = match &app.view {
        View::Detail { track_id, task_id } => (track_id.clone(), task_id.clone()),
        _ => {
            app.mode = Mode::Navigate;
            return;
        }
    };

    let new_value = app.detail_state.as_ref().map(|ds| ds.edit_buffer.clone()).unwrap_or_default();
    let original = app.detail_state.as_ref().map(|ds| ds.edit_original.clone()).unwrap_or_default();

    // Apply the note change
    let track = match app.find_track_mut(&track_id) {
        Some(t) => t,
        None => {
            app.mode = Mode::Navigate;
            return;
        }
    };
    let _ = task_ops::set_note(track, &task_id, new_value.clone());
    let _ = app.save_track(&track_id);

    if new_value != original {
        app.undo_stack.push(Operation::FieldEdit {
            track_id,
            task_id,
            field: "note".to_string(),
            old_value: original,
            new_value,
        });
    }

    // Exit edit mode
    app.mode = Mode::Navigate;
    app.autocomplete = None;
    if let Some(ds) = &mut app.detail_state {
        ds.editing = false;
    }
}

/// Confirm a detail view single-line edit (title, tags, deps, spec, refs)
fn confirm_detail_edit(app: &mut App) {
    let (track_id, task_id) = match &app.view {
        View::Detail { track_id, task_id } => (track_id.clone(), task_id.clone()),
        _ => {
            app.mode = Mode::Navigate;
            return;
        }
    };

    let region = match &app.detail_state {
        Some(ds) => ds.region,
        None => {
            app.mode = Mode::Navigate;
            return;
        }
    };

    let new_value = app.edit_buffer.clone();
    let original = app.detail_state.as_ref().map(|ds| ds.edit_original.clone()).unwrap_or_default();

    // Apply the change based on region
    match region {
        DetailRegion::Title => {
            if !new_value.trim().is_empty() && new_value != original {
                let track = match app.find_track_mut(&track_id) {
                    Some(t) => t,
                    None => {
                        app.mode = Mode::Navigate;
                        return;
                    }
                };
                let _ = task_ops::edit_title(track, &task_id, new_value.clone());

                app.undo_stack.push(Operation::TitleEdit {
                    track_id: track_id.clone(),
                    task_id: task_id.clone(),
                    old_title: original,
                    new_title: new_value,
                });

                let _ = app.save_track(&track_id);
            }
        }
        DetailRegion::Tags => {
            // Parse tags from input: "#tag1 #tag2" or "tag1 tag2" (deduplicated)
            let new_tags: Vec<String> = dedup_preserve_order(
                new_value
                    .split_whitespace()
                    .map(|s| s.strip_prefix('#').unwrap_or(s).to_string())
                    .filter(|s| !s.is_empty()),
            );

            let track = match app.find_track_mut(&track_id) {
                Some(t) => t,
                None => {
                    app.mode = Mode::Navigate;
                    return;
                }
            };
            if let Some(task) = task_ops::find_task_mut_in_track(track, &task_id) {
                task.tags = new_tags;
                task.mark_dirty();
            }
            let _ = app.save_track(&track_id);

            if new_value != original {
                app.undo_stack.push(Operation::FieldEdit {
                    track_id: track_id.clone(),
                    task_id: task_id.clone(),
                    field: "tags".to_string(),
                    old_value: original,
                    new_value,
                });
            }
        }
        DetailRegion::Deps => {
            // Parse deps: "EFF-003, MOD-007" or "EFF-003 MOD-007" (deduplicated)
            let new_deps: Vec<String> = dedup_preserve_order(
                new_value
                    .split(|c: char| c == ',' || c.is_whitespace())
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty()),
            );

            let track = match app.find_track_mut(&track_id) {
                Some(t) => t,
                None => {
                    app.mode = Mode::Navigate;
                    return;
                }
            };
            if let Some(task) = task_ops::find_task_mut_in_track(track, &task_id) {
                // Remove existing deps and add new ones
                task.metadata.retain(|m| !matches!(m, Metadata::Dep(_)));
                if !new_deps.is_empty() {
                    task.metadata.push(Metadata::Dep(new_deps));
                }
                task.mark_dirty();
            }
            let _ = app.save_track(&track_id);

            if new_value != original {
                app.undo_stack.push(Operation::FieldEdit {
                    track_id: track_id.clone(),
                    task_id: task_id.clone(),
                    field: "deps".to_string(),
                    old_value: original,
                    new_value,
                });
            }
        }
        DetailRegion::Spec => {
            let track = match app.find_track_mut(&track_id) {
                Some(t) => t,
                None => {
                    app.mode = Mode::Navigate;
                    return;
                }
            };
            if !new_value.trim().is_empty() {
                let _ = task_ops::set_spec(track, &task_id, new_value.trim().to_string());
            } else {
                // Remove spec
                if let Some(task) = task_ops::find_task_mut_in_track(track, &task_id) {
                    task.metadata.retain(|m| !matches!(m, Metadata::Spec(_)));
                    task.mark_dirty();
                }
            }
            let _ = app.save_track(&track_id);

            if new_value != original {
                app.undo_stack.push(Operation::FieldEdit {
                    track_id: track_id.clone(),
                    task_id: task_id.clone(),
                    field: "spec".to_string(),
                    old_value: original,
                    new_value,
                });
            }
        }
        DetailRegion::Refs => {
            // Parse refs: space or comma separated paths (deduplicated)
            let new_refs: Vec<String> = dedup_preserve_order(
                new_value
                    .split(|c: char| c == ',' || c.is_whitespace())
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty()),
            );

            let track = match app.find_track_mut(&track_id) {
                Some(t) => t,
                None => {
                    app.mode = Mode::Navigate;
                    return;
                }
            };
            if let Some(task) = task_ops::find_task_mut_in_track(track, &task_id) {
                task.metadata.retain(|m| !matches!(m, Metadata::Ref(_)));
                if !new_refs.is_empty() {
                    task.metadata.push(Metadata::Ref(new_refs));
                }
                task.mark_dirty();
            }
            let _ = app.save_track(&track_id);

            if new_value != original {
                app.undo_stack.push(Operation::FieldEdit {
                    track_id: track_id.clone(),
                    task_id: task_id.clone(),
                    field: "refs".to_string(),
                    old_value: original,
                    new_value,
                });
            }
        }
        _ => {}
    }

    // Exit edit mode
    app.mode = Mode::Navigate;
    app.edit_target = None;
    app.edit_buffer.clear();
    app.edit_cursor = 0;
    app.autocomplete = None;
    if let Some(ds) = &mut app.detail_state {
        ds.editing = false;
    }
}

/// Cancel a detail view edit
fn cancel_detail_edit(app: &mut App) {
    app.mode = Mode::Navigate;
    app.edit_target = None;
    app.edit_buffer.clear();
    app.edit_cursor = 0;
    app.autocomplete = None;
    if let Some(ds) = &mut app.detail_state {
        ds.editing = false;
        ds.edit_buffer.clear();
    }
}

/// Advance search by `direction` (+1 = next, -1 = prev) with proper wrapping.
fn search_next(app: &mut App, direction: i32) {
    app.search_wrap_message = None;
    execute_search_dir(app, direction);
}

/// Execute search: find matches in the current view and jump to the match.
/// `direction` is +1 (next) or -1 (prev) or 0 (first from cursor).
/// Matches are found relative to the current cursor position, not a stored match index.
/// Uses regex via ops::search for full-field matching. Auto-expands collapsed subtrees
/// in track view to reveal matching tasks.
fn execute_search_dir(app: &mut App, direction: i32) {
    let pattern = match &app.last_search {
        Some(p) => p.clone(),
        None => return,
    };
    // Build case-insensitive regex; fall back to escaped literal on invalid regex
    let re = match Regex::new(&format!("(?i){}", pattern)) {
        Ok(r) => r,
        Err(_) => match Regex::new(&format!("(?i){}", regex::escape(&pattern))) {
            Ok(r) => r,
            Err(_) => return,
        },
    };

    app.search_match_count = Some(count_matches_for_pattern(app, &re));

    match app.view.clone() {
        View::Track(idx) => search_in_track(app, idx, &re, direction),
        View::Detail { .. } => {} // Search not supported in detail view
        View::Tracks => search_in_tracks_view(app, &re, direction),
        View::Inbox => search_in_inbox(app, &re, direction),
        View::Recent => search_in_recent(app, &re, direction),
    }
}

/// Given a sorted list of cursor positions where matches occur,
/// find the next one relative to `current_cursor` in the given direction.
/// Returns (index into positions, wrapped: bool) or None if empty.
/// direction: 0 = at or after cursor, +1 = strictly after, -1 = strictly before.
fn find_next_match_position(
    positions: &[usize],
    current_cursor: usize,
    direction: i32,
) -> Option<(usize, bool)> {
    if positions.is_empty() {
        return None;
    }
    match direction {
        0 => {
            // Initial search: find first match at or after cursor, fallback to first
            if let Some(idx) = positions.iter().position(|&p| p >= current_cursor) {
                Some((idx, false))
            } else {
                Some((0, false))
            }
        }
        1 => {
            // Next: find first match strictly after cursor
            if let Some(idx) = positions.iter().position(|&p| p > current_cursor) {
                Some((idx, false))
            } else {
                Some((0, true)) // wrap to top
            }
        }
        -1 => {
            // Prev: find last match strictly before cursor
            if let Some(idx) = positions.iter().rposition(|&p| p < current_cursor) {
                Some((idx, false))
            } else {
                Some((positions.len() - 1, true)) // wrap to bottom
            }
        }
        _ => None,
    }
}

/// Search within a single track view. Uses ops::search to find matching task IDs,
/// then auto-expands ancestors and jumps the cursor to the next match relative
/// to the current cursor position.
fn search_in_track(app: &mut App, view_idx: usize, re: &Regex, direction: i32) {
    let track_id = match app.active_track_ids.get(view_idx) {
        Some(id) => id.clone(),
        None => return,
    };

    // Use ops::search scoped to this track to get all matching task IDs
    let hits = search_tasks(&app.project, re, Some(&track_id));
    if hits.is_empty() {
        return;
    }

    // Deduplicate: multiple hits per task → unique task IDs in order
    let mut matched_task_ids: Vec<String> = Vec::new();
    for hit in &hits {
        if !matched_task_ids.contains(&hit.task_id) {
            matched_task_ids.push(hit.task_id.clone());
        }
    }

    // Auto-expand ancestors of all matching tasks so they become visible
    for task_id in &matched_task_ids {
        auto_expand_for_task(app, &track_id, task_id);
    }

    // Rebuild flat items after expansion and collect flat indices of matching tasks
    let flat_items = app.build_flat_items(&track_id);
    let track = match App::find_track_in_project(&app.project, &track_id) {
        Some(t) => t,
        None => return,
    };

    let mut match_positions: Vec<usize> = Vec::new();
    for (fi, item) in flat_items.iter().enumerate() {
        if let FlatItem::Task { section, path, .. } = item {
            if let Some(task) = resolve_task_from_track(track, *section, path) {
                if matched_task_ids
                    .iter()
                    .any(|id| task.id.as_deref() == Some(id.as_str()))
                {
                    match_positions.push(fi);
                }
            }
        }
    }

    if match_positions.is_empty() {
        return;
    }

    let current_cursor = app.track_states.get(&track_id).map_or(0, |s| s.cursor);

    if let Some((idx, wrapped)) = find_next_match_position(&match_positions, current_cursor, direction) {
        app.search_match_idx = idx;
        if wrapped {
            app.search_wrap_message = Some(if direction == 1 {
                "Search wrapped to top".to_string()
            } else {
                "Search wrapped to bottom".to_string()
            });
        }
        let state = app.get_track_state(&track_id);
        state.cursor = match_positions[idx];
    }
}

/// Auto-expand all ancestor nodes of a task so it becomes visible in the flat list.
fn auto_expand_for_task(app: &mut App, track_id: &str, target_task_id: &str) {
    // First pass: collect expand keys immutably
    let keys_to_expand = {
        let track = match App::find_track_in_project(&app.project, track_id) {
            Some(t) => t,
            None => return,
        };

        let mut keys = Vec::new();
        for section_kind in [SectionKind::Backlog, SectionKind::Parked, SectionKind::Done] {
            let tasks = track.section_tasks(section_kind);
            if let Some(path) = find_task_path(tasks, target_task_id) {
                for depth in 0..path.len().saturating_sub(1) {
                    let ancestor_path = &path[..=depth];
                    let mut current = match tasks.get(ancestor_path[0]) {
                        Some(t) => t,
                        None => break,
                    };
                    for &pi in &ancestor_path[1..] {
                        current = match current.subtasks.get(pi) {
                            Some(t) => t,
                            None => break,
                        };
                    }
                    keys.push(crate::tui::app::task_expand_key(
                        current,
                        section_kind,
                        ancestor_path,
                    ));
                }
                break;
            }
        }
        keys
    };

    // Second pass: insert keys mutably
    let state = app.get_track_state(track_id);
    for key in keys_to_expand {
        state.expanded.insert(key);
    }
}

/// Find the index path to a task with the given ID within a task tree.
/// Returns e.g. [2, 0, 1] meaning tasks[2].subtasks[0].subtasks[1].
fn find_task_path(tasks: &[crate::model::Task], target_id: &str) -> Option<Vec<usize>> {
    for (i, task) in tasks.iter().enumerate() {
        if task.id.as_deref() == Some(target_id) {
            return Some(vec![i]);
        }
        if let Some(mut sub_path) = find_task_path(&task.subtasks, target_id) {
            sub_path.insert(0, i);
            return Some(sub_path);
        }
    }
    None
}

fn search_in_tracks_view(app: &mut App, re: &Regex, direction: i32) {
    let match_positions: Vec<usize> = app
        .project
        .config
        .tracks
        .iter()
        .enumerate()
        .filter(|(_, tc)| re.is_match(&tc.name) || re.is_match(&tc.id))
        .map(|(i, _)| i)
        .collect();

    if match_positions.is_empty() {
        return;
    }

    let current_cursor = app.tracks_cursor;
    if let Some((idx, wrapped)) = find_next_match_position(&match_positions, current_cursor, direction) {
        app.search_match_idx = idx;
        if wrapped {
            app.search_wrap_message = Some(if direction == 1 {
                "Search wrapped to top".to_string()
            } else {
                "Search wrapped to bottom".to_string()
            });
        }
        app.tracks_cursor = match_positions[idx];
    }
}

fn search_in_inbox(app: &mut App, re: &Regex, direction: i32) {
    let inbox = match &app.project.inbox {
        Some(inbox) => inbox,
        None => return,
    };

    let hits = search_inbox(inbox, re);
    if hits.is_empty() {
        return;
    }

    // Deduplicate by item index and sort by position
    let mut match_positions: Vec<usize> = Vec::new();
    for hit in &hits {
        if !match_positions.contains(&hit.item_index) {
            match_positions.push(hit.item_index);
        }
    }
    match_positions.sort();

    let current_cursor = app.inbox_cursor;
    if let Some((idx, wrapped)) = find_next_match_position(&match_positions, current_cursor, direction) {
        app.search_match_idx = idx;
        if wrapped {
            app.search_wrap_message = Some(if direction == 1 {
                "Search wrapped to top".to_string()
            } else {
                "Search wrapped to bottom".to_string()
            });
        }
        app.inbox_cursor = match_positions[idx];
    }
}

fn search_in_recent(app: &mut App, re: &Regex, direction: i32) {
    // Search done tasks across all tracks using ops::search
    let all_hits = search_tasks(&app.project, re, None);

    // Collect done task IDs that matched (search_tasks searches all sections)
    let mut matched_done_ids: Vec<String> = Vec::new();
    for hit in &all_hits {
        // Check if this task is actually in a Done section
        for (tid, track) in &app.project.tracks {
            if *tid != hit.track_id {
                continue;
            }
            for done_task in track.section_tasks(SectionKind::Done) {
                if done_task.id.as_deref() == Some(hit.task_id.as_str())
                    && !matched_done_ids.contains(&hit.task_id)
                {
                    matched_done_ids.push(hit.task_id.clone());
                }
            }
        }
    }

    if matched_done_ids.is_empty() {
        return;
    }

    // Build the same ordering as recent_view: collect all done tasks sorted by resolved date
    let mut done_tasks: Vec<(String, String)> = Vec::new();
    for (track_id, track) in &app.project.tracks {
        for task in track.section_tasks(SectionKind::Done) {
            let resolved = task
                .metadata
                .iter()
                .find_map(|m| {
                    if let crate::model::Metadata::Resolved(d) = m {
                        Some(d.clone())
                    } else {
                        None
                    }
                })
                .unwrap_or_default();
            done_tasks.push((
                task.id.clone().unwrap_or_default(),
                format!("{}:{}", track_id, resolved),
            ));
        }
    }
    done_tasks.sort_by(|a, b| b.1.cmp(&a.1));

    // Find flat indices of matching done tasks
    let match_positions: Vec<usize> = done_tasks
        .iter()
        .enumerate()
        .filter(|(_, (id, _))| matched_done_ids.contains(id))
        .map(|(i, _)| i)
        .collect();

    if match_positions.is_empty() {
        return;
    }

    let current_cursor = app.recent_cursor;
    if let Some((idx, wrapped)) = find_next_match_position(&match_positions, current_cursor, direction) {
        app.search_match_idx = idx;
        if wrapped {
            app.search_wrap_message = Some(if direction == 1 {
                "Search wrapped to top".to_string()
            } else {
                "Search wrapped to bottom".to_string()
            });
        }
        app.recent_cursor = match_positions[idx];
    }
}

/// Count unique matches for a regex pattern in the current view.
fn count_matches_for_pattern(app: &App, re: &Regex) -> usize {
    match &app.view {
        View::Detail { .. } => 0,
        View::Track(idx) => {
            let track_id = match app.active_track_ids.get(*idx) {
                Some(id) => id.as_str(),
                None => return 0,
            };
            let hits = search_tasks(&app.project, re, Some(track_id));
            let mut seen: Vec<&str> = Vec::new();
            for hit in &hits {
                if !seen.contains(&hit.task_id.as_str()) {
                    seen.push(&hit.task_id);
                }
            }
            seen.len()
        }
        View::Tracks => app
            .project
            .config
            .tracks
            .iter()
            .filter(|tc| re.is_match(&tc.name) || re.is_match(&tc.id))
            .count(),
        View::Inbox => {
            let inbox = match &app.project.inbox {
                Some(inbox) => inbox,
                None => return 0,
            };
            let hits = search_inbox(inbox, re);
            let mut seen: Vec<usize> = Vec::new();
            for hit in &hits {
                if !seen.contains(&hit.item_index) {
                    seen.push(hit.item_index);
                }
            }
            seen.len()
        }
        View::Recent => {
            let all_hits = search_tasks(&app.project, re, None);
            let mut matched_done_ids: Vec<String> = Vec::new();
            for hit in &all_hits {
                for (tid, track) in &app.project.tracks {
                    if *tid != hit.track_id {
                        continue;
                    }
                    for done_task in track.section_tasks(SectionKind::Done) {
                        if done_task.id.as_deref() == Some(hit.task_id.as_str())
                            && !matched_done_ids.contains(&hit.task_id)
                        {
                            matched_done_ids.push(hit.task_id.clone());
                        }
                    }
                }
            }
            matched_done_ids.len()
        }
    }
}

/// Update search_match_count based on current search input (for real-time display in Search mode).
fn update_match_count(app: &mut App) {
    if let Some(re) = app.active_search_re() {
        app.search_match_count = Some(count_matches_for_pattern(app, &re));
    } else {
        app.search_match_count = None;
    }
}

/// Switch to the next/prev tab. Direction: 1 = forward, -1 = backward.
fn switch_tab(app: &mut App, direction: i32) {
    let total_tracks = app.active_track_ids.len();
    // All views in order: Track(0)..Track(N-1), Tracks, Inbox, Recent
    let total_views = total_tracks + 3;

    let current_idx = match &app.view {
        View::Track(i) => *i,
        View::Detail { track_id, .. } => {
            // When in detail view, tab switching goes back to track view
            app.active_track_ids.iter().position(|id| id == track_id).unwrap_or(0)
        }
        View::Tracks => total_tracks,
        View::Inbox => total_tracks + 1,
        View::Recent => total_tracks + 2,
    };
    // Close detail view if open
    app.close_detail_fully();

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

/// Activate autocomplete for the given detail region
fn activate_autocomplete_for_region(app: &mut App, region: DetailRegion) {
    let (kind, candidates) = match region {
        DetailRegion::Tags => (AutocompleteKind::Tag, app.collect_all_tags()),
        DetailRegion::Deps => (AutocompleteKind::TaskId, app.collect_all_task_ids()),
        DetailRegion::Spec | DetailRegion::Refs => (AutocompleteKind::FilePath, app.collect_file_paths()),
        _ => {
            app.autocomplete = None;
            return;
        }
    };

    if candidates.is_empty() {
        app.autocomplete = None;
        return;
    }

    let mut ac = AutocompleteState::new(kind, candidates);
    // Filter with current edit buffer content
    let filter_text = autocomplete_filter_text(&app.edit_buffer, kind);
    ac.filter(&filter_text);
    app.autocomplete = Some(ac);
}

/// Get the relevant filter text from the edit buffer for autocomplete.
/// For tags: the current word being typed (after last space).
/// For deps: the current word being typed (after last comma or space).
/// For file paths: the whole buffer (single path).
fn autocomplete_filter_text(buffer: &str, kind: AutocompleteKind) -> String {
    match kind {
        AutocompleteKind::Tag => {
            // Get last word (which may start with #)
            let word = buffer.rsplit_once(' ').map(|(_, w)| w).unwrap_or(buffer);
            word.strip_prefix('#').unwrap_or(word).to_string()
        }
        AutocompleteKind::TaskId => {
            // Get last entry (after comma or space)
            let word = buffer
                .rsplit(|c: char| c == ',' || c.is_whitespace())
                .next()
                .unwrap_or(buffer)
                .trim();
            word.to_string()
        }
        AutocompleteKind::FilePath => {
            // Get current entry (after last space for multi-value refs)
            let word = buffer
                .rsplit(' ')
                .next()
                .unwrap_or(buffer)
                .trim();
            word.to_string()
        }
    }
}

/// Update autocomplete filter when text changes
fn update_autocomplete_filter(app: &mut App) {
    if let Some(ac) = &mut app.autocomplete {
        let kind = ac.kind;
        let filter_text = autocomplete_filter_text(&app.edit_buffer, kind);
        ac.filter(&filter_text);
        // Hide if no matches
        ac.visible = !ac.filtered.is_empty();
    }
}

/// Accept the currently selected autocomplete entry
fn autocomplete_accept(app: &mut App) {
    let (selected, kind) = match &app.autocomplete {
        Some(ac) => match ac.selected_entry() {
            Some(s) => (s.to_string(), ac.kind),
            None => {
                app.autocomplete = None;
                return;
            }
        },
        None => return,
    };

    match kind {
        AutocompleteKind::Tag => {
            // Replace the current word with the selected tag (skip duplicates)
            let existing: Vec<String> = app.edit_buffer
                .split_whitespace()
                .map(|s| s.strip_prefix('#').unwrap_or(s).to_string())
                .collect();
            let buf = &app.edit_buffer;
            let last_space = buf.rfind(' ');
            if existing.iter().any(|e| *e == selected) {
                // Already present — clear the current word being typed
                if let Some(pos) = last_space {
                    app.edit_buffer.truncate(pos + 1);
                }
            } else {
                let insert_value = format!("#{}", selected);
                if let Some(pos) = last_space {
                    app.edit_buffer.truncate(pos + 1);
                    app.edit_buffer.push_str(&insert_value);
                } else {
                    app.edit_buffer = insert_value;
                }
                app.edit_buffer.push(' ');
            }
            app.edit_cursor = app.edit_buffer.len();
        }
        AutocompleteKind::TaskId => {
            // Replace the current entry with the selected ID (skip duplicates)
            let existing: Vec<&str> = app.edit_buffer
                .split(|c: char| c == ',' || c.is_whitespace())
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .collect();
            let buf = &app.edit_buffer;
            let last_sep = buf.rfind(|c: char| c == ',' || c.is_whitespace());
            if existing.iter().any(|e| *e == selected) {
                // Already present — clear the current word being typed
                if let Some(pos) = last_sep {
                    app.edit_buffer.truncate(pos + 1);
                    if !app.edit_buffer.ends_with(' ') {
                        app.edit_buffer.push(' ');
                    }
                }
            } else {
                if let Some(pos) = last_sep {
                    app.edit_buffer.truncate(pos + 1);
                    if !app.edit_buffer.ends_with(' ') {
                        app.edit_buffer.push(' ');
                    }
                    app.edit_buffer.push_str(&selected);
                } else {
                    app.edit_buffer = selected;
                }
            }
            app.edit_cursor = app.edit_buffer.len();
        }
        AutocompleteKind::FilePath => {
            // Support space-separated entries (for refs); normalized to commas on confirm
            // Check for duplicate: skip if this path is already in the buffer
            let existing: Vec<&str> = app.edit_buffer
                .split_whitespace()
                .collect();
            if existing.iter().any(|e| *e == selected) {
                // Already present — just move cursor to end and dismiss current filter word
                let buf = &app.edit_buffer;
                let last_space = buf.rfind(' ');
                if let Some(pos) = last_space {
                    app.edit_buffer.truncate(pos + 1);
                } else {
                    app.edit_buffer.push(' ');
                }
                app.edit_cursor = app.edit_buffer.len();
            } else {
                let buf = &app.edit_buffer;
                let last_space = buf.rfind(' ');
                if let Some(pos) = last_space {
                    app.edit_buffer.truncate(pos + 1);
                    app.edit_buffer.push_str(&selected);
                } else {
                    app.edit_buffer = selected;
                }
                app.edit_buffer.push(' ');
                app.edit_cursor = app.edit_buffer.len();
            }
        }
    }

    // Re-filter after acceptance (so user can keep adding more)
    update_autocomplete_filter(app);
}

// ---------------------------------------------------------------------------
// Inbox interactions (Phase 7.2)
// ---------------------------------------------------------------------------

/// Add a new inbox item at the bottom and enter EDIT mode for its title.
fn inbox_add_item(app: &mut App) {
    let inbox = match &mut app.project.inbox {
        Some(inbox) => inbox,
        None => return,
    };

    // Add an empty item at the end
    let item = crate::model::inbox::InboxItem::new(String::new());
    inbox.items.push(item);
    let new_index = inbox.items.len() - 1;

    // Move cursor to new item
    app.inbox_cursor = new_index;

    // Enter EDIT mode for the title
    app.edit_buffer.clear();
    app.edit_cursor = 0;
    app.edit_target = Some(EditTarget::NewInboxItem { index: new_index });
    app.edit_history = Some(EditHistory::new("", 0, 0));
    app.mode = Mode::Edit;
}

/// Insert a new inbox item after the current cursor position and enter EDIT mode.
fn inbox_insert_after(app: &mut App) {
    let inbox = match &mut app.project.inbox {
        Some(inbox) => inbox,
        None => return,
    };

    let insert_at = if inbox.items.is_empty() {
        0
    } else {
        (app.inbox_cursor + 1).min(inbox.items.len())
    };

    let item = crate::model::inbox::InboxItem::new(String::new());
    inbox.items.insert(insert_at, item);

    // Move cursor to the new item
    app.inbox_cursor = insert_at;

    // Enter EDIT mode for the title
    app.edit_buffer.clear();
    app.edit_cursor = 0;
    app.edit_target = Some(EditTarget::NewInboxItem { index: insert_at });
    app.edit_history = Some(EditHistory::new("", 0, 0));
    app.mode = Mode::Edit;
}

/// Edit the title of the selected inbox item.
fn inbox_edit_title(app: &mut App) {
    let inbox = match &app.project.inbox {
        Some(inbox) => inbox,
        None => return,
    };
    let item = match inbox.items.get(app.inbox_cursor) {
        Some(item) => item,
        None => return,
    };

    let original_title = item.title.clone();
    app.edit_buffer = original_title.clone();
    app.edit_cursor = app.edit_buffer.len();
    app.edit_target = Some(EditTarget::ExistingInboxTitle {
        index: app.inbox_cursor,
        original_title,
    });
    app.edit_history = Some(EditHistory::new(&app.edit_buffer, app.edit_cursor, 0));
    app.mode = Mode::Edit;
}

/// Edit the tags of the selected inbox item.
fn inbox_edit_tags(app: &mut App) {
    let inbox = match &app.project.inbox {
        Some(inbox) => inbox,
        None => return,
    };
    let item = match inbox.items.get(app.inbox_cursor) {
        Some(item) => item,
        None => return,
    };

    let original_tags: String = item.tags.iter().map(|t| format!("#{}", t)).collect::<Vec<_>>().join(" ");
    app.edit_buffer = if original_tags.is_empty() { String::new() } else { format!("{} ", original_tags) };
    app.edit_cursor = app.edit_buffer.len();
    app.edit_target = Some(EditTarget::ExistingInboxTags {
        index: app.inbox_cursor,
        original_tags: original_tags.clone(),
    });
    app.edit_history = Some(EditHistory::new(&app.edit_buffer, app.edit_cursor, 0));

    // Activate tag autocomplete
    let candidates = app.collect_all_tags();
    app.autocomplete = Some(AutocompleteState::new(AutocompleteKind::Tag, candidates));
    update_autocomplete_filter(app);

    app.mode = Mode::Edit;
}

/// Delete the selected inbox item (with confirmation).
fn inbox_delete_item(app: &mut App) {
    let inbox = match &app.project.inbox {
        Some(inbox) => inbox,
        None => return,
    };
    if inbox.items.is_empty() || app.inbox_cursor >= inbox.items.len() {
        return;
    }

    let title = &inbox.items[app.inbox_cursor].title;
    let short_title = if title.len() > 30 {
        format!("{}...", &title[..30])
    } else {
        title.clone()
    };

    app.confirm_state = Some(super::app::ConfirmState {
        message: format!("Delete \"{}\"? (y/n)", short_title),
        action: super::app::ConfirmAction::DeleteInboxItem { index: app.inbox_cursor },
    });
    app.mode = Mode::Confirm;
}

/// Enter MOVE mode for inbox items.
fn inbox_enter_move_mode(app: &mut App) {
    let count = app.inbox_count();
    if count == 0 || app.inbox_cursor >= count {
        return;
    }

    app.move_state = Some(MoveState::InboxItem {
        original_index: app.inbox_cursor,
    });
    app.mode = Mode::Move;
}

/// Begin the triage flow for the selected inbox item.
fn inbox_begin_triage(app: &mut App) {
    let count = app.inbox_count();
    if count == 0 || app.inbox_cursor >= count {
        return;
    }

    // Activate track selection autocomplete
    let active_tracks: Vec<String> = app
        .project
        .config
        .tracks
        .iter()
        .filter(|t| t.state == "active")
        .map(|t| format!("{} ({})", t.name, t.id))
        .collect();

    if active_tracks.is_empty() {
        app.status_message = Some("No active tracks to triage to".to_string());
        return;
    }

    app.edit_buffer.clear();
    app.edit_cursor = 0;
    app.autocomplete = Some(AutocompleteState::new(AutocompleteKind::Tag, active_tracks));
    if let Some(ac) = &mut app.autocomplete {
        ac.filter(""); // Show all
    }

    app.triage_state = Some(super::app::TriageState {
        inbox_index: app.inbox_cursor,
        step: super::app::TriageStep::SelectTrack,
        popup_anchor: None,
    });
    app.mode = Mode::Triage;
}

// ---------------------------------------------------------------------------
// Triage mode handler (Phase 7.3)
// ---------------------------------------------------------------------------

fn handle_triage(app: &mut App, key: KeyEvent) {
    let step = match &app.triage_state {
        Some(ts) => ts.step.clone(),
        None => {
            app.mode = Mode::Navigate;
            return;
        }
    };

    match step {
        super::app::TriageStep::SelectTrack => handle_triage_select_track(app, key),
        super::app::TriageStep::SelectPosition { track_id } => {
            handle_triage_select_position(app, key, &track_id.clone())
        }
    }
}

fn handle_triage_select_track(app: &mut App, key: KeyEvent) {
    match (key.modifiers, key.code) {
        // Cancel
        (_, KeyCode::Esc) => {
            app.mode = Mode::Navigate;
            app.triage_state = None;
            app.autocomplete = None;
            app.edit_buffer.clear();
        }

        // Navigate autocomplete
        (KeyModifiers::NONE, KeyCode::Up) => {
            if let Some(ac) = &mut app.autocomplete {
                ac.move_up();
            }
        }
        (KeyModifiers::NONE, KeyCode::Down) => {
            if let Some(ac) = &mut app.autocomplete {
                ac.move_down();
            }
        }

        // Select track
        (_, KeyCode::Enter) => {
            let selected = app.autocomplete.as_ref().and_then(|ac| ac.selected_entry().map(|s| s.to_string()));
            if let Some(entry) = selected {
                // Extract track_id from "Track Name (track_id)"
                let track_id = entry
                    .rsplit('(')
                    .next()
                    .and_then(|s| s.strip_suffix(')'))
                    .unwrap_or(&entry)
                    .to_string();

                // Verify track exists
                let valid = app.project.config.tracks.iter().any(|t| t.id == track_id);
                if valid {
                    // Capture anchor from autocomplete before clearing it
                    let anchor = app.autocomplete_anchor;
                    app.autocomplete = None;
                    app.edit_buffer.clear();
                    if let Some(ts) = &mut app.triage_state {
                        ts.popup_anchor = anchor;
                        ts.step = super::app::TriageStep::SelectPosition { track_id };
                    }
                }
            }
        }

        // Filter by typing
        (KeyModifiers::NONE | KeyModifiers::SHIFT, KeyCode::Char(c)) => {
            app.edit_buffer.push(c);
            app.edit_cursor = app.edit_buffer.len();
            if let Some(ac) = &mut app.autocomplete {
                ac.filter(&app.edit_buffer);
            }
        }

        // Backspace
        (_, KeyCode::Backspace) => {
            app.edit_buffer.pop();
            app.edit_cursor = app.edit_buffer.len();
            if let Some(ac) = &mut app.autocomplete {
                ac.filter(&app.edit_buffer);
            }
        }

        _ => {}
    }
}

fn handle_triage_select_position(app: &mut App, key: KeyEvent, track_id: &str) {
    match (key.modifiers, key.code) {
        // Cancel
        (_, KeyCode::Esc) => {
            app.mode = Mode::Navigate;
            app.triage_state = None;
            app.autocomplete = None;
            app.edit_buffer.clear();
        }

        // t = top, b = bottom (default), Enter = bottom
        (KeyModifiers::NONE, KeyCode::Char('t')) => {
            execute_triage(app, track_id, InsertPosition::Top);
        }
        (KeyModifiers::NONE, KeyCode::Char('b')) | (_, KeyCode::Enter) => {
            execute_triage(app, track_id, InsertPosition::Bottom);
        }

        _ => {}
    }
}

fn execute_triage(app: &mut App, track_id: &str, position: InsertPosition) {
    let inbox_index = match &app.triage_state {
        Some(ts) => ts.inbox_index,
        None => return,
    };

    // Get the item before triaging (for undo)
    let inbox_item = match &app.project.inbox {
        Some(inbox) => match inbox.items.get(inbox_index) {
            Some(item) => item.clone(),
            None => return,
        },
        None => return,
    };

    let prefix = app.track_prefix(track_id).unwrap_or("").to_string();

    let inbox = match &mut app.project.inbox {
        Some(inbox) => inbox,
        None => return,
    };
    let track = match app.project.tracks.iter_mut().find(|(id, _)| id == track_id) {
        Some((_, track)) => track,
        None => return,
    };

    let task_id = match crate::ops::inbox_ops::triage(inbox, inbox_index, track, position, &prefix) {
        Ok(id) => id,
        Err(_) => return,
    };

    // Push undo operation
    app.undo_stack.push(Operation::InboxTriage {
        inbox_index,
        item: inbox_item,
        track_id: track_id.to_string(),
        task_id,
    });

    // Save both inbox and track
    let _ = app.save_inbox();
    let _ = app.save_track(track_id);

    // Advance cursor (or clamp to last item)
    let count = app.inbox_count();
    if count == 0 {
        app.inbox_cursor = 0;
    } else {
        app.inbox_cursor = app.inbox_cursor.min(count - 1);
    }

    // Return to navigate mode
    app.mode = Mode::Navigate;
    app.triage_state = None;
    app.autocomplete = None;
    app.edit_buffer.clear();

    let track_name = app.track_name(track_id).to_string();
    app.status_message = Some(format!("Triaged to {}", track_name));
}

// ---------------------------------------------------------------------------
// Confirm mode handler
// ---------------------------------------------------------------------------

fn handle_confirm(app: &mut App, key: KeyEvent) {
    match (key.modifiers, key.code) {
        // Confirm: y
        (KeyModifiers::NONE, KeyCode::Char('y')) => {
            let state = app.confirm_state.take();
            app.mode = Mode::Navigate;
            if let Some(state) = state {
                match state.action {
                    super::app::ConfirmAction::DeleteInboxItem { index } => {
                        confirm_inbox_delete(app, index);
                    }
                }
            }
        }
        // Cancel: n or Esc
        (KeyModifiers::NONE, KeyCode::Char('n')) | (_, KeyCode::Esc) => {
            app.confirm_state = None;
            app.mode = Mode::Navigate;
        }
        _ => {}
    }
}

fn confirm_inbox_delete(app: &mut App, index: usize) {
    let inbox = match &mut app.project.inbox {
        Some(inbox) => inbox,
        None => return,
    };

    if index >= inbox.items.len() {
        return;
    }

    let item = inbox.items.remove(index);
    app.undo_stack.push(Operation::InboxDelete {
        index,
        item,
    });
    let _ = app.save_inbox();

    // Clamp cursor
    let count = app.inbox_count();
    if count == 0 {
        app.inbox_cursor = 0;
    } else {
        app.inbox_cursor = app.inbox_cursor.min(count - 1);
    }
}

// ---------------------------------------------------------------------------
// Recent view interactions (Phase 7.4)
// ---------------------------------------------------------------------------

/// Reopen a done task from the recent view (set state back to todo).
fn reopen_recent_task(app: &mut App) {
    // Rebuild the sorted done-task list to find the task at current cursor
    let mut done_tasks: Vec<(String, String, String)> = Vec::new(); // (track_id, task_id, resolved)

    for (track_id, track) in &app.project.tracks {
        for task in track.section_tasks(SectionKind::Done) {
            let resolved = task
                .metadata
                .iter()
                .find_map(|m| {
                    if let crate::model::task::Metadata::Resolved(d) = m {
                        Some(d.clone())
                    } else {
                        None
                    }
                })
                .unwrap_or_default();
            if let Some(ref id) = task.id {
                done_tasks.push((track_id.clone(), id.clone(), resolved));
            }
        }
    }

    // Sort by resolved date, most recent first (matching render order)
    done_tasks.sort_by(|a, b| b.2.cmp(&a.2));

    let cursor = app.recent_cursor;
    let (track_id, task_id) = match done_tasks.get(cursor) {
        Some((tid, taskid, _)) => (tid.clone(), taskid.clone()),
        None => return,
    };

    // Move task from Done section to Backlog
    let track = match app.find_track_mut(&track_id) {
        Some(t) => t,
        None => return,
    };

    // Remove from Done section
    let (done_index, mut task) = {
        let done = match track.section_tasks_mut(SectionKind::Done) {
            Some(d) => d,
            None => return,
        };
        let idx = match done.iter().position(|t| t.id.as_deref() == Some(task_id.as_str())) {
            Some(i) => i,
            None => return,
        };
        (idx, done.remove(idx))
    };

    // Capture old state for undo
    let old_state = task.state;
    let old_resolved = task
        .metadata
        .iter()
        .find_map(|m| {
            if let crate::model::task::Metadata::Resolved(d) = m {
                Some(d.clone())
            } else {
                None
            }
        });

    // Set state to Todo and insert at top of Backlog
    task.state = crate::model::task::TaskState::Todo;
    task.metadata.retain(|m| m.key() != "resolved");
    task.mark_dirty();

    if let Some(backlog) = track.section_tasks_mut(SectionKind::Backlog) {
        backlog.insert(0, task);
    }

    app.undo_stack.push(Operation::Reopen {
        track_id: track_id.clone(),
        task_id: task_id.clone(),
        old_state,
        old_resolved,
        done_index,
    });

    let _ = app.save_track(&track_id);

    // Clamp recent cursor
    let count = count_recent_tasks(app);
    if count == 0 {
        app.recent_cursor = 0;
    } else {
        app.recent_cursor = app.recent_cursor.min(count - 1);
    }

    let track_name = app.track_name(&track_id).to_string();
    app.status_message = Some(format!("Reopened in {}", track_name));
}
