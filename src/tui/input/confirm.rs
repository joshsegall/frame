use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::model::SectionKind;
use crate::ops::task_ops::{self};

use crate::tui::app::{App, Mode, PendingMove, SaveTarget, TrackExit, TrackOnDisk};
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

    let item = inbox.take_item(index);
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

    // Config and file move under one lock, or neither. Both halves are the same
    // change, and another `fr` that read the project between them would see a
    // track that is archived in the config and still in `tracks/` — or write
    // back the copy it had loaded and undo the move.
    let mut archived = false;
    let mut stale = None;
    let done = app.with_project_lock(|app| {
        // Has another process already taken this track out of `tracks/`?
        //
        // Nothing below would notice. `save_track_logged` writes
        // `tracks/<file>` back from memory — `absorb_external_change` reads the
        // file to see whether anyone else has written it, finds it *missing*
        // and returns, so the save goes through unconditionally and recreates
        // it. `archive_track_file` then renames that copy over the one the
        // other process archived, and a rename does not consult what it lands
        // on: no merge runs, nothing reaches the recovery log, and whatever
        // they put there is gone. The config merge sees both sides saying
        // `archived` and reports nothing either.
        //
        // Asking disk rather than `app.project.config` is the whole point, and
        // it is safe to ask here for the same reason `track_id_taken_on_disk`
        // is: the lock is held, so this cannot go stale before the move.
        match app.track_on_disk(track_id) {
            TrackOnDisk::Archived => {
                stale = Some(format!(
                    "\"{track_name}\" was already archived by another process — nothing was changed"
                ));
                return;
            }
            TrackOnDisk::Gone => {
                stale = Some(format!(
                    "\"{track_name}\" is no longer in this project — nothing was changed"
                ));
                return;
            }
            TrackOnDisk::Live | TrackOnDisk::Unreadable => {}
        }

        // Whatever this track is still holding belongs in the copy being
        // archived, so it has to reach disk before the file moves. If it
        // cannot — the usual reason being that a save already failed and it is
        // waiting in `unsaved` — then archiving would move a stale file and
        // take the newer version out of reach with it, since the in-memory copy
        // goes away below.
        app.save_track_logged(track_id);
        if app
            .unsaved
            .contains_key(&SaveTarget::Track(track_id.to_string()))
        {
            return;
        }

        if let Some(tc) = app
            .project
            .config
            .tracks
            .iter_mut()
            .find(|t| t.id == track_id)
        {
            tc.state = "archived".to_string();
        }
        // Config first, file second, same as the CLI — and recorded the same
        // way, so an interruption between them is completed by the next write
        // command rather than left for `fr check` to report.
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
            let _ =
                crate::ops::track_ops::archive_track_file(&app.project.frame_dir, track_id, &file);
        }

        if let Some(marker) = marker {
            marker.commit();
        }
        archived = true;
    });
    if !done {
        return;
    }
    if let Some(message) = stale {
        app.status_message = Some(message);
        app.status_is_error = true;
        app.catch_up_on_config();
        return;
    }
    if !archived {
        app.status_message = Some(format!(
            "could not archive \"{track_name}\": its latest edits are not on disk yet"
        ));
        app.status_is_error = true;
        return;
    }

    // Out of `tracks/` means out of the project, exactly as a restart would
    // have it: `load_project` does not load an archived track. Left in memory it
    // is still reachable — by a jump to one of its tasks, by the tracks view,
    // by an undo — and *anything* that saves it writes `tracks/<file>`, which
    // recreates the file the archive just moved. P8 found that in three
    // keystrokes with no second writer involved: archive, move a task, and
    // every id in the track exists twice, in two files, both looking
    // authoritative.
    //
    // No flush here: this one already flushed above, under the lock and before
    // the file moved, which is the only point at which a flush is either
    // possible or safe. By now `tracks/<file>` is gone and writing it would put
    // it back.
    app.release_track(track_id, TrackExit::NoFlush);

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

    // Unlink and config rewrite under one lock, or neither — see
    // `confirm_archive_track`. This one matters more: the file it removes is
    // the only copy there is.
    let mut content = None;
    let mut stale = None;
    let done = app.with_project_lock(|app| {
        // The same question the archive asks, and it matters more here. This
        // one removes the row outright, so a track another process archived
        // while the session was not looking would lose its config entry with
        // its file sitting in `archive/_tracks/` — the unclaimed archived file
        // `fr check` reports, created by a keystroke rather than found. The
        // `content` undo records would be `None` too, since the file is no
        // longer where this reads it from, so the undo could not put it back.
        match app.track_on_disk(track_id) {
            TrackOnDisk::Archived => {
                stale = Some(format!(
                    "\"{}\" was archived by another process — nothing was changed",
                    tc.name
                ));
                return;
            }
            TrackOnDisk::Gone => {
                stale = Some(format!(
                    "\"{}\" is no longer in this project — nothing was changed",
                    tc.name
                ));
                return;
            }
            TrackOnDisk::Live | TrackOnDisk::Unreadable => {}
        }

        // Read the file before unlinking it, so undo has something to put back.
        // This is the only copy: delete does not archive, and nothing here
        // reaches the recovery log.
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
    });
    if !done {
        return;
    }
    if let Some(message) = stale {
        app.status_message = Some(message);
        app.status_is_error = true;
        app.catch_up_on_config();
        return;
    }

    // Remove from in-memory tracks. The file is already unlinked, so there is
    // nothing to flush into; an edit that never reached it goes to the recovery
    // log, which is also the only place it can now be read — the `content` the
    // undo recorded came off disk and is the version *before* that edit.
    app.release_track(track_id, TrackExit::NoFlush);

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

