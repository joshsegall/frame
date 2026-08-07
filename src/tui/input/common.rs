use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::collections::HashSet;

use crate::model::SectionKind;
use crate::model::task::Metadata;
use crate::ops::task_ops::{self};
use crate::util::unicode;

use crate::tui::app::{
    App, DetailRegion, DetailState, FlatItem, Mode, PendingMove, RepeatableAction, View,
    resolve_task_from_flat,
};
use crate::tui::undo::{Operation, UndoNavTarget};
use std::io::Write;
use std::process::{Command, Stdio};

use super::edit::detail_move_region;
use super::navigate::{count_recent_tasks, count_tracks};
use super::search::auto_expand_for_task;
use super::tracks::update_track_header;

pub(super) fn clipboard_set(text: &str) {
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

pub(super) fn clipboard_get() -> Option<String> {
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

/// Convert (line, col) to absolute byte offset in a multi-line buffer.
pub(super) fn multiline_pos_to_offset(text: &str, line: usize, col: usize) -> usize {
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
pub(super) fn offset_to_multiline_pos(text: &str, offset: usize) -> (usize, usize) {
    let mut remaining = offset;
    for (i, line) in text.split('\n').enumerate() {
        if remaining <= line.len() {
            return (i, remaining);
        }
        remaining -= line.len() + 1;
    }
    let line_count = text.split('\n').count();
    let last_len = text.split('\n').next_back().map_or(0, |l| l.len());
    (line_count.saturating_sub(1), last_len)
}

/// Get multi-line selection range as (start_offset, end_offset) in absolute terms.
pub fn multiline_selection_range(ds: &DetailState) -> Option<(usize, usize)> {
    let (anchor_line, anchor_col) = ds.multiline_selection_anchor?;
    let anchor_off = multiline_pos_to_offset(&ds.edit_buffer, anchor_line, anchor_col);
    let cursor_off =
        multiline_pos_to_offset(&ds.edit_buffer, ds.edit_cursor_line, ds.edit_cursor_col);
    let start = anchor_off.min(cursor_off);
    let end = anchor_off.max(cursor_off);
    if start == end {
        return None;
    }
    Some((start, end))
}

/// Get the selected text in a multi-line buffer.
pub(super) fn get_multiline_selection_text(ds: &DetailState) -> Option<String> {
    let (start, end) = multiline_selection_range(ds)?;
    Some(ds.edit_buffer[start..end].to_string())
}

/// Delete the selected text in a multi-line buffer, updating cursor position.
/// Returns the deleted text if there was a selection.
pub(super) fn delete_multiline_selection(ds: &mut DetailState) -> Option<String> {
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
pub fn selection_cols_for_line(
    buffer: &str,
    sel_start: usize,
    sel_end: usize,
    target_line: usize,
) -> Option<(usize, usize)> {
    let mut offset = 0;
    for (i, line) in buffer.split('\n').enumerate() {
        if i == target_line {
            let line_end = offset + line.len();
            let vis_start = sel_start.max(offset);
            let vis_end = sel_end.min(line_end);
            if vis_start >= vis_end {
                // Blank line within selection: return (0, 0) so the renderer
                // can show a one-column selection indicator.
                if line.is_empty() && sel_start <= offset && sel_end > offset {
                    return Some((0, 0));
                }
                return None;
            }
            return Some((vis_start - offset, vis_end - offset));
        }
        offset += line.len() + 1;
    }
    None
}

// ---------------------------------------------------------------------------
// Kitty keyboard protocol normalizer

/// Map a base key to its US-layout shifted symbol.
/// Returns None if the key is not a shiftable symbol (or is already shifted).
pub(super) fn shift_symbol(c: char) -> Option<char> {
    match c {
        '`' => Some('~'),
        '1' => Some('!'),
        '2' => Some('@'),
        '3' => Some('#'),
        '4' => Some('$'),
        '5' => Some('%'),
        '6' => Some('^'),
        '7' => Some('&'),
        '8' => Some('*'),
        '9' => Some('('),
        '0' => Some(')'),
        '-' => Some('_'),
        '=' => Some('+'),
        '[' => Some('{'),
        ']' => Some('}'),
        '\\' => Some('|'),
        ';' => Some(':'),
        '\'' => Some('"'),
        ',' => Some('<'),
        '.' => Some('>'),
        '/' => Some('?'),
        _ => None,
    }
}

/// Normalize key events from terminals using the kitty keyboard protocol.
///
/// Kitty protocol sends `Char(lowercase) + SHIFT` instead of `Char(UPPERCASE) + SHIFT`,
/// and `Char(base_symbol) + SHIFT` instead of `Char(shifted_symbol)`.
///
/// For traditional terminals (e.g. Warp) this is a no-op:
/// - Already-uppercase letters: `'P'.is_ascii_lowercase()` = false → skip
/// - Already-shifted symbols: `shift_symbol('>')` = None → skip
pub(super) fn normalize_key(mut key: KeyEvent) -> KeyEvent {
    if let KeyCode::Char(c) = key.code
        && key.modifiers.contains(KeyModifiers::SHIFT)
    {
        if c.is_ascii_lowercase() {
            // Shift+p → Char('P') with SHIFT preserved
            key.code = KeyCode::Char(c.to_ascii_uppercase());
        } else if let Some(shifted) = shift_symbol(c) {
            // Shift+. → Char('>') with SHIFT removed
            key.code = KeyCode::Char(shifted);
            key.modifiers.remove(KeyModifiers::SHIFT);
        }
    }
    key
}

/// Drain any pending watcher events for a specific track (already handled via mtime).
/// Reloads remaining pending paths for other files.
pub(super) fn drain_pending_for_track(app: &mut App, handled_track_id: &str) {
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

/// Get the task ID at the current cursor position, if any.
pub(super) fn get_cursor_task_id(app: &mut App) -> Option<String> {
    let track_id = app.current_track_id().map(|s| s.to_string())?;
    let items = app.build_flat_items(&track_id);
    let state = app.get_track_state(&track_id);
    let cursor = state.cursor;
    if cursor >= items.len() {
        return None;
    }
    if let FlatItem::Task { section, path, .. } = &items[cursor] {
        let track = App::find_track_in_project(&app.project, &track_id)?;
        let task = resolve_task_from_flat(track, *section, path)?;
        return task.id.as_ref().map(|i| i.to_string());
    }
    None
}

/// Adjust cursor after filter change: try to stay on the same task ID,
/// then fall back to keeping screen position, then find nearest selectable.
pub(super) fn reset_cursor_for_filter(app: &mut App, prev_task_id: Option<&str>) {
    if let Some(track_id) = app.current_track_id().map(|s| s.to_string()) {
        let items = app.build_flat_items(&track_id);
        let old_cursor = app.get_track_state(&track_id).cursor;

        if items.is_empty() {
            let state = app.get_track_state(&track_id);
            state.cursor = 0;
            state.scroll_offset = 0;
            return;
        }

        // First: try to find the same task ID in the filtered results
        if let Some(target_id) = prev_task_id
            && let Some(track) = App::find_track_in_project(&app.project, &track_id)
        {
            for (i, item) in items.iter().enumerate() {
                if let FlatItem::Task {
                    section,
                    path,
                    is_context,
                    ..
                } = item
                {
                    if *is_context {
                        continue;
                    }
                    if let Some(task) = resolve_task_from_flat(track, *section, path)
                        && task.id.as_deref() == Some(target_id)
                    {
                        app.get_track_state(&track_id).cursor = i;
                        return;
                    }
                }
            }
        }

        // Second: try to keep screen position (clamp to valid range)
        let cursor = old_cursor.min(items.len().saturating_sub(1));
        if !is_non_selectable(&items[cursor]) {
            app.get_track_state(&track_id).cursor = cursor;
            return;
        }

        // Third: find nearest selectable item (prefer forward, then backward)
        let forward = items[cursor..]
            .iter()
            .position(|item| !is_non_selectable(item));
        if let Some(offset) = forward {
            app.get_track_state(&track_id).cursor = cursor + offset;
            return;
        }
        let backward = items[..cursor]
            .iter()
            .rposition(|item| !is_non_selectable(item));
        if let Some(pos) = backward {
            app.get_track_state(&track_id).cursor = pos;
            return;
        }

        app.get_track_state(&track_id).cursor = 0;
    }
}

/// Clear selection and return to Navigate mode.
pub(super) fn clear_selection(app: &mut App) {
    app.selection.clear();
    app.range_anchor = None;
    app.mode = Mode::Navigate;
}

pub(super) fn perform_undo(app: &mut App) {
    // If the top of the undo stack is a StateChange to Done and there's a matching
    // pending move, cancel the pending move so undo reverts both in one press.
    if let Some(op) = app.undo_stack.peek_last_undo() {
        match op {
            Operation::StateChange {
                track_id,
                task_id,
                new_state,
                ..
            } => {
                if *new_state == crate::model::task::TaskState::Done {
                    let tid = track_id.clone();
                    let taskid = task_id.clone();
                    app.cancel_pending_move(&tid, &taskid);
                    app.cancel_pending_subtask_hide(&tid, &taskid);
                }
            }
            Operation::Reopen {
                track_id, task_id, ..
            } => {
                // Cancel any pending ToBacklog move so the undo doesn't conflict
                let tid = track_id.clone();
                let taskid = task_id.clone();
                app.cancel_pending_move(&tid, &taskid);
            }
            _ => {}
        }
    }

    // Check if this is a Bulk operation — collect affected task IDs for multi-flash
    let bulk_task_ids = collect_bulk_task_ids(app.undo_stack.peek_last_undo());

    let inbox = app.project.inbox.as_mut();
    if let Some(nav) = app.undo_stack.undo(&mut app.project.tracks, inbox) {
        apply_nav_side_effects(app, &nav, true);
        if !bulk_task_ids.is_empty() {
            // Bulk undo: save affected tracks, flash all affected tasks, don't navigate
            app.flash_tasks(bulk_task_ids);
        } else {
            navigate_to_undo_target(app, &nav);
        }
    }
}

pub(super) fn perform_redo(app: &mut App) {
    // Check if this is a Bulk operation — collect affected task IDs for multi-flash
    let bulk_task_ids = collect_bulk_task_ids(app.undo_stack.peek_last_redo());

    let inbox = app.project.inbox.as_mut();
    if let Some(nav) = app.undo_stack.redo(&mut app.project.tracks, inbox) {
        apply_nav_side_effects(app, &nav, false);
        if !bulk_task_ids.is_empty() {
            app.flash_tasks(bulk_task_ids);
        } else {
            navigate_to_undo_target(app, &nav);
        }
    }
}

/// Collect task IDs from a Bulk operation for multi-flash.
/// Returns empty set for non-Bulk operations.
pub(super) fn collect_bulk_task_ids(op: Option<&Operation>) -> HashSet<String> {
    let mut ids = HashSet::new();
    if let Some(Operation::Bulk(ops)) = op {
        for sub_op in ops {
            match sub_op {
                Operation::StateChange { task_id, .. }
                | Operation::TitleEdit { task_id, .. }
                | Operation::TaskAdd { task_id, .. }
                | Operation::SubtaskAdd { task_id, .. }
                | Operation::TaskMove { task_id, .. }
                | Operation::FieldEdit { task_id, .. } => {
                    ids.insert(task_id.clone());
                }
                Operation::CrossTrackMove {
                    task_id_old,
                    task_id_new,
                    ..
                } => {
                    ids.insert(task_id_old.clone());
                    ids.insert(task_id_new.clone());
                }
                _ => {}
            }
        }
    }
    ids
}

/// Apply side effects that undo/redo can't handle internally (e.g., config changes, saves).
pub(super) fn apply_nav_side_effects(app: &mut App, nav: &UndoNavTarget, is_undo: bool) {
    match nav {
        UndoNavTarget::Task { track_id, .. } => {
            // For cross-track moves (including bulk), also save other tracks
            let op = if is_undo {
                app.undo_stack.peek_last_redo()
            } else {
                app.undo_stack.peek_last_undo()
            };
            // A *re*done triage navigates to the new task, so it lands here
            // rather than in the `Inbox` arm below — but it removed an inbox
            // item, and that removal has to reach disk in the same lock as the
            // task that replaced it. Saving only the track left the item in
            // `inbox.md` *and* the task in the track: the same duplicate the
            // Inbox arm takes one lock to avoid, arrived at from the other
            // direction.
            if matches!(op, Some(Operation::InboxTriage { .. })) {
                app.save_batch_logged(&[track_id.as_str()], true);
            } else {
                app.save_track_logged(track_id);
            }
            let op = if is_undo {
                app.undo_stack.peek_last_redo()
            } else {
                app.undo_stack.peek_last_undo()
            };
            let mut extra_tracks: Vec<String> = Vec::new();
            // Collect reparent data before mutably borrowing app for expanded set update
            let mut reparent_expand_update: Option<(String, Vec<(String, String)>)> = None;
            match op {
                Some(Operation::CrossTrackMove {
                    source_track_id,
                    target_track_id,
                    ..
                }) => {
                    let other = if track_id == source_track_id {
                        target_track_id
                    } else {
                        source_track_id
                    };
                    extra_tracks.push(other.clone());
                }
                Some(Operation::Reparent {
                    id_mappings,
                    track_id: reparent_track_id,
                    ..
                }) if !id_mappings.is_empty() => {
                    // Reparent may update dep references across all tracks
                    for (tid, _) in &app.project.tracks {
                        if tid != track_id {
                            extra_tracks.push(tid.clone());
                        }
                    }
                    // Collect data for expanded set update (done after match to avoid borrow conflict)
                    reparent_expand_update = Some((reparent_track_id.clone(), id_mappings.clone()));
                }
                Some(Operation::Bulk(ops)) => {
                    for sub_op in ops {
                        if let Operation::CrossTrackMove {
                            source_track_id,
                            target_track_id,
                            ..
                        } = sub_op
                        {
                            if source_track_id != track_id {
                                extra_tracks.push(source_track_id.clone());
                            }
                            if target_track_id != track_id {
                                extra_tracks.push(target_track_id.clone());
                            }
                        }
                    }
                }
                _ => {}
            }
            // Preserve expand/collapse state through ID rekeying (after match to avoid borrow conflict).
            // Undo reverses new→old, so replace new_id keys with old_id.
            // Redo applies old→new, so replace old_id keys with new_id.
            if let Some((ref reparent_track, ref mappings)) = reparent_expand_update {
                let state = app.get_track_state(reparent_track);
                for (old_id, new_id) in mappings {
                    if is_undo {
                        if state.expanded.remove(new_id) {
                            state.expanded.insert(old_id.clone());
                        }
                    } else if state.expanded.remove(old_id) {
                        state.expanded.insert(new_id.clone());
                    }
                }
            }
            for other in &extra_tracks {
                app.save_track_logged(other);
            }
        }
        UndoNavTarget::TracksView { track_id } => {
            // Find the operation on the redo/undo stack (it was just moved there)
            let op = if is_undo {
                app.undo_stack.peek_last_redo().cloned()
            } else {
                app.undo_stack.peek_last_undo().cloned()
            };
            // Every arm below writes `project.toml`, and half of them also
            // create, unlink or move a track file. Undoing an archive is the
            // same two-file change as archiving, so it takes the lock the same
            // way — one lock for the whole reversal rather than one per file.
            // Saves inside write under it; see `App::with_project_lock`.
            app.with_project_lock(|app| {
                match op {
                    Some(Operation::TrackMove {
                        old_index,
                        new_index,
                        ..
                    }) => {
                        let target_index = if is_undo { old_index } else { new_index };
                        let _ = crate::ops::track_ops::reorder_tracks(
                            &mut app.project.config,
                            track_id,
                            target_index,
                        );
                        rebuild_active_track_ids(app);
                        save_config(app);
                    }
                    Some(Operation::TrackCcFocus {
                        old_focus,
                        new_focus,
                    }) => {
                        let target = if is_undo { old_focus } else { new_focus };
                        app.project.config.agent.cc_focus = target;
                        save_config(app);
                    }
                    Some(Operation::TrackNameEdit {
                        track_id: tid,
                        old_name,
                        new_name,
                    }) => {
                        let target_name = if is_undo { &old_name } else { &new_name };
                        if let Some(tc) = app.project.config.tracks.iter_mut().find(|t| t.id == tid)
                        {
                            tc.name = target_name.clone();
                        }
                        save_config(app);
                        // Update track file header
                        update_track_header(app, &tid, target_name);
                        app.save_track_logged(&tid);
                    }
                    Some(Operation::TrackShelve {
                        track_id: tid,
                        was_active,
                    }) => {
                        // Undo: restore original state; Redo: re-apply toggle
                        let new_state = if is_undo {
                            if was_active { "active" } else { "shelved" }
                        } else if was_active {
                            "shelved"
                        } else {
                            "active"
                        };
                        if let Some(tc) = app.project.config.tracks.iter_mut().find(|t| t.id == tid)
                        {
                            tc.state = new_state.to_string();
                        }
                        rebuild_active_track_ids(app);
                        save_config(app);
                    }
                    Some(Operation::TrackArchive {
                        track_id: tid,
                        old_state,
                    }) => {
                        if is_undo {
                            // Restore from archived to old_state
                            if let Some(tc) =
                                app.project.config.tracks.iter_mut().find(|t| t.id == tid)
                            {
                                tc.state = old_state.clone();
                            }
                            // Restore track file from archive/_tracks/
                            if let Some(file) = app.track_file(&tid).map(|f| f.to_string()) {
                                let _ = crate::ops::track_ops::restore_track_file(
                                    &app.project.frame_dir,
                                    &tid,
                                    &file,
                                );
                            }
                            rebuild_active_track_ids(app);
                            save_config(app);
                            // Reload track into memory
                            if let Some(new_track) = app.read_track_from_disk(&tid) {
                                if !app.project.tracks.iter().any(|(id, _)| id == &tid) {
                                    app.project.tracks.push((tid.clone(), new_track));
                                } else {
                                    app.replace_track(&tid, new_track);
                                }
                            }
                        } else {
                            // Re-archive
                            if let Some(tc) =
                                app.project.config.tracks.iter_mut().find(|t| t.id == tid)
                            {
                                tc.state = "archived".to_string();
                            }
                            if let Some(file) = app.track_file(&tid).map(|f| f.to_string()) {
                                let _ = crate::ops::track_ops::archive_track_file(
                                    &app.project.frame_dir,
                                    &tid,
                                    &file,
                                );
                            }
                            // Out of `tracks/` is out of the project — see
                            // `confirm_archive_track`. Anything still holding it
                            // in memory writes the file back.
                            app.project.tracks.retain(|(id, _)| id != &tid);
                            rebuild_active_track_ids(app);
                            save_config(app);
                        }
                    }
                    Some(Operation::TrackAdd {
                        track_id: tid,
                        track_name,
                        config_index,
                        prefix,
                    }) => {
                        if is_undo {
                            // Remove the track
                            let file = app.track_file(&tid).map(|f| f.to_string());
                            if let Some(file) = &file {
                                let _ = std::fs::remove_file(app.project.frame_dir.join(file));
                            }
                            app.project.config.tracks.retain(|t| t.id != tid);
                            app.project.config.ids.prefixes.shift_remove(&tid);
                            app.project.tracks.retain(|(id, _)| id != &tid);
                            rebuild_active_track_ids(app);
                            save_config(app);
                        } else {
                            // Re-create the track under the name the user gave it.
                            let name = track_name.clone();
                            let tc = crate::model::TrackConfig {
                                id: tid.clone(),
                                name: name.clone(),
                                state: "active".to_string(),
                                file: format!("tracks/{}.md", tid),
                            };
                            let track_content = format!("# {}\n\n## Backlog\n\n## Done\n", name);
                            let track_path = app.project.frame_dir.join(&tc.file);
                            let _ = crate::io::recovery::atomic_write(
                                &track_path,
                                track_content.as_bytes(),
                            );
                            // Both recorded by the add. Re-deriving the prefix here
                            // would ask the *current* prefix set a question only
                            // the original set could answer, and appending would
                            // put the track somewhere the add never put it.
                            let idx = config_index.min(app.project.config.tracks.len());
                            app.project.config.tracks.insert(idx, tc);
                            app.project.config.ids.prefixes.insert(tid.clone(), prefix);
                            if let Ok(text) = std::fs::read_to_string(&track_path) {
                                let track = crate::parse::parse_track(&text);
                                app.project.tracks.push((tid.clone(), track));
                            }
                            rebuild_active_track_ids(app);
                            save_config(app);
                        }
                    }
                    Some(Operation::TrackDelete {
                        track_id: tid,
                        track_name,
                        old_state,
                        prefix,
                        config_index,
                        prefix_index,
                        content,
                    }) => {
                        if is_undo {
                            // Restore the file as it was. Only when it could not be
                            // read at delete time is there nothing to put back, and
                            // a minimal shell is then the best available answer.
                            let tc = crate::model::TrackConfig {
                                id: tid.clone(),
                                name: track_name.clone(),
                                state: old_state.clone(),
                                file: format!("tracks/{}.md", tid),
                            };
                            let track_content = content.clone().unwrap_or_else(|| {
                                format!("# {}\n\n## Backlog\n\n## Done\n", track_name)
                            });
                            let track_path = app.project.frame_dir.join(&tc.file);
                            let _ = crate::io::recovery::atomic_write(
                                &track_path,
                                track_content.as_bytes(),
                            );
                            // Back where it was, in both lists. Pushing put a
                            // deleted first track back as the last one — the file
                            // was restored intact and the project still looked
                            // rearranged.
                            let idx = config_index.min(app.project.config.tracks.len());
                            app.project.config.tracks.insert(idx, tc);
                            if let Some(p) = &prefix {
                                let pidx = prefix_index
                                    .unwrap_or(usize::MAX)
                                    .min(app.project.config.ids.prefixes.len());
                                app.project.config.ids.prefixes.shift_insert(
                                    pidx,
                                    tid.clone(),
                                    p.clone(),
                                );
                            }
                            if let Ok(text) = std::fs::read_to_string(&track_path) {
                                let track = crate::parse::parse_track(&text);
                                app.project.tracks.push((tid.clone(), track));
                            }
                            rebuild_active_track_ids(app);
                            save_config(app);
                        } else {
                            // Re-delete the track
                            let file = app.track_file(&tid).map(|f| f.to_string());
                            if let Some(file) = &file {
                                let _ = std::fs::remove_file(app.project.frame_dir.join(file));
                            }
                            app.project.config.tracks.retain(|t| t.id != tid);
                            app.project.config.ids.prefixes.shift_remove(&tid);
                            app.project.tracks.retain(|(id, _)| id != &tid);
                            rebuild_active_track_ids(app);
                            save_config(app);
                        }
                    }
                    Some(Operation::TrackUnarchive { track_id: tid }) => {
                        if is_undo {
                            // Re-archive the track
                            if let Some(tc) =
                                app.project.config.tracks.iter_mut().find(|t| t.id == tid)
                            {
                                tc.state = "archived".to_string();
                            }
                            if let Some(file) = app.track_file(&tid).map(|f| f.to_string()) {
                                let _ = crate::ops::track_ops::archive_track_file(
                                    &app.project.frame_dir,
                                    &tid,
                                    &file,
                                );
                            }
                            // Out of `tracks/` is out of the project — see
                            // `confirm_archive_track`. Anything still holding it
                            // in memory writes the file back.
                            app.project.tracks.retain(|(id, _)| id != &tid);
                            rebuild_active_track_ids(app);
                            save_config(app);
                        } else {
                            // Re-unarchive the track
                            if let Some(tc) =
                                app.project.config.tracks.iter_mut().find(|t| t.id == tid)
                            {
                                tc.state = "active".to_string();
                            }
                            if let Some(file) = app.track_file(&tid).map(|f| f.to_string()) {
                                let _ = crate::ops::track_ops::restore_track_file(
                                    &app.project.frame_dir,
                                    &tid,
                                    &file,
                                );
                            }
                            if let Some(new_track) = app.read_track_from_disk(&tid) {
                                if !app.project.tracks.iter().any(|(id, _)| id == &tid) {
                                    app.project.tracks.push((tid.clone(), new_track));
                                } else {
                                    app.replace_track(&tid, new_track);
                                }
                            }
                            rebuild_active_track_ids(app);
                            save_config(app);
                        }
                    }
                    _ => {}
                }
            });
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
            // One lock across both writes — undoing a triage restores the
            // inbox item and removes the task, and a writer slipping between
            // the two would see the item in both places.
            match triage_track_id {
                Some(tid) => app.save_batch_logged(&[&tid], true),
                None => app.save_inbox_logged(),
            }
        }
        UndoNavTarget::Recent { .. } => {
            // Reopen or SectionMove undo/redo: save the affected track
            let affected_track_id = {
                let op = if is_undo {
                    app.undo_stack.peek_last_redo()
                } else {
                    app.undo_stack.peek_last_undo()
                };
                match op {
                    Some(Operation::Reopen { track_id, .. }) => Some(track_id.clone()),
                    Some(Operation::SectionMove { track_id, .. }) => Some(track_id.clone()),
                    _ => None,
                }
            };
            if let Some(tid) = affected_track_id {
                app.save_track_logged(&tid);
            }
        }
    }
}

/// Navigate the UI to show the item affected by an undo/redo operation.
pub(super) fn navigate_to_undo_target(app: &mut App, nav: &UndoNavTarget) {
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
                // Expand ancestors so the task is visible (e.g. subtask under collapsed parent)
                auto_expand_for_task(app, track_id, task_id);
                // Move cursor to the affected task and flash it
                move_cursor_to_task(app, track_id, task_id);
                app.flash_task(task_id);

                // If the operation targets a detail region, navigate to it
                // and flash the specific region instead of the header
                if let Some(region) = detail_region {
                    app.flash_detail_region = Some(*region);
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

            app.tracks_name_col_min = 0;
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
// Repeat last action (.)

#[derive(Clone, Copy)]
pub(super) enum StateAction {
    Cycle,
    Done,
    SetTodo,
    ToggleBlocked,
    ToggleParked,
}

/// Schedule the section move a state change calls for, and report the section
/// the task sits in at top level (`None` for a subtask, which has no section of
/// its own and stays put).
///
/// The destination is **computed** — `task_ops::canonical_section` — rather than
/// chosen from a list of transitions. Every version of this that enumerated
/// `from → to` pairs was missing one: a Parked task marked done went to the
/// Backlog (`12c0b57`), a Done task parked stayed in Done (`5eb069f`), and a Done
/// task reopened outside the Board and Recent views stayed in Done. Asking where
/// the new state belongs has no cases to leave out.
///
/// `push_undo` is the caller's to decide, because it depends on what that caller
/// has already recorded — see [`PendingMove::push_undo`]. Every path through
/// [`task_state_action`] pushes an `Operation::StateChange`, which restores state
/// but *not* section, so those all need one.
pub(super) fn schedule_section_move(
    app: &mut App,
    track_id: &str,
    task_id: &str,
    new_state: crate::model::task::TaskState,
    old_state: crate::model::task::TaskState,
    push_undo: bool,
) -> Option<SectionKind> {
    let track = App::find_track_in_project(&app.project, track_id)?;
    let from = task_ops::top_level_section(track, task_id)?;
    let to = task_ops::canonical_section(new_state);
    if from != to {
        app.pending_moves.push(PendingMove {
            from,
            to,
            push_undo,
            track_id: track_id.to_string(),
            task_id: task_id.to_string(),
            deadline: std::time::Instant::now() + std::time::Duration::from_secs(5),
            old_state: Some(old_state),
        });
    }
    Some(from)
}

/// Apply a state change to the task under the cursor.
pub(super) fn task_state_action(app: &mut App, action: StateAction) {
    let (track_id, task_id) = if let View::Detail { track_id, task_id } = &app.view {
        let subtask_id = app.detail_state.as_ref().and_then(|ds| {
            if ds.region == DetailRegion::Subtasks {
                ds.flat_subtask_ids.get(ds.subtask_cursor).cloned()
            } else {
                None
            }
        });
        (
            track_id.clone(),
            subtask_id.unwrap_or_else(|| task_id.clone()),
        )
    } else if matches!(app.view, View::Board) {
        match app.board_cursor_task_id() {
            Some((tid, task_id)) => (tid, task_id),
            None => return,
        }
    } else if let Some((track_id, task_id, _section)) = app.cursor_task_id() {
        (track_id, task_id)
    } else {
        return;
    };

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
    let old_resolved = task.metadata.iter().find_map(|m| {
        if let Metadata::Resolved(d) = m {
            Some(d.clone())
        } else {
            None
        }
    });

    match action {
        StateAction::Cycle => task_ops::cycle_state(task),
        StateAction::Done => task_ops::set_done(task),
        StateAction::SetTodo => task_ops::set_state(task, crate::model::task::TaskState::Todo),
        StateAction::ToggleBlocked => task_ops::set_blocked(task),
        StateAction::ToggleParked => task_ops::set_parked(task),
    }

    let new_state = task.state;
    let new_resolved = task.metadata.iter().find_map(|m| {
        if let Metadata::Resolved(d) = m {
            Some(d.clone())
        } else {
            None
        }
    });

    // Only push undo if state actually changed
    if old_state != new_state {
        // Flash the task with state-specific color
        app.flash_state = Some(new_state);
        app.flash_task(&task_id);

        // Any move scheduled by an earlier state change on this task is stale.
        // Cancelling here, before the new one is scheduled below, is what lets
        // this be unconditional — the old code had to carve out the move it had
        // just created two branches earlier.
        app.cancel_pending_move(&track_id, &task_id);
        if old_state == crate::model::task::TaskState::Done
            || old_state == crate::model::task::TaskState::Parked
        {
            app.cancel_pending_subtask_hide(&track_id, &task_id);
        }

        // Board: pin the task to its current column for the grace period so it
        // does not jump the instant the key is pressed. A task on its way out of
        // Done needs no pin — it is still in the Done section, and so still in
        // the Done column, until the move fires.
        if matches!(app.view, View::Board)
            && !(old_state == crate::model::task::TaskState::Done
                && new_state != crate::model::task::TaskState::Done)
        {
            app.board_state
                .column_pins
                .retain(|p| !(p.track_id == track_id && p.task_id == task_id));
            app.board_state
                .column_pins
                .push(crate::tui::app::BoardColumnPin {
                    track_id: track_id.clone(),
                    task_id: task_id.clone(),
                    pinned_state: old_state,
                    deadline: std::time::Instant::now() + std::time::Duration::from_secs(5),
                });
        }

        app.undo_stack.push(Operation::StateChange {
            track_id: track_id.clone(),
            task_id: task_id.clone(),
            old_state,
            new_state,
            old_resolved: old_resolved.clone(),
            new_resolved,
        });

        // Record repeatable action
        app.last_action = Some(match action {
            StateAction::Cycle => RepeatableAction::CycleState,
            StateAction::Done => RepeatableAction::SetState(crate::model::TaskState::Done),
            StateAction::SetTodo => RepeatableAction::SetState(crate::model::TaskState::Todo),
            StateAction::ToggleBlocked => {
                RepeatableAction::SetState(crate::model::TaskState::Blocked)
            }
            StateAction::ToggleParked => {
                RepeatableAction::SetState(crate::model::TaskState::Parked)
            }
        });

        // The section move — one call, every direction. `push_undo` is true
        // because the `StateChange` pushed above restores state and the resolved
        // date but leaves the task where it sits; without an entry of its own,
        // undoing an unpark left a `[~]` task in the Backlog.
        //
        // A subtask has no section of its own, so it gets a grace period before
        // being hidden instead.
        if schedule_section_move(app, &track_id, &task_id, new_state, old_state, true).is_none()
            && new_state == crate::model::task::TaskState::Done
        {
            app.pending_subtask_hides
                .push(crate::tui::app::PendingSubtaskHide {
                    track_id: track_id.clone(),
                    task_id: task_id.clone(),
                    deadline: std::time::Instant::now() + std::time::Duration::from_secs(5),
                });
        }

        // Leaving Done: `set_state` has already stripped the resolved date, but
        // while a move out of the Done section is pending the task is still
        // *in* that section, so the Done column and the Recent view still show
        // it — and both sort on that date. Put it back for the grace period;
        // the flush that performs the move strips it again.
        //
        // Which is why this asks for the pending move rather than for the old
        // state. Mark a Backlog task done and change your mind inside the grace
        // period and there is no move out of Done to schedule — the move *into*
        // Done never fired — so nothing would ever strip the date again, and the
        // task kept a `resolved:` it had never earned. Undo could not take it
        // back either: the recorded `new_resolved` was captured before the date
        // was restored, so the operation and the file disagreed about what the
        // keystroke had done. That disagreement is what
        // `tests/undo_properties.rs` reported.
        let move_out_of_done_pending = app
            .pending_moves
            .iter()
            .any(|pm| pm.track_id == track_id && pm.task_id == task_id && pm.leaves_done());
        if move_out_of_done_pending && let Some(ref resolved) = old_resolved {
            let track = app.find_track_mut(&track_id).expect("track exists");
            let task = task_ops::find_task_mut_in_track(track, &task_id).expect("task exists");
            task.metadata.push(Metadata::Resolved(resolved.clone()));
            task.mark_dirty();
        }
    }

    app.save_track_logged(&track_id);
}

// ---------------------------------------------------------------------------
// CC tag / CC focus

/// Save the project config to project.toml.
/// Write `project.toml` under the project lock, recording a failure rather
/// than discarding it.
///
/// It used to write straight through with no lock and `let _ =` on the result,
/// which is both halves of the same problem: another `fr` could be midway
/// through a read-modify-write of the project, and a write that did not happen
/// left memory and disk disagreeing with nothing recorded anywhere. Inside
/// [`App::with_project_lock`] this writes under the lock already held.
pub(super) fn save_config(app: &mut App) {
    app.save_config_logged();
}

// ---------------------------------------------------------------------------
// Add task

/// Move the cursor to a specific task by ID in a track view.
pub(super) fn move_cursor_to_task(app: &mut App, track_id: &str, target_task_id: &str) {
    let flat_items = app.build_flat_items(track_id);
    let track = App::find_track_in_project(&app.project, track_id);
    if let Some(track) = track {
        for (i, item) in flat_items.iter().enumerate() {
            if let FlatItem::Task { section, path, .. } = item
                && let Some(task) = resolve_task_from_flat(track, *section, path)
                && task.id.as_deref() == Some(target_task_id)
            {
                let state = app.get_track_state(track_id);
                state.cursor = i;
                return;
            }
        }
    }
}

/// Remove a task by ID from a specific section (hard remove, not mark-done).
pub(super) fn remove_task_from_section(
    track: &mut crate::model::Track,
    task_id: &str,
    section: SectionKind,
) {
    if let Some(tasks) = track.section_tasks_mut(section) {
        tasks.retain(|t| t.id.as_deref() != Some(task_id));
    }
}

pub(super) use crate::tui::fields::dedup_preserve_order;

pub(super) fn word_boundary_left(s: &str, pos: usize) -> usize {
    unicode::word_boundary_left(s, pos)
}

pub(super) fn word_boundary_right(s: &str, pos: usize) -> usize {
    unicode::word_boundary_right(s, pos)
}

/// Find the line index of the previous paragraph boundary (blank line or start of buffer).
/// Lands on the blank line above the preceding paragraph, mirroring next_paragraph_line.
pub(super) fn prev_paragraph_line(lines: &[&str], current: usize) -> usize {
    if current == 0 {
        return 0;
    }
    let mut i = current;
    // If on a blank line, skip consecutive blank lines upward
    while i > 0 && lines[i].trim().is_empty() {
        i -= 1;
    }
    // Move up through non-blank lines (the paragraph body)
    while i > 0 && !lines[i].trim().is_empty() {
        i -= 1;
    }
    // i is now on the blank line above the paragraph, or 0
    i
}

/// Find the line index of the next paragraph boundary (blank line or end of buffer).
/// Moves down past the current paragraph, then to the start of the next one.
pub(super) fn next_paragraph_line(lines: &[&str], current: usize) -> usize {
    let last = lines.len().saturating_sub(1);
    if current >= last {
        return last;
    }
    let mut i = current;
    // If we're on a blank line, skip over consecutive blank lines first
    while i < last && lines[i].trim().is_empty() {
        i += 1;
    }
    // Move down through non-blank lines until we hit a blank line or end
    while i < last && !lines[i + 1].trim().is_empty() {
        i += 1;
    }
    // Move to the blank line / next paragraph start
    if i < last {
        i += 1;
    }
    i
}

// ---------------------------------------------------------------------------
// MOVE mode

/// Move cursor by delta in the current view
pub(super) fn move_cursor(app: &mut App, delta: i32) {
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

            // Skip non-selectable items (separators and context rows)
            let new_cursor = new_cursor as usize;
            let new_cursor = skip_non_selectable(&flat_items, new_cursor, delta);

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
        View::Search => {
            if let Some(ref mut sr) = app.project_search_results {
                let count = sr.items.len();
                if count == 0 {
                    return;
                }
                let mut new_cursor = sr.cursor as i32 + delta;
                new_cursor = new_cursor.clamp(0, count as i32 - 1);
                sr.cursor = new_cursor as usize;
            }
        }
        View::Board => {
            board_move_cursor(app, delta);
        }
    }
}

/// Move cursor within the focused board column, skipping TrackHeader items.
pub(super) fn board_move_cursor(app: &mut App, delta: i32) {
    use crate::tui::app::BoardItem;
    let col = app.board_state.focus_column.index();
    let columns = app.build_board_columns();
    let column = &columns[col];
    if column.is_empty() {
        return;
    }
    let count = column.len();
    let mut pos = app.board_state.cursor[col] as i32 + delta;
    pos = pos.clamp(0, count as i32 - 1);
    let mut pos = pos as usize;

    // Skip non-selectable headers
    if delta > 0 {
        while pos < count && matches!(column[pos], BoardItem::TrackHeader { .. }) {
            pos += 1;
        }
        if pos >= count {
            // Find the last selectable item
            pos = count.saturating_sub(1);
            while pos > 0 && matches!(column[pos], BoardItem::TrackHeader { .. }) {
                pos -= 1;
            }
        }
    } else if delta < 0 {
        while pos > 0 && matches!(column[pos], BoardItem::TrackHeader { .. }) {
            pos -= 1;
        }
        // If landed on header at position 0, try moving forward
        if matches!(column[pos], BoardItem::TrackHeader { .. }) {
            pos += 1;
            while pos < count && matches!(column[pos], BoardItem::TrackHeader { .. }) {
                pos += 1;
            }
            if pos >= count {
                pos = 0; // give up, everything is headers
            }
        }
    }

    app.board_state.cursor[col] = pos;
}

/// Move cursor to the next/previous top-level task (depth 0) in the current view.
/// In track view, this skips over subtasks to jump between top-level items.
/// In other views, falls back to regular single-step movement.
pub(super) fn move_paragraph(app: &mut App, direction: i32) {
    match &app.view {
        View::Track(idx) => {
            let track_id = match app.active_track_ids.get(*idx) {
                Some(id) => id.clone(),
                None => return,
            };
            let flat_items = app.build_flat_items(&track_id);
            if flat_items.is_empty() {
                return;
            }

            let state = app.get_track_state(&track_id);
            let cursor = state.cursor;

            let target = if direction > 0 {
                // Search forward for next top-level task after current position
                flat_items
                    .iter()
                    .enumerate()
                    .skip(cursor + 1)
                    .find(|(_, item)| is_paragraph_boundary(item))
                    .map(|(i, _)| i)
            } else {
                // Search backward for previous top-level task before current position
                flat_items[..cursor]
                    .iter()
                    .enumerate()
                    .rev()
                    .find(|(_, item)| is_paragraph_boundary(item))
                    .map(|(i, _)| i)
            };

            if let Some(target) = target {
                let state = app.get_track_state(&track_id);
                state.cursor = target;
            }
        }
        View::Detail { .. } => {
            // Alt+Up/Down in detail view: skip empty regions
            let ds = match &app.detail_state {
                Some(ds) => ds,
                None => return,
            };
            if ds.regions.is_empty() {
                return;
            }
            let current_idx = ds.regions.iter().position(|r| *r == ds.region).unwrap_or(0);
            let populated = &ds.regions_populated;

            let target_idx = if direction > 0 {
                (current_idx + 1..ds.regions.len())
                    .find(|&i| populated.get(i).copied().unwrap_or(false))
            } else {
                (0..current_idx)
                    .rev()
                    .find(|&i| populated.get(i).copied().unwrap_or(false))
            };

            if let Some(idx) = target_idx {
                let region = ds.regions[idx];
                let ds = app.detail_state.as_mut().unwrap();
                // Reset note view line when leaving Note region
                if ds.region == DetailRegion::Note {
                    ds.note_view_line = None;
                }
                ds.region = region;
                // Reset subtask cursor when entering Subtasks from above
                if ds.region == DetailRegion::Subtasks && direction > 0 {
                    ds.subtask_cursor = 0;
                }
            }
        }
        View::Search => {
            // Jump between group headers in search results
            if let Some(ref mut sr) = app.project_search_results {
                if sr.groups.is_empty() || sr.items.is_empty() {
                    return;
                }
                // Find which group the cursor is currently in
                let cur_group = sr
                    .groups
                    .iter()
                    .enumerate()
                    .rev()
                    .find(|(_, (start, _, _))| sr.cursor >= *start)
                    .map(|(i, _)| i)
                    .unwrap_or(0);
                let target = if direction > 0 {
                    // Next group
                    if cur_group + 1 < sr.groups.len() {
                        Some(sr.groups[cur_group + 1].0)
                    } else {
                        None
                    }
                } else {
                    // Previous group: jump to start of current if not at start, else previous
                    let cur_start = sr.groups[cur_group].0;
                    if sr.cursor > cur_start {
                        Some(cur_start)
                    } else if cur_group > 0 {
                        Some(sr.groups[cur_group - 1].0)
                    } else {
                        None
                    }
                };
                if let Some(t) = target {
                    sr.cursor = t;
                }
            }
        }
        _ => {
            // Other views have no nesting — fall back to regular movement
            move_cursor(app, direction);
        }
    }
}

/// A "paragraph boundary" is a selectable top-level task (depth 0) or a section separator.
pub(super) fn is_paragraph_boundary(item: &FlatItem) -> bool {
    match item {
        FlatItem::Task {
            depth, is_context, ..
        } => *depth == 0 && !*is_context,
        FlatItem::ParkedSeparator => false,
        FlatItem::BulkMoveStandin { .. } => false,
        FlatItem::DoneSummary { .. } => false,
    }
}

/// Check if a flat item is non-selectable (separator, context row, or done summary)
pub(super) fn is_non_selectable(item: &FlatItem) -> bool {
    match item {
        FlatItem::ParkedSeparator => true,
        FlatItem::Task { is_context, .. } => *is_context,
        FlatItem::BulkMoveStandin { .. } => false,
        FlatItem::DoneSummary { .. } => true,
    }
}

/// Skip over non-selectable items (separators and context rows) when navigating
pub(super) fn skip_non_selectable(items: &[FlatItem], cursor: usize, direction: i32) -> usize {
    if cursor >= items.len() {
        return cursor;
    }
    if is_non_selectable(&items[cursor]) {
        // Try moving in the requested direction
        let mut pos = cursor;
        while pos < items.len() && is_non_selectable(&items[pos]) {
            let next = (pos as i32 + direction).clamp(0, items.len() as i32 - 1) as usize;
            if next == pos {
                break;
            }
            pos = next;
        }
        if pos < items.len() && !is_non_selectable(&items[pos]) {
            return pos;
        }
        // If still stuck, try the other direction from original cursor
        let mut pos = cursor;
        while pos < items.len() && is_non_selectable(&items[pos]) {
            let next = (pos as i32 - direction).clamp(0, items.len() as i32 - 1) as usize;
            if next == pos {
                break;
            }
            pos = next;
        }
        if pos < items.len() && !is_non_selectable(&items[pos]) {
            return pos;
        }
    }
    cursor
}

pub(super) fn jump_to_top(app: &mut App) {
    match &app.view {
        View::Track(idx) => {
            let track_id = match app.active_track_ids.get(*idx) {
                Some(id) => id.clone(),
                None => return,
            };
            let flat_items = app.build_flat_items(&track_id);
            let first = flat_items
                .iter()
                .position(|item| !is_non_selectable(item))
                .unwrap_or(0);
            let state = app.get_track_state(&track_id);
            state.cursor = first;
            state.scroll_offset = 0;
        }
        View::Detail { .. } => {
            if let Some(ds) = &mut app.detail_state {
                ds.region = ds.regions.first().copied().unwrap_or(DetailRegion::Title);
                ds.scroll_offset = 0;
                ds.note_view_line = None;
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
        View::Search => {
            if let Some(ref mut sr) = app.project_search_results {
                sr.cursor = 0;
                sr.scroll_offset = 0;
            }
        }
        View::Board => {
            board_jump_top(app);
        }
    }
}

/// Jump to the first selectable item in the focused board column
pub(super) fn board_jump_top(app: &mut App) {
    use crate::tui::app::BoardItem;
    let col = app.board_state.focus_column.index();
    let columns = app.build_board_columns();
    let column = &columns[col];
    let first = column
        .iter()
        .position(|item| matches!(item, BoardItem::Task { .. }))
        .unwrap_or(0);
    app.board_state.cursor[col] = first;
    app.board_state.scroll[col] = 0;
}

pub(super) fn jump_to_bottom(app: &mut App) {
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
            // Skip non-selectable items at end
            target = skip_non_selectable(&flat_items, target, -1);
            let state = app.get_track_state(&track_id);
            state.cursor = target;
        }
        View::Detail { .. } => {
            if let Some(ds) = &mut app.detail_state {
                let has_subtasks = ds.regions.contains(&DetailRegion::Subtasks);
                let has_note = ds.regions.contains(&DetailRegion::Note);
                if has_subtasks {
                    // Jump to first subtask (last region on screen)
                    ds.region = DetailRegion::Subtasks;
                    ds.note_view_line = None;
                    ds.subtask_cursor = 0;
                } else if has_note && ds.total_lines > 0 {
                    // Jump to end of note content
                    ds.region = DetailRegion::Note;
                    ds.note_view_line = Some(ds.note_content_end);
                } else {
                    ds.region = ds.regions.last().copied().unwrap_or(DetailRegion::Title);
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
        View::Search => {
            if let Some(ref mut sr) = app.project_search_results
                && !sr.items.is_empty()
            {
                sr.cursor = sr.items.len() - 1;
            }
        }
        View::Board => {
            board_jump_bottom(app);
        }
    }
}

/// Jump to the last selectable item in the focused board column
pub(super) fn board_jump_bottom(app: &mut App) {
    use crate::tui::app::BoardItem;
    let col = app.board_state.focus_column.index();
    let columns = app.build_board_columns();
    let column = &columns[col];
    if column.is_empty() {
        return;
    }
    let last = column
        .iter()
        .rposition(|item| matches!(item, BoardItem::Task { .. }))
        .unwrap_or(0);
    app.board_state.cursor[col] = last;
}

/// Rebuild active_track_ids from config and clamp tracks_cursor.
///
/// The work lives on `App` because adopting an externally-changed config needs
/// it too, and that happens in `app.rs` well below this module.
pub(super) fn rebuild_active_track_ids(app: &mut App) {
    app.rebuild_active_track_ids();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::app::app_on_disk;

    fn track_text(app: &App) -> String {
        std::fs::read_to_string(app.project.frame_dir.join("tracks/a.md")).unwrap()
    }

    /// Marking a Backlog task done and changing your mind inside the grace
    /// period must not leave a resolution date behind.
    ///
    /// Leaving Done puts the date back so the row holds its place in the Done
    /// column while the move out of Done is pending, and the flush that
    /// performs that move strips it again. But the move *into* Done had not
    /// fired yet, so there was no move out of Done to schedule — and nothing
    /// left to strip the date. The task sat in the Backlog, state todo, wearing
    /// a `resolved:` it had never earned, and undo could not take it off: the
    /// recorded `new_resolved` was captured before the date was restored, so the
    /// operation and the file disagreed about what the keystroke had done.
    #[test]
    fn changing_your_mind_about_done_leaves_no_resolution_date() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut app = app_on_disk(tmp.path());
        assert!(app.jump_to_task("A-001"));

        task_state_action(&mut app, StateAction::Done);
        task_state_action(&mut app, StateAction::SetTodo);

        let text = track_text(&app);
        assert!(
            !text.contains("resolved:"),
            "a todo task carries no resolution date: {text:?}"
        );
    }

    /// A task that really is in the Done section keeps the date for the grace
    /// period — the Done column and the Recent view both sort on it, and the
    /// row is still there until the move fires.
    #[test]
    fn a_task_on_its_way_out_of_done_keeps_its_date_until_the_move_fires() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut app = app_on_disk(tmp.path());
        assert!(app.jump_to_task("A-001"));

        task_state_action(&mut app, StateAction::Done);
        for track_id in app.flush_all_pending_moves() {
            app.save_track_logged(&track_id);
        }
        assert!(track_text(&app).contains("resolved:"), "it is done");

        // The track view does not list Done tasks, so a user reaches this one
        // from the detail view — as `tests/parity.rs` does for the same reason.
        app.view = View::Detail {
            track_id: "a".into(),
            task_id: "A-001".into(),
        };
        task_state_action(&mut app, StateAction::SetTodo);
        assert!(
            track_text(&app).contains("resolved:"),
            "and still shows as done until the move out of Done fires"
        );

        for track_id in app.flush_all_pending_moves() {
            app.save_track_logged(&track_id);
        }
        assert!(
            !track_text(&app).contains("resolved:"),
            "which is what strips it"
        );
    }

    /// Redoing a triage navigates to the new task, so it lands in the `Task`
    /// arm of `apply_nav_side_effects` rather than the `Inbox` one — and that
    /// arm saved only the track. The item was removed from the inbox in memory
    /// and left in `inbox.md` on disk, so the same note existed twice: once as
    /// an inbox item and once as the task that replaced it.
    #[test]
    fn redoing_a_triage_takes_the_item_out_of_the_inbox_file() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut app = app_on_disk(tmp.path());
        let inbox_path = app.project.frame_dir.join("inbox.md");
        std::fs::write(&inbox_path, "# Inbox\n\n- Look into the parser\n").unwrap();
        app.project.inbox =
            Some(crate::parse::parse_inbox("# Inbox\n\n- Look into the parser\n").0);
        // Memory and the file were both set by hand above, so the session's
        // ancestor has to be moved with them. A save compares the file against
        // this to decide whether another process has written; left pointing at
        // the inbox `app_on_disk` built, it would read the item below as a
        // concurrent capture and merge it back in, and this test would fail for
        // a reason with nothing to do with redo.
        app.baselines.insert(
            crate::tui::app::SaveTarget::Inbox,
            "# Inbox\n\n- Look into the parser\n".to_string(),
        );

        // The state a triage leaves behind: item gone, task in the track.
        let item = app.project.inbox.as_mut().unwrap().items.remove(0);
        let track = app.find_track_mut("a").unwrap();
        let task = crate::model::task::Task::new(
            crate::model::task::TaskState::Todo,
            Some("A-002".into()),
            item.title.clone(),
        );
        track
            .section_tasks_mut(SectionKind::Backlog)
            .unwrap()
            .push(task);
        app.undo_stack.push(Operation::InboxTriage {
            inbox_index: 0,
            item,
            track_id: "a".into(),
            task_id: "A-002".into(),
        });
        app.save_batch_logged(&["a"], true);

        perform_undo(&mut app);
        assert!(
            std::fs::read_to_string(&inbox_path)
                .unwrap()
                .contains("Look into the parser"),
            "undo puts the item back"
        );

        perform_redo(&mut app);
        eprintln!("UNSAVED: {:?}", app.unsaved.keys().collect::<Vec<_>>());
        let on_disk = std::fs::read_to_string(&inbox_path).unwrap();
        assert!(
            !on_disk.contains("Look into the parser"),
            "and redo takes it away again, on disk and not only in memory: {on_disk:?}"
        );
    }
}
