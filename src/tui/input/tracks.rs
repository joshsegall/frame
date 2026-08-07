use crate::tui::app::{App, EditHistory, EditTarget, Mode};
use crate::tui::undo::Operation;

use super::*;

/// Find the cursor position for a track ID in the tracks view.
///
/// Both this and [`tracks_cursor_track_id`] are thin wrappers over
/// [`App::tracks_view_order`], which owns the flat order — see its docs for
/// why that ordering lives in one place.
pub(super) fn tracks_find_cursor_pos(app: &App, target_id: &str) -> Option<usize> {
    app.tracks_view_position(target_id)
}

/// Map the tracks_cursor to the track ID at that position.
pub(super) fn tracks_cursor_track_id(app: &App) -> Option<String> {
    app.track_at_tracks_cursor().map(|s| s.to_string())
}

// ---------------------------------------------------------------------------
// Track management (Tracks view actions)

/// Enter EDIT mode to add a new track (type name → auto-generate ID)
pub(super) fn tracks_add_track(app: &mut App) {
    // Save cursor for restore on cancel
    app.pre_edit_cursor = Some(app.tracks_cursor);
    // Move cursor to the new row position (after all active tracks)
    let active_count = app
        .project
        .config
        .tracks
        .iter()
        .filter(|t| t.state == "active")
        .count();
    app.tracks_cursor = active_count;
    app.new_track_insert_pos = Some(active_count);
    app.edit_buffer.clear();
    app.edit_cursor = 0;
    app.edit_target = Some(EditTarget::NewTrackName);
    app.edit_history = Some(EditHistory::new("", 0, 0));
    app.edit_selection_anchor = None;
    app.mode = Mode::Edit;
}

/// Insert a new track after the cursor position and enter EDIT mode.
pub(super) fn tracks_insert_after(app: &mut App) {
    let active_count = app
        .project
        .config
        .tracks
        .iter()
        .filter(|t| t.state == "active")
        .count();
    // Only insert among active tracks
    if app.tracks_cursor >= active_count {
        return;
    }
    let insert_pos = (app.tracks_cursor + 1).min(active_count);
    app.pre_edit_cursor = Some(app.tracks_cursor);
    app.tracks_cursor = insert_pos;
    app.new_track_insert_pos = Some(insert_pos);
    app.edit_buffer.clear();
    app.edit_cursor = 0;
    app.edit_target = Some(EditTarget::NewTrackName);
    app.edit_history = Some(EditHistory::new("", 0, 0));
    app.edit_selection_anchor = None;
    app.mode = Mode::Edit;
}

/// Add a new track at the top of the active list and enter EDIT mode.
pub(super) fn tracks_prepend(app: &mut App) {
    app.pre_edit_cursor = Some(app.tracks_cursor);
    app.tracks_cursor = 0;
    app.new_track_insert_pos = Some(0);
    app.edit_buffer.clear();
    app.edit_cursor = 0;
    app.edit_target = Some(EditTarget::NewTrackName);
    app.edit_history = Some(EditHistory::new("", 0, 0));
    app.edit_selection_anchor = None;
    app.mode = Mode::Edit;
}