#[cfg(test)]
mod lock_tests {
    use super::*;
    use crate::io::lock::FileLock;
    use crate::tui::app::{app_on_disk, app_with_config_file};

    /// Put the project in the state another process leaves behind when it
    /// archives a track this session still believes is active: the row on disk
    /// says `archived` and the file has moved, while memory says neither.
    fn archived_by_someone_else(app: &App) {
        let frame_dir = &app.project.frame_dir;
        let config = frame_dir.join("project.toml");
        let text = std::fs::read_to_string(&config).unwrap();
        std::fs::write(
            &config,
            text.replace("state = \"active\"", "state = \"archived\""),
        )
        .unwrap();
        std::fs::create_dir_all(frame_dir.join("archive/_tracks")).unwrap();
        std::fs::rename(
            frame_dir.join("tracks/a.md"),
            frame_dir.join("archive/_tracks/a.md"),
        )
        .unwrap();
    }

    /// **A stale archive must not move its own copy over the archived one.**
    ///
    /// Found by P8 once `TrackArchive` was allowed to land on a track the CLI
    /// owned. Nothing on the path noticed: `save_track_logged` writes
    /// `tracks/a.md` back from memory, because `absorb_external_change` reads
    /// the file to see whether anyone else has written it, finds it missing and
    /// returns; the config merge sees both sides saying `archived` and reports
    /// nothing; and `archive_track_file` renames the stale copy over the other
    /// one, which a rename does without consulting what it lands on. No merge,
    /// no recovery entry, and whatever the other process archived is gone.
    #[test]
    fn archiving_a_track_someone_else_archived_does_not_overwrite_it() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut app = app_with_config_file(tmp.path());
        archived_by_someone_else(&app);
        let archived = app.project.frame_dir.join("archive/_tracks/a.md");
        let theirs = "# A\n\n## Backlog\n\n- [ ] `A-001` One\n- [ ] `A-002` Theirs\n\n## Done\n";
        std::fs::write(&archived, theirs).unwrap();

        confirm_archive_track(&mut app, "a");

