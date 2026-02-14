use crate::tui::app::{App, EditHistory, EditTarget, Mode};
use crate::tui::undo::Operation;

use super::*;

/// Find the cursor position for a track ID in the tracks view (flat order: active, shelved, archived)
pub(super) fn tracks_find_cursor_pos(app: &App, target_id: &str) -> Option<usize> {
    let mut idx = 0;
    for tc in &app.project.config.tracks {
        if tc.state == "active" {
            if tc.id == target_id {
                return Some(idx);
            }
            idx += 1;
        }
    }
    for tc in &app.project.config.tracks {
        if tc.state == "shelved" {
            if tc.id == target_id {
                return Some(idx);
            }
            idx += 1;
        }
    }
    for tc in &app.project.config.tracks {
        if tc.state == "archived" {
            if tc.id == target_id {
                return Some(idx);
            }
            idx += 1;
        }
    }
    None
}

/// Map the tracks_cursor to the track ID at that position.
/// The flat order is: active tracks, then shelved, then archived.
pub(super) fn tracks_cursor_track_id(app: &App) -> Option<String> {
    let mut ordered: Vec<&str> = Vec::new();
    for tc in &app.project.config.tracks {
        if tc.state == "active" {
            ordered.push(&tc.id);
        }
    }
    for tc in &app.project.config.tracks {
        if tc.state == "shelved" {
            ordered.push(&tc.id);
        }
    }
    for tc in &app.project.config.tracks {
        if tc.state == "archived" {
            ordered.push(&tc.id);
        }
    }
    ordered.get(app.tracks_cursor).map(|s| s.to_string())
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
pub(super) fn tracks_edit_name(app: &mut App) {
    let track_id = match tracks_cursor_track_id(app) {
        Some(id) => id,
        None => return,
    };
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

    let display_name = app.track_name(&track_id).to_string();
    app.confirm_state = Some(crate::tui::app::ConfirmState {
        message: format!("Delete track \"{}\"? [y/n]", display_name),
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
            let _ = app.save_track(&track_id);

            // Save all other affected tracks (those with updated dep references)
            let affected_tracks: Vec<String> = app
                .project
                .tracks
                .iter()
                .filter(|(tid, track)| tid != &track_id && has_dirty_tasks(track))
                .map(|(tid, _)| tid.clone())
                .collect();
            for tid in &affected_tracks {
                let _ = app.save_track(tid);
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