/// Enter EDIT mode to rename the track under the cursor
///
/// An archived track cannot be renamed, and the reason is that renaming one
/// writes its file. The name lives in `project.toml` *and* in the track's own
/// `# Title` header, so a rename saves the track — to `tracks/<file>`, which is
/// not where an archived track lives. Archived in this session, that recreated
/// the file the archive had just moved to `archive/_tracks/`, leaving every
/// task in the project twice; archived in an earlier one, `load_project` never
/// loaded the track, so the save failed with "track not found" and left an
/// unsaved entry nothing could ever clear.
///
/// Frozen is what the CLI already means by archived — `fr track rename
/// --prefix` skips archived tracks for the same reason — so this says so
/// instead of inventing a way to rewrite a file under `archive/`.
pub(super) fn tracks_edit_name(app: &mut App) {
    let track_id = match tracks_cursor_track_id(app) {
        Some(id) => id,
        None => return,
    };
    if app
        .project
        .config
        .tracks
        .iter()
        .any(|t| t.id == track_id && t.state == "archived")
    {
        app.status_message = Some(format!(
            "\"{}\" is archived — unarchive it to rename it",
            app.track_name(&track_id)
        ));
        app.status_is_error = true;
        return;
    }
    let current_name = app.track_name(&track_id).to_string();
    let cursor_pos = current_name.len();
    app.edit_buffer = current_name.clone();
    app.edit_cursor = cursor_pos;
    app.edit_target = Some(EditTarget::ExistingTrackName {
        track_id,
        original_name: current_name.clone(),
    });
    app.edit_history = Some(EditHistory::new(&current_name, cursor_pos, 0));
    app.edit_selection_anchor = None;
    app.mode = Mode::Edit;
}

/// Toggle shelve/activate for the track under the cursor
pub(super) fn tracks_toggle_shelve(app: &mut App) {
    let track_id = match tracks_cursor_track_id(app) {
        Some(id) => id,
        None => return,
    };

    let tc = match app.project.config.tracks.iter().find(|t| t.id == track_id) {
        Some(tc) => tc.clone(),
        None => return,
    };

    let was_active = tc.state == "active";
    let new_state = if was_active {
        "shelved"
    } else if tc.state == "shelved" {
        "active"
    } else {
        return;
    };

    // Update config
    if let Some(tc_mut) = app
        .project
        .config
        .tracks
        .iter_mut()
        .find(|t| t.id == track_id)
    {
        tc_mut.state = new_state.to_string();
    }
    save_config(app);

    // Update active_track_ids
    app.active_track_ids = app
        .project
        .config
        .tracks
        .iter()
        .filter(|t| t.state == "active")
        .map(|t| t.id.clone())
        .collect();

    app.undo_stack.push(Operation::TrackShelve {
        track_id: track_id.clone(),
        was_active,
    });

    // Clamp cursor
    let total = tracks_total_count(app);
    if total > 0 {
        app.tracks_cursor = app.tracks_cursor.min(total - 1);
    }

    app.status_message = Some(format!(
        "{} {} {}",
        if was_active { "shelved" } else { "activated" },
        track_id,
        if was_active { "\u{23F8}" } else { "\u{25B6}" }
    ));
}

pub(super) fn palette_archive_track(app: &mut App) {
    let track_id = match tracks_cursor_track_id(app) {
        Some(id) => id,
        None => return,
    };

    let track = match App::find_track_in_project(&app.project, &track_id) {
        Some(t) => t,
        None => return,
    };
    let count = crate::ops::track_ops::total_task_count(track);
    let display_name = app.track_name(&track_id).to_string();

    app.confirm_state = Some(crate::tui::app::ConfirmState {
        message: format!(
            "Archive track \"{}\"? ({} tasks) [y/n]",
            display_name, count
        ),
        action: crate::tui::app::ConfirmAction::ArchiveTrack { track_id },
    });
    app.mode = Mode::Confirm;
}

pub(super) fn palette_delete_track(app: &mut App) {
    let track_id = match tracks_cursor_track_id(app) {
        Some(id) => id,
        None => return,
    };

    let track = match App::find_track_in_project(&app.project, &track_id) {
        Some(t) => t,
        None => return,
    };
    let display_name = app.track_name(&track_id).to_string();

    // Delete is for empty tracks only — the same rule `fr track delete`
    // enforces, and the one `doc/tui.md` and `Operation::TrackDelete` both
    // already state. Unlike archiving, this unlinks the file, so a track with
    // tasks in it has nowhere to come back from once the session ends and the
    // undo stack goes with it.
    if !crate::ops::track_ops::is_track_empty_by_id(&app.project.frame_dir, track, &track_id) {
        let count = crate::ops::track_ops::total_task_count(track);
        app.status_message = Some(format!(
            "\"{}\" has {} tasks — archive it instead",
            display_name, count
        ));
        return;
    }

    app.confirm_state = Some(crate::tui::app::ConfirmState {
        message: format!("Delete track \"{}\"? (empty) [y/n]", display_name),
        action: crate::tui::app::ConfirmAction::DeleteTrack { track_id },
    });
    app.mode = Mode::Confirm;
}

