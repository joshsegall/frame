use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::model::SectionKind;
use crate::ops::task_ops::{self};

use crate::tui::app::{App, Mode, PendingMove, PendingMoveKind};
use crate::tui::undo::Operation;

use super::*;

pub(super) fn handle_confirm(app: &mut App, key: KeyEvent) {
    match (key.modifiers, key.code) {
        // Confirm: y
        (KeyModifiers::NONE, KeyCode::Char('y')) => {
            let state = app.confirm_state.take();
            app.mode = Mode::Navigate;
            if let Some(state) = state {
                match state.action {
                    crate::tui::app::ConfirmAction::DeleteInboxItem { index } => {
                        confirm_inbox_delete(app, index);
                    }
                    crate::tui::app::ConfirmAction::ArchiveTrack { track_id } => {
                        confirm_archive_track(app, &track_id);
                    }
                    crate::tui::app::ConfirmAction::DeleteTrack { track_id } => {
                        confirm_delete_track(app, &track_id);
                    }
                    crate::tui::app::ConfirmAction::DeleteTask { track_id, task_id } => {
                        confirm_delete_task(app, &track_id, &task_id);
                    }
                    crate::tui::app::ConfirmAction::BulkDeleteTasks { task_ids } => {
                        confirm_bulk_delete_tasks(app, &task_ids);
                    }
                    crate::tui::app::ConfirmAction::PruneRecovery => {
                        confirm_prune_recovery(app);
                    }
                    crate::tui::app::ConfirmAction::UnarchiveTrack { track_id } => {
                        confirm_unarchive_track(app, &track_id);
                    }
                    crate::tui::app::ConfirmAction::ImportTasks {
                        track_id,
                        file_path,
                    } => {
                        confirm_import_tasks(app, &track_id, &file_path);
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

pub(super) fn confirm_inbox_delete(app: &mut App, index: usize) {
    let inbox = match &mut app.project.inbox {
        Some(inbox) => inbox,
        None => return,
    };

    if index >= inbox.items.len() {
        return;
    }

    let item = inbox.items.remove(index);
    app.undo_stack.push(Operation::InboxDelete { index, item });
    let _ = app.save_inbox();

    // Clamp cursor
    let count = app.inbox_count();
    if count == 0 {
        app.inbox_cursor = 0;
    } else {
        app.inbox_cursor = app.inbox_cursor.min(count - 1);
    }
}

pub(super) fn confirm_archive_track(app: &mut App, track_id: &str) {
    let track_name = app.track_name(track_id).to_string();
    let old_state = app
        .project
        .config
        .tracks
        .iter()
        .find(|t| t.id == track_id)
        .map(|t| t.state.clone())
        .unwrap_or_default();

    // Update config state to archived
    if let Some(tc) = app
        .project
        .config
        .tracks
        .iter_mut()
        .find(|t| t.id == track_id)
    {
        tc.state = "archived".to_string();
    }
    save_config(app);

    // Move track file to archive/_tracks/
    if let Some(file) = app.track_file(track_id).map(|f| f.to_string()) {
        let _ = crate::ops::track_ops::archive_track_file(&app.project.frame_dir, track_id, &file);
    }

    rebuild_active_track_ids(app);

    app.undo_stack.push(Operation::TrackArchive {
        track_id: track_id.to_string(),
        old_state,
    });

    app.status_message = Some(format!("archived \"{}\"", track_name));
}

pub(super) fn confirm_delete_track(app: &mut App, track_id: &str) {
    let tc = match app.project.config.tracks.iter().find(|t| t.id == track_id) {
        Some(tc) => tc.clone(),
        None => return,
    };
    let prefix = app.project.config.ids.prefixes.get(track_id).cloned();

    // Remove track file
    if let Some(file) = app.track_file(track_id).map(|f| f.to_string()) {
        let track_path = app.project.frame_dir.join(&file);
        let _ = std::fs::remove_file(&track_path);
    }

    // Remove from config
    app.project.config.tracks.retain(|t| t.id != track_id);
    if prefix.is_some() {
        app.project.config.ids.prefixes.shift_remove(track_id);
    }
    save_config(app);

    // Remove from in-memory tracks
    app.project.tracks.retain(|(id, _)| id != track_id);

    rebuild_active_track_ids(app);

    app.undo_stack.push(Operation::TrackDelete {
        track_id: track_id.to_string(),
        track_name: tc.name.clone(),
        old_state: tc.state.clone(),
        prefix,
    });

    app.status_message = Some(format!("deleted track \"{}\"", tc.name));
}

// ---------------------------------------------------------------------------
// Recent view interactions (Phase 7.4)

/// Reopen a done task from the recent view (set state back to todo).
pub(super) fn reopen_recent_task(app: &mut App) {
    // Rebuild the sorted done-task list to find the task at current cursor
    let entries = build_recent_entries(app);

    let cursor = app.recent_cursor;
    let (track_id, task_id) = match entries.get(cursor) {
        Some(entry) => (entry.track_id.clone(), entry.id.clone()),
        None => return,
    };

    if task_id.is_empty() {
        return;
    }

    // Archived tasks cannot be reopened
    if entries.get(cursor).is_some_and(|e| e.is_archived) {
        app.status_message = Some("Archived tasks cannot be reopened".to_string());
        return;
    }

    // Check if this task already has a pending ToBacklog move (re-press = cancel reopen)
    if let Some(_pm) = app.cancel_pending_move(&track_id, &task_id) {
        // Re-close: restore state to Done, restore resolved date
        let track = match app.find_track_mut(&track_id) {
            Some(t) => t,
            None => return,
        };
        let task = match task_ops::find_task_mut_in_track(track, &task_id) {
            Some(t) => t,
            None => return,
        };

        task.state = crate::model::task::TaskState::Done;
        // Resolved date was never removed (kept during grace period), just restore state
        task.mark_dirty();

        // Pop the Reopen from undo stack (move to redo)
        // We do this by performing an undo, but we need to be careful—
        // instead, just pop the top entry if it's our Reopen
        let inbox = app.project.inbox.as_mut();
        let _ = app.undo_stack.undo(&mut app.project.tracks, inbox);

        let _ = app.save_track(&track_id);
        app.status_message = Some("Re-closed".to_string());
        return;
    }

    // Normal reopen: change state in-place in Done section (don't move yet)
    let track = match app.find_track_mut(&track_id) {
        Some(t) => t,
        None => return,
    };

    // Find the done_index for undo before mutating
    let done_index = {
        let done = track.section_tasks(SectionKind::Done);
        if done.is_empty() {
            return;
        }
        match done
            .iter()
            .position(|t| t.id.as_deref() == Some(task_id.as_str()))
        {
            Some(i) => i,
            None => return,
        }
    };

    let task = match task_ops::find_task_mut_in_track(track, &task_id) {
        Some(t) => t,
        None => return,
    };

    // Capture old state for undo
    let old_state = task.state;
    let old_resolved = task.metadata.iter().find_map(|m| {
        if let crate::model::task::Metadata::Resolved(d) = m {
            Some(d.clone())
        } else {
            None
        }
    });

    // Set state to Todo in-place in Done section.
    // Keep resolved date so the task maintains its sort position in Recent view
    // during the grace period. The resolved date is removed when the actual move
    // to Backlog happens (in execute_pending_move).
    task.state = crate::model::task::TaskState::Todo;
    task.mark_dirty();

    app.undo_stack.push(Operation::Reopen {
        track_id: track_id.clone(),
        task_id: task_id.clone(),
        old_state,
        old_resolved,
        done_index,
    });

    // Schedule pending move to Backlog (grace period)
    app.pending_moves.push(PendingMove {
        kind: PendingMoveKind::ToBacklog,
        track_id: track_id.clone(),
        task_id: task_id.clone(),
        deadline: std::time::Instant::now() + std::time::Duration::from_secs(5),
    });

    let _ = app.save_track(&track_id);

    let track_name = app.track_name(&track_id).to_string();
    app.status_message = Some(format!("Reopening in {}...", track_name));
}