        assert_eq!(
            std::fs::read_to_string(&archived).unwrap(),
            theirs,
            "the archived copy is theirs, and this session never held `A-002`"
        );
        assert!(
            !app.project.frame_dir.join("tracks/a.md").exists(),
            "and the file it was holding must not be back under tracks/"
        );
        assert!(
            app.status_message
                .as_deref()
                .is_some_and(|s| s.contains("already archived by another process")),
            "the refusal has to say why: {:?}",
            app.status_message
        );
        // Refusing is only half of it. A session still holding the track can
        // write `tracks/a.md` back from any later save — the `1da9c05` hole —
        // so the refusal ends by taking the config it just read.
        assert!(
            App::find_track_in_project(&app.project, "a").is_none(),
            "the session has to let go of a track the project no longer has"
        );
    }

    /// The same guard on the operation that would do more damage. Deleting a
    /// track another process archived removes its row while the file sits in
    /// `archive/_tracks/` — the unclaimed archived file `fr check` reports,
    /// created by a keystroke rather than found — and the `content` the undo
    /// records would be `None`, so there would be no way back either.
    #[test]
    fn deleting_a_track_someone_else_archived_does_not_drop_its_row() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut app = app_with_config_file(tmp.path());
        archived_by_someone_else(&app);

        confirm_delete_track(&mut app, "a");

        let config = std::fs::read_to_string(app.project.frame_dir.join("project.toml")).unwrap();
        assert!(config.contains("id = \"a\""), "the row survives: {config}");
        assert!(
            app.project.frame_dir.join("archive/_tracks/a.md").exists(),
            "and so does the file"
        );
        assert!(
            app.status_message
                .as_deref()
                .is_some_and(|s| s.contains("archived by another process")),
            "{:?}",
            app.status_message
        );
    }

    /// The same answer for the same reason, reached the other way: a
    /// `project.toml` frame cannot parse stops the operation before it starts.
    ///
    /// This is the site the pre-flight exists for. Left to fail inside the body,
    /// the config write is refused *after* the marker is begun and the file is
    /// moved — so `tracks/a.md` ends up in `archive/_tracks/` with the config
    /// still calling the track active, and the committed marker's recovery would
    /// have asserted "config already had it archived", which was never true.
    #[test]
    fn archiving_a_track_does_nothing_while_project_toml_is_damaged() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut app = app_with_config_file(tmp.path());
        let track_path = app.project.frame_dir.join("tracks/a.md");
        let archived = app.project.frame_dir.join("archive/_tracks/a.md");
        let config = app.project.frame_dir.join("project.toml");
        let damaged = "[project]\n<<<<<<< HEAD\nname = \"mine\"\n";
        std::fs::write(&config, damaged).unwrap();

        confirm_archive_track(&mut app, "a");

        assert!(track_path.exists(), "the track file must not have moved");
        assert!(!archived.exists(), "and must not have been copied either");
        assert_eq!(
            std::fs::read_to_string(&config).unwrap(),
            damaged,
            "the damaged config is untouched"
        );
        assert_eq!(
            app.project.config.tracks[0].state, "active",
            "nor should the config have been changed in memory"
        );
        assert!(
            crate::io::inflight::read(&app.project.frame_dir).is_none(),
            "and no marker claims an archive is half-done"
        );

        // And once the file is readable again, the same keystroke works.
        std::fs::write(&config, crate::tui::app::CONFIG_WITH_COMMENTS).unwrap();
        confirm_archive_track(&mut app, "a");
        assert!(!track_path.exists() && archived.exists(), "the file moved");
        assert_eq!(app.project.config.tracks[0].state, "archived");
        assert!(
            std::fs::read_to_string(&config)
                .unwrap()
                .contains("struct dump cannot emit"),
            "with the file's comments intact"
        );
    }

    /// Archiving a track is a `project.toml` write and a file move, and the TUI
    /// did both with no lock at all — so another `fr` that had already read the
    /// project could have the track moved out from under it and then write the
    /// copy it loaded back into `tracks/`, leaving the same tasks in two files
    /// and every id twice. P8 found it in three events.
    ///
    /// Contended, the right answer is to do nothing: there is no half of
    /// "archive this track" worth keeping, and an operation that moves a file
    /// has nothing the retry machinery could hold for a later attempt.
    #[test]
    fn archiving_a_track_does_nothing_while_another_process_holds_the_lock() {
        crate::io::lock::cap_waits(std::time::Duration::from_millis(20));
        let tmp = tempfile::TempDir::new().unwrap();
        let mut app = app_on_disk(tmp.path());
        let track_path = app.project.frame_dir.join("tracks/a.md");
        let archived = app.project.frame_dir.join("archive/_tracks/a.md");

        let held = FileLock::acquire_default(&app.project.frame_dir).unwrap();
        confirm_archive_track(&mut app, "a");

        assert!(track_path.exists(), "the track file must not have moved");
        assert!(!archived.exists(), "and must not have been copied either");
        assert_eq!(
            app.project.config.tracks[0].state, "active",
            "nor should the config have been changed in memory"
        );

        // And once the other writer is done, the same keystroke works.
        drop(held);
        confirm_archive_track(&mut app, "a");
        assert!(!track_path.exists() && archived.exists(), "the file moved");
        assert_eq!(app.project.config.tracks[0].state, "archived");
    }

    /// Out of `tracks/` is out of the project: `load_project` would not load an
    /// archived track after a restart, and a session that keeps one in memory
    /// leaves it reachable — by a jump to one of its tasks, by the tracks view,
    /// by an undo — where *anything* that saves it writes `tracks/<file>` and
    /// recreates the file the archive just moved. P8 found that in three
    /// keystrokes and no second writer: archive, move a task, and every id in
    /// the track exists twice, in two files.
    #[test]
    fn an_archived_track_leaves_the_project() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut app = app_on_disk(tmp.path());
        let live = app.project.frame_dir.join("tracks/a.md");

        confirm_archive_track(&mut app, "a");

        assert!(!live.exists(), "the file moved to the archive");
        assert!(
            app.project.tracks.iter().all(|(id, _)| id != "a"),
            "and the track is no longer part of the loaded project"
        );
        assert!(
            !app.jump_to_task("A-001"),
            "so nothing can navigate back into it and write the file again"
        );
    }

    /// An edit that has not reached disk belongs in the copy being archived.
    /// The archive moves the *file*, so without flushing first it moves the
    /// stale one and the newer version goes out of reach with the in-memory
    /// track.
    #[test]
    fn archiving_carries_an_unsaved_edit_into_the_archive() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut app = app_on_disk(tmp.path());
        let archived = app.project.frame_dir.join("archive/_tracks/a.md");

        let tasks = app
            .find_track_mut("a")
            .unwrap()
            .section_tasks_mut(SectionKind::Backlog)
            .unwrap();
        tasks[0].title = "Edited but not yet saved".into();
        tasks[0].dirty = true;

        confirm_archive_track(&mut app, "a");

        let text = std::fs::read_to_string(&archived).expect("the archived file exists");
        assert!(
            text.contains("Edited but not yet saved"),
            "the archived copy is the latest one:\n{text}"
        );
    }

    /// The same for delete, where the stakes are higher: the file it unlinks is
    /// the only copy there is.
    #[test]
    fn deleting_a_track_does_nothing_while_another_process_holds_the_lock() {
        crate::io::lock::cap_waits(std::time::Duration::from_millis(20));
        let tmp = tempfile::TempDir::new().unwrap();
        let mut app = app_on_disk(tmp.path());
        let track_path = app.project.frame_dir.join("tracks/a.md");

        let _held = FileLock::acquire_default(&app.project.frame_dir).unwrap();
        confirm_delete_track(&mut app, "a");

        assert!(track_path.exists(), "the only copy must still be there");
        assert_eq!(
            app.project.config.tracks.len(),
            1,
            "and the config must still list the track"
        );
    }
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