/// Count total tracks in all states (for cursor clamping)
pub(super) fn tracks_total_count(app: &App) -> usize {
    app.project.config.tracks.len()
}

/// Enter EDIT mode to rename the track prefix under the cursor
pub(super) fn tracks_rename_prefix(app: &mut App) {
    let track_id = match tracks_cursor_track_id(app) {
        Some(id) => id,
        None => return,
    };

    let current_prefix = match app.project.config.ids.prefixes.get(&track_id) {
        Some(p) => p.clone(),
        None => return,
    };

    let track_name = app.track_name(&track_id).to_string();
    let cursor_pos = current_prefix.len();

    app.edit_buffer = current_prefix.clone();
    app.edit_cursor = cursor_pos;
    app.edit_target = Some(EditTarget::ExistingPrefix {
        track_id: track_id.clone(),
        original_prefix: current_prefix.clone(),
    });
    app.edit_history = Some(EditHistory::new(&current_prefix, cursor_pos, 0));
    app.edit_selection_anchor = Some(0); // Select all text initially
    app.prefix_rename = Some(crate::tui::app::PrefixRenameState {
        track_id,
        track_name,
        old_prefix: current_prefix,
        new_prefix: String::new(),
        confirming: false,
        task_id_count: 0,
        dep_ref_count: 0,
        affected_track_count: 0,
        validation_error: String::new(),
    });
    app.mode = Mode::Edit;
}

/// Validate a prefix string and return an error message (empty = valid)
pub(super) fn validate_prefix(
    input: &str,
    track_id: &str,
    config: &crate::model::config::ProjectConfig,
) -> String {
    if input.is_empty() {
        return "prefix cannot be empty".to_string();
    }
    if !input.chars().all(|c| c.is_ascii_alphanumeric()) {
        return "letters and numbers only".to_string();
    }
    // Check for duplicate prefix (case-insensitive)
    for (tid, prefix) in &config.ids.prefixes {
        if tid != track_id && prefix.eq_ignore_ascii_case(input) {
            let name = config
                .tracks
                .iter()
                .find(|t| t.id == *tid)
                .map(|t| t.name.as_str())
                .unwrap_or(tid);
            return format!("prefix already used by {}", name);
        }
    }
    String::new()
}

/// Execute the prefix rename: call ops layer, save all tracks + config, push sync marker
pub(super) fn execute_prefix_rename(app: &mut App) {
    let pr = match app.prefix_rename.take() {
        Some(pr) => pr,
        None => return,
    };

    let old_prefix = pr.old_prefix.clone();
    let new_prefix = pr.new_prefix.clone();
    let track_id = pr.track_id.clone();

    // The widest change the TUI makes: an archive file, `project.toml`, the
    // renamed track, and every other track carrying a `dep:` into it. All of it
    // under one lock — a writer landing in the middle would see ids under two
    // prefixes at once — or none of it.
    app.with_project_lock(|app| {
        // Rename IDs in archive file (shared ops function)
        let _ = crate::ops::track_ops::rename_archive_prefix(
            &app.project.frame_dir,
            &track_id,
            &old_prefix,
            &new_prefix,
        );

        // Call the rename operation on in-memory tracks
        let result = crate::ops::track_ops::rename_track_prefix(
            &mut app.project.config,
            &mut app.project.tracks,
            &track_id,
            &old_prefix,
            &new_prefix,
        );

        match result {
            Ok(rename_result) => {
                // Save config
                save_config(app);

                // Save the target track
                app.save_track_logged(&track_id);

                // Save all other affected tracks (those with updated dep references)
                let affected_tracks: Vec<String> = app
                    .project
                    .tracks
                    .iter()
                    .filter(|(tid, track)| tid != &track_id && has_dirty_tasks(track))
                    .map(|(tid, _)| tid.clone())
                    .collect();
                for tid in &affected_tracks {
                    app.save_track_logged(tid);
                }

                // Push sync marker (no undo for prefix rename)
                app.undo_stack.push_sync_marker();

                app.status_message = Some(format!(
                    "renamed {} \u{2192} {}: {} tasks, {} deps across {} tracks",
                    old_prefix,
                    new_prefix,
                    rename_result.tasks_renamed,
                    rename_result.deps_updated,
                    rename_result.tracks_affected,
                ));
            }
            Err(e) => {
                app.status_message = Some(format!("prefix rename failed: {}", e));
                app.status_is_error = true;
            }
        }
    });
}

