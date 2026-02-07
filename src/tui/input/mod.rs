use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use regex::Regex;

use crate::model::task::Metadata;
use crate::model::SectionKind;
use crate::ops::search::{search_inbox, search_tasks};
use crate::ops::task_ops::{self, InsertPosition};

use super::app::{App, EditTarget, FlatItem, Mode, MoveState, View, resolve_task_from_flat};
use super::undo::Operation;

/// Handle a key event in the current mode
pub fn handle_key(app: &mut App, key: KeyEvent) {
    match &app.mode {
        Mode::Navigate => handle_navigate(app, key),
        Mode::Search => handle_search(app, key),
        Mode::Edit => handle_edit(app, key),
        Mode::Move => handle_move(app, key),
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

        // Esc: clear search first, then normal behavior
        (_, KeyCode::Esc) => {
            if app.last_search.is_some() {
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

        // Search: n/N for next/prev match
        (KeyModifiers::NONE, KeyCode::Char('n')) => {
            if app.last_search.is_some() {
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

        // Expand/collapse (track view only)
        (KeyModifiers::NONE, KeyCode::Right | KeyCode::Char('l')) => {
            expand_or_enter(app);
        }
        (KeyModifiers::NONE, KeyCode::Left | KeyCode::Char('h')) => {
            collapse_or_parent(app);
        }

        // Task state changes (track view only)
        (KeyModifiers::NONE, KeyCode::Char(' ')) => {
            task_state_action(app, StateAction::Cycle);
        }
        (KeyModifiers::NONE, KeyCode::Char('x')) => {
            task_state_action(app, StateAction::Done);
        }
        (KeyModifiers::NONE, KeyCode::Char('b')) => {
            task_state_action(app, StateAction::ToggleBlocked);
        }
        (KeyModifiers::NONE, KeyCode::Char('~')) => {
            task_state_action(app, StateAction::ToggleParked);
        }

        // Add task (track view only)
        (KeyModifiers::NONE, KeyCode::Char('a')) => {
            add_task_action(app, AddPosition::Bottom);
        }
        (KeyModifiers::NONE, KeyCode::Char('o') | KeyCode::Char('-')) => {
            add_task_action(app, AddPosition::AfterCursor);
        }
        (KeyModifiers::NONE, KeyCode::Char('p')) => {
            add_task_action(app, AddPosition::Top);
        }
        (KeyModifiers::SHIFT, KeyCode::Char('A')) => {
            add_subtask_action(app);
        }

        // Inline title edit
        (KeyModifiers::NONE, KeyCode::Char('e')) => {
            enter_title_edit(app);
        }

        // Toggle cc tag on task
        (KeyModifiers::NONE, KeyCode::Char('c')) => {
            toggle_cc_tag(app);
        }

        // Set cc-focus to current track
        (KeyModifiers::SHIFT, KeyCode::Char('C')) => {
            set_cc_focus_current(app);
        }

        // Move mode
        (KeyModifiers::NONE, KeyCode::Char('m')) => {
            enter_move_mode(app);
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
    if let Some(track_id) = app.undo_stack.undo(&mut app.project.tracks) {
        let _ = app.save_track(&track_id);
    }
}

fn perform_redo(app: &mut App) {
    if let Some(track_id) = app.undo_stack.redo(&mut app.project.tracks) {
        let _ = app.save_track(&track_id);
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
    app.mode = Mode::Edit;
}

// ---------------------------------------------------------------------------
// EDIT mode input
// ---------------------------------------------------------------------------

fn handle_edit(app: &mut App, key: KeyEvent) {
    match (key.modifiers, key.code) {
        // Confirm edit
        (_, KeyCode::Enter) => {
            confirm_edit(app);
        }
        // Cancel edit
        (_, KeyCode::Esc) => {
            cancel_edit(app);
        }
        // Cursor movement
        (KeyModifiers::NONE, KeyCode::Left) => {
            if app.edit_cursor > 0 {
                app.edit_cursor -= 1;
            }
        }
        (KeyModifiers::NONE, KeyCode::Right) => {
            if app.edit_cursor < app.edit_buffer.len() {
                app.edit_cursor += 1;
            }
        }
        // Jump to start/end of line
        (m, KeyCode::Left) if m.contains(KeyModifiers::SUPER) => {
            app.edit_cursor = 0;
        }
        (m, KeyCode::Right) if m.contains(KeyModifiers::SUPER) => {
            app.edit_cursor = app.edit_buffer.len();
        }
        // Word movement
        (m, KeyCode::Left) if m.contains(KeyModifiers::ALT) => {
            app.edit_cursor = word_boundary_left(&app.edit_buffer, app.edit_cursor);
        }
        (m, KeyCode::Right) if m.contains(KeyModifiers::ALT) => {
            app.edit_cursor = word_boundary_right(&app.edit_buffer, app.edit_cursor);
        }
        // Backspace
        (KeyModifiers::NONE, KeyCode::Backspace) => {
            if app.edit_cursor > 0 {
                app.edit_buffer.remove(app.edit_cursor - 1);
                app.edit_cursor -= 1;
            }
        }
        // Word backspace
        (m, KeyCode::Backspace) if m.contains(KeyModifiers::ALT) => {
            let new_pos = word_boundary_left(&app.edit_buffer, app.edit_cursor);
            app.edit_buffer.drain(new_pos..app.edit_cursor);
            app.edit_cursor = new_pos;
        }
        // Type character
        (KeyModifiers::NONE | KeyModifiers::SHIFT, KeyCode::Char(c)) => {
            app.edit_buffer.insert(app.edit_cursor, c);
            app.edit_cursor += 1;
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
    }
}

fn cancel_edit(app: &mut App) {
    let target = app.edit_target.take();
    let saved_cursor = app.pre_edit_cursor.take();
    app.mode = Mode::Navigate;

    // If we were creating a new task, remove the placeholder
    if let Some(EditTarget::NewTask {
        task_id,
        track_id,
        parent_id,
    }) = target
    {
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
    // For existing title edit, cancel means revert (title unchanged since we didn't write)
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
                        // Position cursor on the moved track
                        let _ = (track_id, original_index);
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
            if is_track_move {
                move_track_in_list(app, -1);
            } else {
                move_task_in_list(app, -1);
            }
        }
        // Move down
        (KeyModifiers::NONE, KeyCode::Down | KeyCode::Char('j')) => {
            if is_track_move {
                move_track_in_list(app, 1);
            } else {
                move_task_in_list(app, 1);
            }
        }
        // Move to top
        (KeyModifiers::NONE, KeyCode::Char('g')) => {
            if is_track_move {
                move_track_to_boundary(app, true);
            } else {
                move_task_to_boundary(app, true);
            }
        }
        (m, KeyCode::Up) if m.contains(KeyModifiers::SUPER) => {
            if is_track_move {
                move_track_to_boundary(app, true);
            } else {
                move_task_to_boundary(app, true);
            }
        }
        (_, KeyCode::Home) => {
            if is_track_move {
                move_track_to_boundary(app, true);
            } else {
                move_task_to_boundary(app, true);
            }
        }
        // Move to bottom
        (KeyModifiers::SHIFT, KeyCode::Char('G')) => {
            if is_track_move {
                move_track_to_boundary(app, false);
            } else {
                move_task_to_boundary(app, false);
            }
        }
        (m, KeyCode::Down) if m.contains(KeyModifiers::SUPER) => {
            if is_track_move {
                move_track_to_boundary(app, false);
            } else {
                move_task_to_boundary(app, false);
            }
        }
        (_, KeyCode::End) => {
            if is_track_move {
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
