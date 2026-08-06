use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::model::SectionKind;
use crate::ops::task_ops::{self};

use crate::tui::app::{App, Mode, PendingMove};
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
    app.save_inbox_logged();

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
    // Config first, file second, same as the CLI — and recorded the same way,
    // so an interruption between them is completed by the next write command
    // rather than left for `fr check` to report.
    let marker = app
        .track_file(track_id)
        .map(|f| f.to_string())
        .and_then(|file| {
            crate::io::inflight::InFlight::begin(
                &app.project.frame_dir,
                crate::io::inflight::Operation::TrackArchive {
                    track_id: track_id.to_string(),
                    file,
                },
                &format!("archive {track_id}"),
            )
            .ok()
        });

    save_config(app);

    // Move track file to archive/_tracks/
    if let Some(file) = app.track_file(track_id).map(|f| f.to_string()) {
        let _ = crate::ops::track_ops::archive_track_file(&app.project.frame_dir, track_id, &file);
    }

    if let Some(marker) = marker {
        marker.commit();
    }

    rebuild_active_track_ids(app);

    app.undo_stack.push(Operation::TrackArchive {
        track_id: track_id.to_string(),
        old_state,
    });

    app.status_message = Some(format!("archived \"{}\"", track_name));
}

pub(super) fn confirm_delete_track(app: &mut App, track_id: &str) {
    let (config_index, tc) = match app
        .project
        .config
        .tracks
        .iter()
        .position(|t| t.id == track_id)
    {
        Some(i) => (i, app.project.config.tracks[i].clone()),
        None => return,
    };
    let prefix = app.project.config.ids.prefixes.get(track_id).cloned();
    let prefix_index = app.project.config.ids.prefixes.get_index_of(track_id);

    // Read the file before unlinking it, so undo has something to put back.
    // This is the only copy: delete does not archive, and nothing here reaches
    // the recovery log.
    let mut content = None;
    if let Some(file) = app.track_file(track_id).map(|f| f.to_string()) {
        let track_path = app.project.frame_dir.join(&file);
        content = std::fs::read_to_string(&track_path).ok();
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
        config_index,
        prefix_index,
        content,
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

        app.save_track_logged(&track_id);
        app.status_message = Some("Re-closed".into());
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
        from: SectionKind::Done,
        to: SectionKind::Backlog,
        // `Operation::Reopen` above puts the task back in Done at its original
        // index itself, so a `SectionMove` entry here would undo it twice.
        push_undo: false,
        track_id: track_id.clone(),
        task_id: task_id.clone(),
        deadline: std::time::Instant::now() + std::time::Duration::from_secs(5),
        old_state: Some(old_state),
    });

    app.save_track_logged(&track_id);

    let track_name = app.track_name(&track_id).to_string();
    app.status_message = Some(format!("Reopening in {}...", track_name));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::app::app_on_disk;
    use crate::tui::input::common::{perform_redo, perform_undo};
    use crate::tui::input::tracks::palette_delete_track;

    const TRACK_A: &str = "# A\n\n## Backlog\n\n- [ ] `A-001` One\n\n## Done\n";

    fn track_path(app: &App) -> std::path::PathBuf {
        app.project.frame_dir.join("tracks/a.md")
    }

    /// Deleting a track unlinks the file — nothing is archived and nothing
    /// reaches the recovery log, so the undo entry is the only copy. It used to
    /// carry the name and not the content, and undo rebuilt an empty shell: the
    /// track came back in the sidebar with its name, prefix and position, and
    /// every task in it was gone with no error reported.
    #[test]
    fn undoing_a_track_delete_restores_the_file_it_removed() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut app = app_on_disk(tmp.path());

        confirm_delete_track(&mut app, "a");
        assert!(!track_path(&app).exists(), "the file should be gone");

        perform_undo(&mut app);

        assert_eq!(
            std::fs::read_to_string(track_path(&app)).unwrap(),
            TRACK_A,
            "undo must put back the bytes that were there, not a fresh shell"
        );
        let track = App::find_track_in_project(&app.project, "a").expect("track back in memory");
        assert_eq!(
            crate::ops::track_ops::total_task_count(track),
            1,
            "and the tasks with it"
        );
    }

    /// Redo re-deletes, so the recorded content has to survive being replayed
    /// in both directions.
    #[test]
    fn a_track_delete_survives_undo_redo_undo() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut app = app_on_disk(tmp.path());

        confirm_delete_track(&mut app, "a");
        perform_undo(&mut app);
        perform_redo(&mut app);
        assert!(!track_path(&app).exists(), "redo deletes it again");

        perform_undo(&mut app);
        assert_eq!(
            std::fs::read_to_string(track_path(&app)).unwrap(),
            TRACK_A,
            "and the second undo restores it just as the first did"
        );
    }

    /// An unreadable file leaves nothing to record. Undo then has no better
    /// answer than the shell, which is the old behaviour — it must not panic
    /// or leave the track missing from the config.
    #[test]
    fn a_delete_with_no_readable_file_still_undoes_to_something() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut app = app_on_disk(tmp.path());
        std::fs::remove_file(track_path(&app)).unwrap();

        confirm_delete_track(&mut app, "a");
        perform_undo(&mut app);

        assert!(
            std::fs::read_to_string(track_path(&app))
                .unwrap()
                .starts_with("# A"),
            "a shell, but a real one"
        );
        assert!(app.project.config.tracks.iter().any(|t| t.id == "a"));
    }

    /// The CLI refuses to delete a track with tasks in it, `doc/tui.md` says
    /// the TUI does too, and `Operation::TrackDelete` calls itself "empty track
    /// only". Only the code disagreed: the prompt went straight up with no
    /// count and no check, and `y` unlinked the file.
    #[test]
    fn deleting_a_track_with_tasks_is_refused_before_the_prompt() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut app = app_on_disk(tmp.path());
        app.tracks_cursor = 0;

        palette_delete_track(&mut app);

        assert!(
            app.confirm_state.is_none(),
            "a non-empty track never reaches the confirmation"
        );
        assert!(!matches!(app.mode, Mode::Confirm));
        let msg = app.status_message.clone().unwrap_or_default();
        assert!(
            msg.contains("1 tasks") && msg.contains("archive"),
            "it should say how much is there and where to put it: {msg}"
        );
        assert!(track_path(&app).exists(), "and touch nothing");
    }

    /// An empty track is still deletable — the guard is a guard, not a removal
    /// of the feature.
    #[test]
    fn deleting_an_empty_track_still_prompts() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut app = app_on_disk(tmp.path());
        std::fs::write(track_path(&app), "# A\n\n## Backlog\n\n## Done\n").unwrap();
        app.replace_track(
            "a",
            crate::parse::parse_track("# A\n\n## Backlog\n\n## Done\n"),
        );
        app.tracks_cursor = 0;

        palette_delete_track(&mut app);

        assert!(app.confirm_state.is_some(), "an empty track prompts");
        assert!(matches!(app.mode, Mode::Confirm));
    }

    /// Redo of a track add rebuilt the name as `tid.clone()`, so a track called
    /// "My Track" came back called "my-track".
    #[test]
    fn redoing_a_track_add_keeps_the_name_the_user_typed() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut app = app_on_disk(tmp.path());
        app.undo_stack.push(Operation::TrackAdd {
            track_id: "a".to_string(),
            track_name: "A".to_string(),
            config_index: 0,
            prefix: "A".to_string(),
        });

        perform_undo(&mut app);
        perform_redo(&mut app);

        let tc = app
            .project
            .config
            .tracks
            .iter()
            .find(|t| t.id == "a")
            .expect("track is back");
        assert_eq!(tc.name, "A", "redo must not rename the track to its ID");
    }

    /// A new track goes among the *active* ones, which is not the end of the
    /// list when something shelved sits below. Redo appended, so the track came
    /// back in a different place than the add had put it — and re-derived its
    /// prefix from the current prefix set rather than replaying the one it was
    /// given, which is a different question with a different answer.
    #[test]
    fn redoing_a_track_add_restores_its_position_and_prefix() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut app = app_on_disk(tmp.path());
        app.project.config.tracks.push(crate::model::TrackConfig {
            id: "z".into(),
            name: "Z".into(),
            state: "shelved".into(),
            file: "tracks/z.md".into(),
        });
        // As the add records it: inserted at index 1, ahead of the shelved
        // track, carrying the prefix it assigned.
        app.project.config.tracks.insert(
            1,
            crate::model::TrackConfig {
                id: "b".into(),
                name: "B".into(),
                state: "active".into(),
                file: "tracks/b.md".into(),
            },
        );
        app.project
            .config
            .ids
            .prefixes
            .insert("b".into(), "B".into());
        app.undo_stack.push(Operation::TrackAdd {
            track_id: "b".into(),
            track_name: "B".into(),
            config_index: 1,
            prefix: "B".into(),
        });

        perform_undo(&mut app);
        perform_redo(&mut app);

        let ids: Vec<_> = app
            .project
            .config
            .tracks
            .iter()
            .map(|t| t.id.as_str())
            .collect();
        assert_eq!(ids, vec!["a", "b", "z"], "redo puts it back where it was");
        assert_eq!(
            app.project.config.ids.prefixes.get("b").map(String::as_str),
            Some("B"),
            "with the prefix the add assigned"
        );
    }

    /// And the same for a delete, from the other side: undo used to push the
    /// restored track onto the end of both the track list and the prefix map,
    /// so deleting the first track and undoing brought it back as the last one.
    /// The file was restored intact and the project still looked rearranged.
    #[test]
    fn undoing_a_track_delete_restores_its_position() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut app = app_on_disk(tmp.path());
        app.project
            .config
            .ids
            .prefixes
            .insert("a".into(), "A".into());
        app.project.config.tracks.push(crate::model::TrackConfig {
            id: "z".into(),
            name: "Z".into(),
            state: "active".into(),
            file: "tracks/z.md".into(),
        });
        app.project
            .config
            .ids
            .prefixes
            .insert("z".into(), "Z".into());

        confirm_delete_track(&mut app, "a");
        perform_undo(&mut app);

        let ids: Vec<_> = app
            .project
            .config
            .tracks
            .iter()
            .map(|t| t.id.as_str())
            .collect();
        assert_eq!(ids, vec!["a", "z"], "back at the front, not appended");
        let prefixes: Vec<_> = app.project.config.ids.prefixes.keys().cloned().collect();
        assert_eq!(prefixes, vec!["a", "z"], "and the prefix map with it");
    }
}