/// Check if any task in a track has the dirty flag set
pub(super) fn has_dirty_tasks(track: &crate::model::Track) -> bool {
    for node in &track.nodes {
        if let crate::model::track::TrackNode::Section { tasks, .. } = node
            && check_dirty_recursive(tasks)
        {
            return true;
        }
    }
    false
}

pub(super) fn check_dirty_recursive(tasks: &[crate::model::Task]) -> bool {
    for task in tasks {
        if task.dirty {
            return true;
        }
        if check_dirty_recursive(&task.subtasks) {
            return true;
        }
    }
    false
}

/// Update prefix validation error based on current edit buffer (no-op if not editing a prefix)
pub(super) fn update_prefix_validation(app: &mut App) {
    if let Some(EditTarget::ExistingPrefix { ref track_id, .. }) = app.edit_target {
        let tid = track_id.clone();
        if let Some(ref mut pr) = app.prefix_rename {
            pr.validation_error = validate_prefix(&app.edit_buffer, &tid, &app.project.config);
        }
    }
}

/// Update the "# Title" header in a track's literal nodes
pub(super) fn update_track_header(app: &mut App, track_id: &str, new_name: &str) {
    if let Some(track) = app.find_track_mut(track_id) {
        for node in &mut track.nodes {
            if let crate::model::track::TrackNode::Literal(lines) = node {
                for line in lines.iter_mut() {
                    if line.starts_with("# ") {
                        *line = format!("# {}", new_name);
                        return;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::app::app_on_disk;

    /// Found by P8 in two keystrokes and no second writer at all: archive a
    /// track, then rename it, and every task in the project exists twice — once
    /// in `archive/_tracks/` where the archive put it, and once in `tracks/`
    /// where the rename's save wrote it back from memory.
    #[test]
    fn an_archived_track_cannot_be_renamed_back_into_existence() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut app = app_on_disk(tmp.path());
        let live = app.project.frame_dir.join("tracks/a.md");

        super::super::confirm::confirm_archive_track(&mut app, "a");
        assert!(!live.exists(), "archiving moved the file");

        // The cursor lands on the archived track: they sort last, and it is the
        // only one left.
        app.tracks_cursor = 0;
        tracks_edit_name(&mut app);

        assert_ne!(
            app.mode,
            crate::tui::app::Mode::Edit,
            "the rename must not have opened"
        );
        assert!(
            !live.exists(),
            "and nothing may have recreated the archived track's file"
        );
    }

    /// The refusal is about *archived*, not about any track the user cannot see
    /// in the active list. A shelved track is still loaded and still lives in
    /// `tracks/`, so renaming it is ordinary.
    #[test]
    fn a_shelved_track_can_still_be_renamed() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut app = app_on_disk(tmp.path());
        app.project.config.tracks[0].state = "shelved".into();
        app.tracks_cursor = 0;

        tracks_edit_name(&mut app);
        assert_eq!(app.mode, crate::tui::app::Mode::Edit);
    }
}
