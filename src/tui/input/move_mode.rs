use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::collections::HashSet;

use crate::model::SectionKind;
use crate::model::task::Task;
use crate::model::track::Track;
use crate::ops::task_ops::{self, InsertPosition};

use crate::tui::app::{App, DetailRegion, Mode, MoveState, View};
use crate::tui::undo::Operation;

use super::*;

/// Enter MOVE mode for the task under the cursor (track view only).
pub(super) fn enter_move_mode(app: &mut App) {
    match &app.view {
        View::Track(_) => {
            if let Some((track_id, task_id, section)) = app.cursor_task_id() {
                // Only allow moving backlog tasks
                if section != SectionKind::Backlog {
                    return;
                }
                let track = match App::find_track_in_project(&app.project, &track_id) {
                    Some(t) => t,
                    None => return,
                };

                // Find the task's location (supports any depth)
                let location =
                    match task_ops::find_task_location(track, &task_id, SectionKind::Backlog) {
                        Some(loc) => loc,
                        None => return,
                    };
                let task = match task_ops::find_task_in_track(track, &task_id) {
                    Some(t) => t,
                    None => return,
                };
                let original_depth = task.depth;

                app.move_state = Some(MoveState::Task {
                    track_id,
                    task_id,
                    original_parent_id: location.parent_id,
                    original_section: SectionKind::Backlog,
                    original_sibling_index: location.sibling_index,
                    original_depth,
                    force_expanded: HashSet::new(),
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

pub(super) fn handle_move(app: &mut App, key: KeyEvent) {
    let is_track_move = matches!(&app.move_state, Some(MoveState::Track { .. }));
    let is_inbox_move = matches!(&app.move_state, Some(MoveState::InboxItem { .. }));
    let is_bulk_move = matches!(&app.move_state, Some(MoveState::BulkTask { .. }));

    match (key.modifiers, key.code) {
        // Confirm
        (_, KeyCode::Enter) | (_, KeyCode::Char('m')) => {
            if let Some(ms) = app.move_state.take() {
                match ms {
                    MoveState::Task {
                        track_id,
                        task_id,
                        original_parent_id,
                        original_section: _,
                        original_sibling_index,
                        original_depth,
                        force_expanded: _,
                    } => {
                        // Keep force-expanded ancestors open so the user can
                        // see where the task landed after confirming.
                        // Determine current location
                        let track = App::find_track_in_project(&app.project, &track_id);
                        let current_location = track.and_then(|t| {
                            task_ops::find_task_location(t, &task_id, SectionKind::Backlog)
                        });

                        if let Some(cur_loc) = current_location {
                            let parent_changed = cur_loc.parent_id != original_parent_id;
                            let task =
                                track.and_then(|t| task_ops::find_task_in_track(t, &task_id));
                            let depth_changed = task.is_some_and(|t| t.depth != original_depth);

                            if parent_changed || depth_changed {
                                // Reparent occurred — rekey IDs
                                let prefix = app
                                    .project
                                    .config
                                    .ids
                                    .prefixes
                                    .get(&track_id)
                                    .cloned()
                                    .unwrap_or_default();

                                let track_mut = app.find_track_mut(&track_id);
                                if let Some(track_mut) = track_mut {
                                    // Get task to compute new ID
                                    let new_id = match &cur_loc.parent_id {
                                        None => {
                                            let next = task_ops::next_id_number(track_mut, &prefix);
                                            format!("{}-{:03}", prefix, next)
                                        }
                                        Some(pid) => {
                                            let parent =
                                                task_ops::find_task_in_track(track_mut, pid);
                                            let child_num = parent.map_or(1, |p| p.subtasks.len());
                                            // The task is already inserted, so the current count includes it
                                            // We need to find its current position to get the right number
                                            format!("{}.{}", pid, child_num)
                                        }
                                    };

                                    // Rekey the subtree
                                    let task_ref =
                                        task_ops::find_task_mut_in_track(track_mut, &task_id);
                                    if let Some(task_ref) = task_ref {
                                        let id_mappings =
                                            task_ops::rekey_subtree(task_ref, &new_id);

                                        // Update dep references across all tracks
                                        for (old_id, new_mapped_id) in &id_mappings {
                                            task_ops::update_dep_references(
                                                &mut app.project.tracks,
                                                old_id,
                                                new_mapped_id,
                                            );
                                        }

                                        let _ = app.save_track(&track_id);
                                        // Save other tracks that may have had deps updated
                                        if !id_mappings.is_empty() {
                                            let other_track_ids: Vec<String> = app
                                                .project
                                                .tracks
                                                .iter()
                                                .filter(|(tid, _)| tid != &track_id)
                                                .map(|(tid, _)| tid.clone())
                                                .collect();
                                            for tid in &other_track_ids {
                                                let _ = app.save_track(tid);
                                            }
                                        }

                                        // Preserve expand/collapse state through ID rekeying
                                        let state = app.get_track_state(&track_id);
                                        for (old_id, new_mapped_id) in &id_mappings {
                                            if state.expanded.remove(old_id) {
                                                state.expanded.insert(new_mapped_id.clone());
                                            }
                                        }

                                        app.undo_stack.push(Operation::Reparent {
                                            track_id: track_id.clone(),
                                            new_task_id: new_id.clone(),
                                            old_parent_id: original_parent_id,
                                            new_parent_id: cur_loc.parent_id,
                                            old_sibling_index: original_sibling_index,
                                            new_sibling_index: cur_loc.sibling_index,
                                            old_depth: original_depth,
                                            id_mappings,
                                        });

                                        // Navigate to the new task ID
                                        move_cursor_to_task(app, &track_id, &new_id);
                                    }
                                }
                            } else {
                                // Same parent — position-only move
                                if cur_loc.sibling_index != original_sibling_index {
                                    app.undo_stack.push(Operation::TaskMove {
                                        track_id,
                                        task_id,
                                        parent_id: original_parent_id,
                                        old_index: original_sibling_index,
                                        new_index: cur_loc.sibling_index,
                                    });
                                }
                            }
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
                    MoveState::Track {
                        track_id,
                        original_index,
                    } => {
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
                    MoveState::BulkTask {
                        track_id,
                        removed_tasks,
                        insert_pos,
                    } => {
                        // Build undo ops from original positions before reinserting
                        let count = removed_tasks.len();
                        let mut ops: Vec<Operation> = Vec::new();
                        for (orig_idx, task) in &removed_tasks {
                            if let Some(id) = &task.id {
                                ops.push(Operation::TaskMove {
                                    track_id: track_id.clone(),
                                    task_id: id.clone(),
                                    parent_id: None,
                                    old_index: *orig_idx,
                                    new_index: insert_pos,
                                });
                            }
                        }
                        // Insert tasks at the current position
                        let track = app.find_track_mut(&track_id).unwrap();
                        let backlog = track.section_tasks_mut(SectionKind::Backlog).unwrap();
                        for (i, (_, task)) in removed_tasks.into_iter().enumerate() {
                            let idx = (insert_pos + i).min(backlog.len());
                            backlog.insert(idx, task);
                        }
                        let _ = app.save_track(&track_id);
                        if !ops.is_empty() {
                            app.undo_stack.push(Operation::Bulk(ops));
                        }
                        app.status_message = Some(format!("{} tasks moved", count));
                    }
                }
            }
            app.mode = if app.selection.is_empty() {
                Mode::Navigate
            } else {
                Mode::Select
            };
        }
        // Cancel: restore original position
        (_, KeyCode::Esc) => {
            if let Some(ms) = app.move_state.take() {
                match ms {
                    MoveState::Task {
                        track_id,
                        task_id,
                        original_parent_id,
                        original_section,
                        original_sibling_index,
                        original_depth,
                        force_expanded,
                    } => {
                        // Restore any force-expanded ancestors
                        restore_force_expanded(app, &track_id, &force_expanded);
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
                            // Remove from current position and restore to original
                            if let Some((mut task, _)) =
                                task_ops::remove_task_subtree(track, &task_id)
                            {
                                task_ops::set_subtree_depth(&mut task, original_depth);
                                let _ = task_ops::insert_task_subtree(
                                    track,
                                    task,
                                    original_parent_id.as_deref(),
                                    original_section,
                                    original_sibling_index,
                                );
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
                    MoveState::BulkTask {
                        track_id,
                        removed_tasks,
                        ..
                    } => {
                        // Restore tasks to original positions
                        let track = app.find_track_mut(&track_id).unwrap();
                        let backlog = track.section_tasks_mut(SectionKind::Backlog).unwrap();
                        for (orig_idx, task) in removed_tasks.into_iter() {
                            let idx = orig_idx.min(backlog.len());
                            backlog.insert(idx, task);
                        }
                        let _ = app.save_track(&track_id);
                    }
                }
            }
            app.mode = if app.selection.is_empty() {
                Mode::Navigate
            } else {
                Mode::Select
            };
        }
        // Move up
        (KeyModifiers::NONE, KeyCode::Up | KeyCode::Char('k')) => {
            if is_bulk_move {
                move_bulk_standin(app, -1);
            } else if is_inbox_move {
                move_inbox_item(app, -1);
            } else if is_track_move {
                move_track_in_list(app, -1);
            } else {
                move_task_in_list(app, -1);
            }
        }
        // Move down
        (KeyModifiers::NONE, KeyCode::Down | KeyCode::Char('j')) => {
            if is_bulk_move {
                move_bulk_standin(app, 1);
            } else if is_inbox_move {
                move_inbox_item(app, 1);
            } else if is_track_move {
                move_track_in_list(app, 1);
            } else {
                move_task_in_list(app, 1);
            }
        }
        // Move to top
        (KeyModifiers::NONE, KeyCode::Char('g')) => {
            if is_bulk_move {
                move_bulk_standin_to_boundary(app, true);
            } else if is_inbox_move {
                move_inbox_to_boundary(app, true);
            } else if is_track_move {
                move_track_to_boundary(app, true);
            } else {
                move_task_to_boundary(app, true);
            }
        }
        (m, KeyCode::Up) if m.contains(KeyModifiers::SUPER) => {
            if is_bulk_move {
                move_bulk_standin_to_boundary(app, true);
            } else if is_inbox_move {
                move_inbox_to_boundary(app, true);
            } else if is_track_move {
                move_track_to_boundary(app, true);
            } else {
                move_task_to_boundary(app, true);
            }
        }
        (_, KeyCode::Home) => {
            if is_bulk_move {
                move_bulk_standin_to_boundary(app, true);
            } else if is_inbox_move {
                move_inbox_to_boundary(app, true);
            } else if is_track_move {
                move_track_to_boundary(app, true);
            } else {
                move_task_to_boundary(app, true);
            }
        }
        // Move to bottom
        (KeyModifiers::SHIFT, KeyCode::Char('G')) => {
            if is_bulk_move {
                move_bulk_standin_to_boundary(app, false);
            } else if is_inbox_move {
                move_inbox_to_boundary(app, false);
            } else if is_track_move {
                move_track_to_boundary(app, false);
            } else {
                move_task_to_boundary(app, false);
            }
        }
        (m, KeyCode::Down) if m.contains(KeyModifiers::SUPER) => {
            if is_bulk_move {
                move_bulk_standin_to_boundary(app, false);
            } else if is_inbox_move {
                move_inbox_to_boundary(app, false);
            } else if is_track_move {
                move_track_to_boundary(app, false);
            } else {
                move_task_to_boundary(app, false);
            }
        }
        (_, KeyCode::End) => {
            if is_bulk_move {
                move_bulk_standin_to_boundary(app, false);
            } else if is_inbox_move {
                move_inbox_to_boundary(app, false);
            } else if is_track_move {
                move_track_to_boundary(app, false);
            } else {
                move_task_to_boundary(app, false);
            }
        }
        // Outdent (decrease depth)
        (KeyModifiers::NONE, KeyCode::Left | KeyCode::Char('h'))
            if !is_track_move && !is_inbox_move && !is_bulk_move =>
        {
            move_task_outdent(app);
        }
        // Indent (increase depth)
        (KeyModifiers::NONE, KeyCode::Right | KeyCode::Char('l'))
            if !is_track_move && !is_inbox_move && !is_bulk_move =>
        {
            move_task_indent(app);
        }
        _ => {}
    }
}

/// After a move that may change the task's parent, ensure all ancestor nodes
/// of the task are expanded so it stays visible. Tracks which expand keys
/// were force-added (vs already expanded) in `MoveState::Task.force_expanded`.
pub(super) fn update_move_force_expanded(app: &mut App) {
    let (track_id, task_id) = match &app.move_state {
        Some(MoveState::Task {
            track_id, task_id, ..
        }) => (track_id.clone(), task_id.clone()),
        _ => return,
    };

    // Collect the ancestor expand keys for the task's current position
    let ancestor_keys = {
        let track = match App::find_track_in_project(&app.project, &track_id) {
            Some(t) => t,
            None => return,
        };
        let tasks = track.section_tasks(SectionKind::Backlog);
        let mut keys = Vec::new();
        if let Some(path) = find_task_path(tasks, &task_id) {
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
                    SectionKind::Backlog,
                    ancestor_path,
                ));
            }
        }
        keys
    };

    // Get mutable access to force_expanded and expanded set
    if let Some(MoveState::Task {
        force_expanded,
        track_id,
        ..
    }) = &mut app.move_state
    {
        let state = app.track_states.entry(track_id.clone()).or_default();

        // Remove old force-expanded keys that are no longer needed as ancestors
        let old_force: Vec<String> = force_expanded
            .iter()
            .filter(|k| !ancestor_keys.contains(k))
            .cloned()
            .collect();
        for key in &old_force {
            force_expanded.remove(key);
            state.expanded.remove(key);
        }

        // Add new ancestor keys that aren't already expanded
        for key in &ancestor_keys {
            if !state.expanded.contains(key) {
                state.expanded.insert(key.clone());
                force_expanded.insert(key.clone());
            }
        }
    }
}

/// Remove all force-expanded keys from the expanded set (on confirm/cancel).
pub(super) fn restore_force_expanded(
    app: &mut App,
    track_id: &str,
    force_expanded: &HashSet<String>,
) {
    if force_expanded.is_empty() {
        return;
    }
    let state = app.track_states.entry(track_id.to_string()).or_default();
    for key in force_expanded {
        state.expanded.remove(key);
    }
}

/// Check for external track changes and abort move mode if task deleted.
/// Returns false if move should be aborted.
pub(super) fn check_move_external_changes(app: &mut App, track_id: &str, task_id: &str) -> bool {
    if app.track_changed_on_disk(track_id)
        && let Some(disk_track) = app.read_track_from_disk(track_id)
    {
        if task_ops::find_task_in_track(&disk_track, task_id).is_none() {
            app.conflict_text = Some(format!("Task {} was deleted externally", task_id));
            app.mode = Mode::Navigate;
            app.move_state = None;
            app.replace_track(track_id, disk_track);
            drain_pending_for_track(app, track_id);
            return false;
        }
        app.replace_track(track_id, disk_track);
        drain_pending_for_track(app, track_id);
    }
    true
}

/// Move the task one position up or down among its siblings.
/// For subtasks, this moves within the parent's children list.
/// At boundaries, crosses to adjacent parents at the same depth.
pub(super) fn move_task_in_list(app: &mut App, direction: i32) {
    let (track_id, task_id) = match &app.move_state {
        Some(MoveState::Task {
            track_id, task_id, ..
        }) => (track_id.clone(), task_id.clone()),
        _ => return,
    };

    if !check_move_external_changes(app, &track_id, &task_id) {
        return;
    }

    let track = match app.find_track_mut(&track_id) {
        Some(t) => t,
        None => return,
    };

    let location = match task_ops::find_task_location(track, &task_id, SectionKind::Backlog) {
        Some(loc) => loc,
        None => return,
    };

    match &location.parent_id {
        None => {
            // Top-level: simple swap in backlog
            let backlog = match track.section_tasks_mut(SectionKind::Backlog) {
                Some(t) => t,
                None => return,
            };
            let cur_idx = location.sibling_index;
            let new_idx = (cur_idx as i32 + direction).clamp(0, backlog.len() as i32 - 1) as usize;
            if new_idx != cur_idx {
                let task = backlog.remove(cur_idx);
                backlog.insert(new_idx, task);
                let _ = app.save_track(&track_id);
                move_cursor_to_task(app, &track_id, &task_id);
            }
        }
        Some(parent_id) => {
            // Subtask: swap within parent's children or cross to adjacent parent
            let parent_id = parent_id.clone();
            let cur_idx = location.sibling_index;
            let parent = match task_ops::find_task_in_track(track, &parent_id) {
                Some(p) => p,
                None => return,
            };
            let sibling_count = parent.subtasks.len();

            let can_move_within = if direction < 0 {
                cur_idx > 0
            } else {
                cur_idx + 1 < sibling_count
            };

            if can_move_within {
                // Simple swap within same parent
                let new_idx = (cur_idx as i32 + direction) as usize;
                let parent_mut = match task_ops::find_task_mut_in_track(track, &parent_id) {
                    Some(p) => p,
                    None => return,
                };
                parent_mut.subtasks.swap(cur_idx, new_idx);
                parent_mut.mark_dirty();
                let _ = app.save_track(&track_id);
                move_cursor_to_task(app, &track_id, &task_id);
            } else {
                // At boundary: cross to adjacent parent
                // Find the task's depth to locate adjacent parents
                let task_depth = match task_ops::find_task_in_track(track, &task_id) {
                    Some(t) => t.depth,
                    None => return,
                };
                let parents = collect_potential_parents(track, task_depth, SectionKind::Backlog);
                let parent_pos = parents.iter().position(|p| p == &parent_id);
                if let Some(pp) = parent_pos {
                    let new_parent_pos = if direction < 0 {
                        if pp == 0 {
                            return;
                        }
                        pp - 1
                    } else {
                        if pp + 1 >= parents.len() {
                            return;
                        }
                        pp + 1
                    };
                    let new_parent_id = parents[new_parent_pos].clone();
                    // Remove task from current parent
                    if let Some((mut task, _)) = task_ops::remove_task_subtree(track, &task_id) {
                        // Insert as first child (moving up) or last child (moving down)
                        let insert_idx = if direction < 0 {
                            let new_parent = task_ops::find_task_in_track(track, &new_parent_id);
                            new_parent.map_or(0, |p| p.subtasks.len())
                        } else {
                            0
                        };
                        task.mark_dirty();
                        let _ = task_ops::insert_task_subtree(
                            track,
                            task,
                            Some(&new_parent_id),
                            SectionKind::Backlog,
                            insert_idx,
                        );
                        let _ = app.save_track(&track_id);
                        update_move_force_expanded(app);
                        move_cursor_to_task(app, &track_id, &task_id);
                    }
                }
            }
        }
    }
}

/// Collect all task IDs at depth-1 that could be parents for a task at the given depth.
/// Returns them in document order.
pub(super) fn collect_potential_parents(
    track: &Track,
    child_depth: usize,
    section: SectionKind,
) -> Vec<String> {
    let parent_depth = child_depth.saturating_sub(1);
    let tasks = track.section_tasks(section);
    let mut result = Vec::new();
    for task in tasks {
        collect_at_depth(task, 0, parent_depth, &mut result);
    }
    result
}

pub(super) fn collect_at_depth(
    task: &Task,
    current_depth: usize,
    target_depth: usize,
    result: &mut Vec<String>,
) {
    if current_depth == target_depth {
        if let Some(ref id) = task.id {
            result.push(id.clone());
        }
        return;
    }
    for sub in &task.subtasks {
        collect_at_depth(sub, current_depth + 1, target_depth, result);
    }
}

/// Move task to top or bottom among its siblings.
pub(super) fn move_task_to_boundary(app: &mut App, to_top: bool) {
    let (track_id, task_id) = match &app.move_state {
        Some(MoveState::Task {
            track_id, task_id, ..
        }) => (track_id.clone(), task_id.clone()),
        _ => return,
    };

    if !check_move_external_changes(app, &track_id, &task_id) {
        return;
    }

    let track = match app.find_track_mut(&track_id) {
        Some(t) => t,
        None => return,
    };

    let location = match task_ops::find_task_location(track, &task_id, SectionKind::Backlog) {
        Some(loc) => loc,
        None => return,
    };

    match &location.parent_id {
        None => {
            // Top-level: use existing move_task
            let pos = if to_top {
                InsertPosition::Top
            } else {
                InsertPosition::Bottom
            };
            let _ = task_ops::move_task(track, &task_id, pos);
        }
        Some(parent_id) => {
            // Subtask: move to first/last among parent's children
            let parent_id = parent_id.clone();
            if let Some((task, _)) = task_ops::remove_task_subtree(track, &task_id) {
                let insert_idx = if to_top {
                    0
                } else {
                    let parent = task_ops::find_task_in_track(track, &parent_id);
                    parent.map_or(0, |p| p.subtasks.len())
                };
                let _ = task_ops::insert_task_subtree(
                    track,
                    task,
                    Some(&parent_id),
                    SectionKind::Backlog,
                    insert_idx,
                );
            }
        }
    }
    let _ = app.save_track(&track_id);
    move_cursor_to_task(app, &track_id, &task_id);
}

/// Outdent: move task to be a sibling of its current parent (h key).
pub(super) fn move_task_outdent(app: &mut App) {
    let (track_id, task_id) = match &app.move_state {
        Some(MoveState::Task {
            track_id, task_id, ..
        }) => (track_id.clone(), task_id.clone()),
        _ => return,
    };

    if !check_move_external_changes(app, &track_id, &task_id) {
        return;
    }

    let track = match app.find_track_mut(&track_id) {
        Some(t) => t,
        None => return,
    };

    let location = match task_ops::find_task_location(track, &task_id, SectionKind::Backlog) {
        Some(loc) => loc,
        None => return,
    };

    // If already top-level, no-op
    let parent_id = match &location.parent_id {
        None => return,
        Some(pid) => pid.clone(),
    };

    // Find parent's location (grandparent context)
    let parent_location =
        match task_ops::find_task_location(track, &parent_id, SectionKind::Backlog) {
            Some(loc) => loc,
            None => return,
        };

    // Remove from current position
    let (mut task, _) = match task_ops::remove_task_subtree(track, &task_id) {
        Some(result) => result,
        None => return,
    };

    // Compute new depth
    let new_depth = match &parent_location.parent_id {
        None => 0, // parent is top-level, so we become top-level
        Some(gp) => {
            let gp_task = task_ops::find_task_in_track(track, gp);
            gp_task.map_or(0, |t| t.depth + 1)
        }
    };

    task_ops::set_subtree_depth(&mut task, new_depth);

    // Insert after the parent in the grandparent's children
    let insert_idx = parent_location.sibling_index + 1;
    let _ = task_ops::insert_task_subtree(
        track,
        task,
        parent_location.parent_id.as_deref(),
        SectionKind::Backlog,
        insert_idx,
    );

    let _ = app.save_track(&track_id);
    update_move_force_expanded(app);
    move_cursor_to_task(app, &track_id, &task_id);
}

/// Indent: make task the last child of the sibling above (l key).
pub(super) fn move_task_indent(app: &mut App) {
    let (track_id, task_id) = match &app.move_state {
        Some(MoveState::Task {
            track_id, task_id, ..
        }) => (track_id.clone(), task_id.clone()),
        _ => return,
    };

    if !check_move_external_changes(app, &track_id, &task_id) {
        return;
    }

    let track = match app.find_track_mut(&track_id) {
        Some(t) => t,
        None => return,
    };

    let location = match task_ops::find_task_location(track, &task_id, SectionKind::Backlog) {
        Some(loc) => loc,
        None => return,
    };

    // Need a sibling above to indent under
    if location.sibling_index == 0 {
        return;
    }

    // Find the sibling directly above
    let sibling_above_id = match &location.parent_id {
        None => {
            let backlog = track.backlog();
            backlog[location.sibling_index - 1].id.clone()
        }
        Some(pid) => {
            let parent = match task_ops::find_task_in_track(track, pid) {
                Some(p) => p,
                None => return,
            };
            parent.subtasks[location.sibling_index - 1].id.clone()
        }
    };

    let sibling_id = match sibling_above_id {
        Some(id) => id,
        None => return,
    };

    // Check depth constraint
    let task = match task_ops::find_task_in_track(track, &task_id) {
        Some(t) => t,
        None => return,
    };
    let task_max_depth = task_ops::max_subtree_depth(task);
    let sibling = match task_ops::find_task_in_track(track, &sibling_id) {
        Some(t) => t,
        None => return,
    };
    let new_depth = sibling.depth + 1;
    if new_depth + task_max_depth > 2 {
        return; // Would exceed max depth
    }

    // Remove from current position
    let (mut task, _) = match task_ops::remove_task_subtree(track, &task_id) {
        Some(result) => result,
        None => return,
    };

    task_ops::set_subtree_depth(&mut task, new_depth);

    // Insert as last child of the sibling above
    let insert_idx = {
        let sib = task_ops::find_task_in_track(track, &sibling_id);
        sib.map_or(0, |s| s.subtasks.len())
    };

    let _ = task_ops::insert_task_subtree(
        track,
        task,
        Some(&sibling_id),
        SectionKind::Backlog,
        insert_idx,
    );

    let _ = app.save_track(&track_id);
    update_move_force_expanded(app);
    move_cursor_to_task(app, &track_id, &task_id);
}

/// Move an inbox item up or down.
pub(super) fn move_inbox_item(app: &mut App, direction: i32) {
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
pub(super) fn move_inbox_to_boundary(app: &mut App, to_top: bool) {
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
pub(super) fn move_track_in_list(app: &mut App, direction: i32) {
    let (track_id, _) = match &app.move_state {
        Some(MoveState::Track {
            track_id,
            original_index,
        }) => (track_id.clone(), *original_index),
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

    let new_pos = (cur_pos as i32 + direction).clamp(0, active_tracks.len() as i32 - 1) as usize;
    if new_pos != cur_pos {
        let _ = crate::ops::track_ops::reorder_tracks(&mut app.project.config, &track_id, new_pos);
        app.tracks_cursor = new_pos;
    }
}

/// Move track to top or bottom of active tracks.
pub(super) fn move_track_to_boundary(app: &mut App, to_top: bool) {
    let (track_id, _) = match &app.move_state {
        Some(MoveState::Track {
            track_id,
            original_index,
        }) => (track_id.clone(), *original_index),
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
    let _ = crate::ops::track_ops::reorder_tracks(&mut app.project.config, &track_id, new_pos);
    app.tracks_cursor = new_pos;
}

/// Save the current track order to project.toml.
pub(super) fn save_track_order(app: &mut App) {
    save_config(app);
}

// ---------------------------------------------------------------------------
// Cursor movement

/// Handle Enter key: open detail view from track view, or enter edit / open subtask in detail view
pub(super) fn handle_enter(app: &mut App) {
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
                detail_enter_edit(app, false);
            }
        }
        View::Tracks => {
            // Switch to Track view for the track under cursor
            let track_id = tracks_cursor_track_id(app);
            if let Some(id) = track_id
                && let Some(idx) = app.active_track_ids.iter().position(|tid| tid == &id)
            {
                app.view = View::Track(idx);
            }
        }
        _ => {}
    }
}
