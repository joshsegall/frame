use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::model::SectionKind;
use crate::model::task::Metadata;
use crate::ops::task_ops::{self, InsertPosition};
use crate::util::unicode;

use crate::tui::app::{
    App, AutocompleteKind, AutocompleteState, DetailRegion, EditHistory, EditTarget, FlatItem,
    Mode, RepeatEditRegion, RepeatableAction, View, resolve_task_from_flat,
};
use crate::tui::undo::Operation;
use crate::tui::wrap;

use super::*;

/// Toggle the `cc` tag on the task under the cursor (track view only).
pub(super) fn toggle_cc_tag(app: &mut App) {
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
    } else if let Some((track_id, task_id, _)) = app.cursor_task_id() {
        (track_id, task_id)
    } else {
        return;
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

    // Record repeatable action
    app.last_action = Some(RepeatableAction::ToggleCcTag);
}

/// Set the current track as cc-focus (track view or tracks view).
pub(super) fn set_cc_focus_current(app: &mut App) {
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

    let old_focus = app.project.config.agent.cc_focus.clone();

    // Toggle: if already cc-focus, clear it; otherwise set it
    if app.project.config.agent.cc_focus.as_deref() == Some(&track_id) {
        app.project.config.agent.cc_focus = None;
    } else {
        app.project.config.agent.cc_focus = Some(track_id.clone());
    }

    let new_focus = app.project.config.agent.cc_focus.clone();

    save_config(app);

    app.undo_stack.push(Operation::TrackCcFocus {
        old_focus,
        new_focus,
    });

    app.status_message = match &app.project.config.agent.cc_focus {
        Some(id) => Some(format!("cc-focus \u{25B6} {}", id)),
        None => Some("cc-focus cleared".to_string()),
    };
}

pub(super) enum AddPosition {
    Top,
    Bottom,
    AfterCursor,
}

/// Add a new task and enter EDIT mode for its title (track view only).
pub(super) fn add_task_action(app: &mut App, pos: AddPosition) {
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

    // For AfterCursor, check if cursor is on a child task — if so, insert a sibling subtask
    if matches!(pos, AddPosition::AfterCursor) {
        let flat_items = app.build_flat_items(&track_id);
        let cursor = app.track_states.get(&track_id).map_or(0, |s| s.cursor);
        if let Some(FlatItem::Task {
            section,
            path,
            depth,
            ..
        }) = flat_items.get(cursor)
            && *depth > 0
            && path.len() > 1
        {
            // Cursor is on a child task — find parent and insert sibling after this task
            let parent_path = &path[..path.len() - 1];
            let track_ref = match App::find_track_in_project(&app.project, &track_id) {
                Some(t) => t,
                None => return,
            };
            let parent_task = match resolve_task_from_flat(track_ref, *section, parent_path) {
                Some(t) => t,
                None => return,
            };
            let parent_id = match &parent_task.id {
                Some(id) => id.clone(),
                None => return,
            };
            let cursor_task = match resolve_task_from_flat(track_ref, *section, path) {
                Some(t) => t,
                None => return,
            };
            let sibling_id = match &cursor_task.id {
                Some(id) => id.clone(),
                None => return,
            };

            let track = match app.find_track_mut(&track_id) {
                Some(t) => t,
                None => return,
            };
            let sub_id =
                match task_ops::add_subtask_after(track, &parent_id, &sibling_id, String::new()) {
                    Ok(id) => id,
                    Err(_) => return,
                };

            // Enter EDIT mode for the new subtask's title
            app.edit_buffer.clear();
            app.edit_cursor = 0;
            app.edit_target = Some(EditTarget::NewTask {
                task_id: sub_id.clone(),
                track_id: track_id.clone(),
                parent_id: Some(parent_id),
            });
            app.pre_edit_cursor = saved_cursor;
            app.edit_history = Some(EditHistory::new("", 0, 0));
            app.edit_is_fresh = true;
            app.mode = Mode::Edit;

            move_cursor_to_task(app, &track_id, &sub_id);
            return;
        }
    }

    let insert_pos = match pos {
        AddPosition::Top => InsertPosition::Top,
        AddPosition::Bottom => InsertPosition::Bottom,
        AddPosition::AfterCursor => {
            // Insert after the cursor's top-level task
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
pub(super) fn add_subtask_action(app: &mut App) {
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
                if let FlatItem::Task { section, path, .. } = item
                    && let Some(task) = resolve_task_from_flat(track, *section, path)
                    && task.id.as_deref() == Some(&parent_id)
                {
                    let key = crate::tui::app::task_expand_key(task, *section, path);
                    let state = app.get_track_state(&track_id);
                    state.expanded.insert(key);
                    break;
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
    app.edit_is_fresh = true;
    app.mode = Mode::Edit;

    // Move cursor to the new subtask
    move_cursor_to_task(app, &track_id, &sub_id);
}

/// Append a new task at the end of the current sibling group.
/// Top-level → same as `a` (bottom of backlog). Subtask → new sibling at
/// the end of the parent's subtask list.
pub(super) fn append_sibling_action(app: &mut App) {
    let track_id = match app.current_track_id() {
        Some(id) => id.to_string(),
        None => return,
    };

    // Determine depth of cursor task
    let flat_items = app.build_flat_items(&track_id);
    let cursor = app.track_states.get(&track_id).map_or(0, |s| s.cursor);
    let (depth, section, path) = match flat_items.get(cursor) {
        Some(FlatItem::Task {
            depth,
            section,
            path,
            ..
        }) => (*depth, *section, path.clone()),
        _ => return,
    };

    if depth == 0 {
        // Top-level: same as `a`
        add_task_action(app, AddPosition::Bottom);
        return;
    }

    // Subtask: find the parent and append a new child at the end
    let parent_path = &path[..path.len() - 1];
    let track_ref = match App::find_track_in_project(&app.project, &track_id) {
        Some(t) => t,
        None => return,
    };
    let parent_task = match resolve_task_from_flat(track_ref, section, parent_path) {
        Some(t) => t,
        None => return,
    };
    let parent_id = match &parent_task.id {
        Some(id) => id.clone(),
        None => return,
    };

    // Save cursor position for restore on cancel
    let saved_cursor = app.track_states.get(&track_id).map(|s| s.cursor);

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
                if let FlatItem::Task { section, path, .. } = item
                    && let Some(task) = resolve_task_from_flat(track, *section, path)
                    && task.id.as_deref() == Some(&parent_id)
                {
                    let key = crate::tui::app::task_expand_key(task, *section, path);
                    let state = app.get_track_state(&track_id);
                    state.expanded.insert(key);
                    break;
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
    app.pre_edit_cursor = saved_cursor;
    app.edit_history = Some(EditHistory::new("", 0, 0));
    app.edit_is_fresh = true;
    app.mode = Mode::Edit;

    move_cursor_to_task(app, &track_id, &sub_id);
}

/// Outdent a fresh new subtask edit: cancel the current placeholder and insert
/// a new task one level up (as a sibling of the current parent). If the parent
/// is top-level, the new task becomes top-level. Called when `-` is the first
/// character typed in a new subtask edit.
pub(super) fn outdent_new_subtask(app: &mut App) {
    // Extract current edit target info
    let (task_id, track_id, parent_id) = match &app.edit_target {
        Some(EditTarget::NewTask {
            task_id,
            track_id,
            parent_id: Some(pid),
        }) => (task_id.clone(), track_id.clone(), pid.clone()),
        _ => return,
    };

    // Preserve the original pre_edit_cursor across outdent operations
    let saved_cursor = app.pre_edit_cursor;

    // Remove the current placeholder subtask from the parent
    if !app.track_changed_on_disk(&track_id) {
        let track = match app.find_track_mut(&track_id) {
            Some(t) => t,
            None => return,
        };
        if let Some(parent) = task_ops::find_task_mut_in_track(track, &parent_id) {
            parent
                .subtasks
                .retain(|t| t.id.as_deref() != Some(&task_id));
            parent.mark_dirty();
        }
    }

    // Find the parent task's position in the flat tree to determine its depth
    let flat_items = app.build_flat_items(&track_id);
    let track_ref = match App::find_track_in_project(&app.project, &track_id) {
        Some(t) => t,
        None => return,
    };

    let mut parent_depth = 0usize;
    let mut parent_path: Option<Vec<usize>> = None;
    let mut parent_section = SectionKind::Backlog;
    for item in &flat_items {
        if let FlatItem::Task {
            section,
            path,
            depth,
            ..
        } = item
            && let Some(task) = resolve_task_from_flat(track_ref, *section, path)
            && task.id.as_deref() == Some(&parent_id)
        {
            parent_depth = *depth;
            parent_path = Some(path.clone());
            parent_section = *section;
            break;
        }
    }

    let parent_path = match parent_path {
        Some(p) => p,
        None => return,
    };

    let prefix = match app.track_prefix(&track_id) {
        Some(p) => p.to_string(),
        None => return,
    };

    if parent_depth > 0 && parent_path.len() > 1 {
        // Parent is itself a subtask — insert as sibling of parent (under grandparent)
        let grandparent_path = &parent_path[..parent_path.len() - 1];
        let grandparent_task =
            match resolve_task_from_flat(track_ref, parent_section, grandparent_path) {
                Some(t) => t,
                None => return,
            };
        let grandparent_id = match &grandparent_task.id {
            Some(id) => id.clone(),
            None => return,
        };

        let track = match app.find_track_mut(&track_id) {
            Some(t) => t,
            None => return,
        };
        let new_id =
            match task_ops::add_subtask_after(track, &grandparent_id, &parent_id, String::new()) {
                Ok(id) => id,
                Err(_) => return,
            };

        // Enter EDIT mode for the new subtask (still has a parent, so edit_is_fresh stays true)
        app.edit_buffer.clear();
        app.edit_cursor = 0;
        app.edit_target = Some(EditTarget::NewTask {
            task_id: new_id.clone(),
            track_id: track_id.clone(),
            parent_id: Some(grandparent_id),
        });
        app.pre_edit_cursor = saved_cursor;
        app.edit_history = Some(EditHistory::new("", 0, 0));
        app.edit_is_fresh = true;
        app.mode = Mode::Edit;

        move_cursor_to_task(app, &track_id, &new_id);
    } else {
        // Parent is top-level — insert a new top-level task after the parent
        let track = match app.find_track_mut(&track_id) {
            Some(t) => t,
            None => return,
        };
        let new_id = match task_ops::add_task(
            track,
            String::new(),
            InsertPosition::After(parent_id),
            &prefix,
        ) {
            Ok(id) => id,
            Err(_) => return,
        };

        // Enter EDIT mode for the new top-level task (no parent, so `-` will be a normal char)
        app.edit_buffer.clear();
        app.edit_cursor = 0;
        app.edit_target = Some(EditTarget::NewTask {
            task_id: new_id.clone(),
            track_id: track_id.clone(),
            parent_id: None,
        });
        app.pre_edit_cursor = saved_cursor;
        app.edit_history = Some(EditHistory::new("", 0, 0));
        app.edit_is_fresh = false;
        app.mode = Mode::Edit;

        move_cursor_to_task(app, &track_id, &new_id);
    }
}

// ---------------------------------------------------------------------------
// Inline title editing

/// Enter EDIT mode to edit the selected task's title.
pub(super) fn enter_title_edit(app: &mut App) {
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

pub(super) fn enter_tag_edit(app: &mut App) {
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

    let original_tags = task
        .tags
        .iter()
        .map(|t| format!("#{}", t))
        .collect::<Vec<_>>()
        .join(" ");
    app.edit_buffer = if original_tags.is_empty() {
        String::new()
    } else {
        format!("{} ", original_tags)
    };
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

pub(super) fn handle_edit(app: &mut App, key: KeyEvent) {
    // Check if we're in multi-line note editing in detail view
    let is_detail_multiline = app
        .detail_state
        .as_ref()
        .is_some_and(|ds| ds.editing && ds.region == DetailRegion::Note)
        && app.edit_target.is_none();

    if is_detail_multiline {
        handle_detail_multiline_edit(app, key);
        return;
    }

    // Check if we're editing a detail region (single-line)
    let is_detail_edit = matches!(app.view, View::Detail { .. })
        && app.detail_state.as_ref().is_some_and(|ds| ds.editing);

    // Handle autocomplete navigation when dropdown is visible
    let ac_visible = app
        .autocomplete
        .as_ref()
        .is_some_and(|ac| ac.visible && !ac.filtered.is_empty());

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
            // For FilterTag: Esc cancels the entire filter tag selection
            (_, KeyCode::Esc) => {
                if matches!(
                    app.edit_target,
                    Some(EditTarget::FilterTag) | Some(EditTarget::JumpTo)
                ) {
                    app.autocomplete = None;
                    app.edit_history = None;
                    app.edit_selection_anchor = None;
                    cancel_edit(app);
                    return;
                }
                if let Some(ac) = &mut app.autocomplete {
                    ac.visible = false;
                }
                return;
            }
            // Enter: accept autocomplete selection and confirm edit
            (_, KeyCode::Enter) => {
                // Accept autocomplete if a candidate is selected AND the user is
                // actually completing a partial word (not just confirming an already-typed entry)
                if let Some(ac) = &app.autocomplete
                    && let Some(entry) = ac.selected_entry()
                {
                    let filter = autocomplete_filter_text(&app.edit_buffer, ac.kind);
                    if filter != entry {
                        autocomplete_accept(app);
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
        // Cancel edit (or clear selection first)
        (_, KeyCode::Esc) => {
            if app.edit_selection_anchor.is_some() {
                app.edit_selection_anchor = None;
                return;
            }
            app.autocomplete = None;
            app.edit_history = None;
            if is_detail_edit {
                cancel_detail_edit(app);
            } else {
                cancel_edit(app);
            }
        }
        // Home / Ctrl+A (macOS Cmd+Left sends ^A): jump to start of line
        (m, KeyCode::Char('a')) if m.contains(KeyModifiers::CONTROL) => {
            app.edit_selection_anchor = None;
            app.edit_cursor = 0;
        }
        // End / Ctrl+E (macOS Cmd+Right sends ^E): jump to end of line
        (m, KeyCode::Char('e')) if m.contains(KeyModifiers::CONTROL) => {
            app.edit_selection_anchor = None;
            app.edit_cursor = app.edit_buffer.len();
        }
        // Kill to start of line: Ctrl+U (macOS Cmd+Backspace sends ^U)
        (m, KeyCode::Char('u')) if m.contains(KeyModifiers::CONTROL) => {
            if app.edit_selection_anchor.is_some() {
                app.delete_selection();
            } else if app.edit_cursor > 0 {
                app.edit_buffer.drain(..app.edit_cursor);
                app.edit_cursor = 0;
            }
            app.edit_selection_anchor = None;
            if let Some(eh) = &mut app.edit_history {
                eh.snapshot(&app.edit_buffer, app.edit_cursor, 0);
            }
            update_autocomplete_filter(app);
        }
        // Copy (Ctrl+C or Super+C)
        (m, KeyCode::Char('c'))
            if m.contains(KeyModifiers::CONTROL) || m.contains(KeyModifiers::SUPER) =>
        {
            if let Some(text) = app.get_selection_text() {
                clipboard_set(&text);
            }
        }
        // Cut (Ctrl+X or Super+X)
        (m, KeyCode::Char('x'))
            if m.contains(KeyModifiers::CONTROL) || m.contains(KeyModifiers::SUPER) =>
        {
            if let Some(text) = app.get_selection_text() {
                clipboard_set(&text);
                app.delete_selection();
                if let Some(eh) = &mut app.edit_history {
                    eh.snapshot(&app.edit_buffer, app.edit_cursor, 0);
                }
                update_autocomplete_filter(app);
            }
        }
        // Paste (Ctrl+V or Super+V)
        (m, KeyCode::Char('v'))
            if m.contains(KeyModifiers::CONTROL) || m.contains(KeyModifiers::SUPER) =>
        {
            if let Some(text) = clipboard_get() {
                // Capture current cursor before mutating (may be stale from arrow keys)
                if let Some(eh) = &mut app.edit_history {
                    eh.snapshot(&app.edit_buffer, app.edit_cursor, 0);
                }
                app.delete_selection();
                app.edit_buffer.insert_str(app.edit_cursor, &text);
                app.edit_cursor += text.len();
                if let Some(eh) = &mut app.edit_history {
                    eh.snapshot(&app.edit_buffer, app.edit_cursor, 0);
                }
                update_autocomplete_filter(app);
            }
        }
        // Inline undo (Ctrl+Z or Super+Z)
        (m, KeyCode::Char('z'))
            if m.contains(KeyModifiers::CONTROL) || m.contains(KeyModifiers::SUPER) =>
        {
            app.edit_selection_anchor = None;
            if let Some(eh) = &mut app.edit_history
                && let Some((buf, pos, _)) = eh.undo()
            {
                app.edit_buffer = buf.to_string();
                app.edit_cursor = pos;
            }
            update_autocomplete_filter(app);
        }
        // Inline redo (Ctrl+Y, Ctrl+Shift+Z, or Super+Shift+Z)
        (m, KeyCode::Char('y')) if m.contains(KeyModifiers::CONTROL) => {
            app.edit_selection_anchor = None;
            if let Some(eh) = &mut app.edit_history
                && let Some((buf, pos, _)) = eh.redo()
            {
                app.edit_buffer = buf.to_string();
                app.edit_cursor = pos;
            }
            update_autocomplete_filter(app);
        }
        (m, KeyCode::Char('Z'))
            if (m.contains(KeyModifiers::CONTROL) || m.contains(KeyModifiers::SUPER))
                && m.contains(KeyModifiers::SHIFT) =>
        {
            app.edit_selection_anchor = None;
            if let Some(eh) = &mut app.edit_history
                && let Some((buf, pos, _)) = eh.redo()
            {
                app.edit_buffer = buf.to_string();
                app.edit_cursor = pos;
            }
            update_autocomplete_filter(app);
        }
        // Cursor movement: single character left/right (with or without Shift)
        (_, KeyCode::Left)
            if !key.modifiers.contains(KeyModifiers::ALT)
                && !key.modifiers.contains(KeyModifiers::CONTROL)
                && !key.modifiers.contains(KeyModifiers::SUPER) =>
        {
            if let Some(prev) = unicode::prev_grapheme_boundary(&app.edit_buffer, app.edit_cursor) {
                app.edit_cursor = prev;
            }
        }
        (_, KeyCode::Right)
            if !key.modifiers.contains(KeyModifiers::ALT)
                && !key.modifiers.contains(KeyModifiers::CONTROL)
                && !key.modifiers.contains(KeyModifiers::SUPER) =>
        {
            if let Some(next) = unicode::next_grapheme_boundary(&app.edit_buffer, app.edit_cursor) {
                app.edit_cursor = next;
            }
        }
        // Jump to start/end of line (Cmd/Super, with or without Shift)
        (m, KeyCode::Left) if m.contains(KeyModifiers::SUPER) => {
            app.edit_cursor = 0;
        }
        (m, KeyCode::Right) if m.contains(KeyModifiers::SUPER) => {
            app.edit_cursor = app.edit_buffer.len();
        }
        // Ctrl+Left/Right: jump to start/end of line (Ctrl+arrow in terminals)
        (m, KeyCode::Left)
            if m.contains(KeyModifiers::CONTROL) && !m.contains(KeyModifiers::ALT) =>
        {
            app.edit_selection_anchor = None;
            app.edit_cursor = 0;
        }
        (m, KeyCode::Right)
            if m.contains(KeyModifiers::CONTROL) && !m.contains(KeyModifiers::ALT) =>
        {
            app.edit_selection_anchor = None;
            app.edit_cursor = app.edit_buffer.len();
        }
        // Home/End keys: jump to start/end of line
        (_, KeyCode::Home) => {
            app.edit_selection_anchor = None;
            app.edit_cursor = 0;
        }
        (_, KeyCode::End) => {
            app.edit_selection_anchor = None;
            app.edit_cursor = app.edit_buffer.len();
        }
        // Word movement (Alt+arrow, with or without Shift)
        (m, KeyCode::Left) if m.contains(KeyModifiers::ALT) => {
            app.edit_cursor = word_boundary_left(&app.edit_buffer, app.edit_cursor);
        }
        (m, KeyCode::Right) if m.contains(KeyModifiers::ALT) => {
            app.edit_cursor = word_boundary_right(&app.edit_buffer, app.edit_cursor);
        }
        // Readline word movement: Alt+B (backward) / Alt+F (forward)
        // Warp and other terminals translate Alt+Left/Right to these
        (m, KeyCode::Char('b')) if m.contains(KeyModifiers::ALT) => {
            app.edit_cursor = word_boundary_left(&app.edit_buffer, app.edit_cursor);
        }
        (m, KeyCode::Char('f')) if m.contains(KeyModifiers::ALT) => {
            app.edit_cursor = word_boundary_right(&app.edit_buffer, app.edit_cursor);
        }
        // Backspace: delete selection or single char
        (KeyModifiers::NONE, KeyCode::Backspace) => {
            if !app.delete_selection()
                && let Some(prev) =
                    unicode::prev_grapheme_boundary(&app.edit_buffer, app.edit_cursor)
            {
                app.edit_buffer.drain(prev..app.edit_cursor);
                app.edit_cursor = prev;
            }
            if let Some(eh) = &mut app.edit_history {
                eh.snapshot(&app.edit_buffer, app.edit_cursor, 0);
            }
            update_prefix_validation(app);
            update_autocomplete_filter(app);
        }
        // Word backspace (Alt or Ctrl)
        (m, KeyCode::Backspace)
            if m.contains(KeyModifiers::ALT) || m.contains(KeyModifiers::CONTROL) =>
        {
            if !app.delete_selection() {
                let new_pos = word_boundary_left(&app.edit_buffer, app.edit_cursor);
                app.edit_buffer.drain(new_pos..app.edit_cursor);
                app.edit_cursor = new_pos;
            }
            if let Some(eh) = &mut app.edit_history {
                eh.snapshot(&app.edit_buffer, app.edit_cursor, 0);
            }
            update_prefix_validation(app);
            update_autocomplete_filter(app);
        }
        // Type character: replace selection if any
        (KeyModifiers::NONE | KeyModifiers::SHIFT, KeyCode::Char(c)) => {
            // Outdent: `-` as first keystroke on a fresh new subtask edit
            if c == '-'
                && app.edit_is_fresh
                && matches!(
                    &app.edit_target,
                    Some(EditTarget::NewTask {
                        parent_id: Some(_),
                        ..
                    })
                )
            {
                outdent_new_subtask(app);
                return;
            }
            app.edit_is_fresh = false;
            app.delete_selection();
            // Auto-uppercase for prefix editing
            let c = if matches!(app.edit_target, Some(EditTarget::ExistingPrefix { .. })) {
                c.to_ascii_uppercase()
            } else {
                c
            };
            app.edit_buffer.insert(app.edit_cursor, c);
            app.edit_cursor += c.len_utf8();
            if let Some(eh) = &mut app.edit_history {
                eh.snapshot(&app.edit_buffer, app.edit_cursor, 0);
            }
            // Update prefix validation on each keystroke
            if let Some(EditTarget::ExistingPrefix { ref track_id, .. }) = app.edit_target {
                let tid = track_id.clone();
                if let Some(ref mut pr) = app.prefix_rename {
                    pr.validation_error =
                        validate_prefix(&app.edit_buffer, &tid, &app.project.config);
                }
            }
            update_autocomplete_filter(app);
        }
        _ => {}
    }

    // Update horizontal scroll for single-line edits in detail view
    if matches!(app.view, View::Detail { .. })
        && !app
            .detail_state
            .as_ref()
            .is_some_and(|ds| ds.editing && ds.region == DetailRegion::Note)
    {
        update_edit_h_scroll(app);
    }
}

/// Keep the cursor visible within the horizontal scroll viewport for single-line edits.
pub(super) fn update_edit_h_scroll(app: &mut App) {
    let width = app.last_edit_available_width as usize;
    if width == 0 {
        return;
    }
    let cursor_col = unicode::byte_offset_to_display_col(
        &app.edit_buffer,
        app.edit_cursor.min(app.edit_buffer.len()),
    );
    let margin = 10.min(width / 3);
    let total = unicode::display_width(&app.edit_buffer);
    // When cursor is at end, the cursor block needs one extra column
    let content_end = if cursor_col >= total {
        total + 1
    } else {
        total
    };

    // Scroll right: cursor approaching right edge
    if cursor_col >= app.edit_h_scroll + width.saturating_sub(margin) {
        app.edit_h_scroll = cursor_col.saturating_sub(width.saturating_sub(margin + 1));
    }
    // Clamp: don't scroll past content end
    app.edit_h_scroll = app
        .edit_h_scroll
        .min(content_end.saturating_sub(width.saturating_sub(1)));
    // Scroll left: cursor approaching left edge
    if cursor_col < app.edit_h_scroll + margin {
        app.edit_h_scroll = cursor_col.saturating_sub(margin);
    }
}

pub(super) fn confirm_edit(app: &mut App) {
    let target = match app.edit_target.take() {
        Some(t) => t,
        None => {
            app.mode = if app.selection.is_empty() {
                Mode::Navigate
            } else {
                Mode::Select
            };
            return;
        }
    };

    let title = app.edit_buffer.clone();
    app.mode = if app.selection.is_empty() {
        Mode::Navigate
    } else {
        Mode::Select
    };
    app.pre_edit_cursor = None;
    app.edit_is_fresh = false;

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
                            parent
                                .subtasks
                                .retain(|t| t.id.as_deref() != Some(&task_id));
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
                            title: title.clone(),
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
                            title: title.clone(),
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
                        title: title.clone(),
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
                        title: title.clone(),
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

                            // Record repeatable action
                            app.last_action =
                                Some(RepeatableAction::EnterEdit(RepeatEditRegion::Title));
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

                    // Record repeatable action
                    app.last_action = Some(RepeatableAction::EnterEdit(RepeatEditRegion::Title));
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

            // Compute tag diff for repeat
            let old_tag_set: Vec<String> = original_tags
                .split_whitespace()
                .map(|s| s.strip_prefix('#').unwrap_or(s).to_string())
                .filter(|s| !s.is_empty())
                .collect();
            let tag_adds: Vec<String> = new_tags
                .iter()
                .filter(|t| !old_tag_set.contains(t))
                .cloned()
                .collect();
            let tag_removes: Vec<String> = old_tag_set
                .iter()
                .filter(|t| !new_tags.contains(t))
                .cloned()
                .collect();

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

                // Record repeatable action
                if !tag_adds.is_empty() || !tag_removes.is_empty() {
                    app.last_action = Some(RepeatableAction::TagEdit {
                        adds: tag_adds,
                        removes: tag_removes,
                    });
                }
            }
            drain_pending_for_track(app, &track_id);
        }
        EditTarget::NewInboxItem { index } => {
            let title = app.edit_buffer.clone();
            if title.trim().is_empty() {
                // Empty title: discard the placeholder
                if let Some(inbox) = &mut app.project.inbox
                    && index < inbox.items.len()
                {
                    inbox.items.remove(index);
                }
            } else {
                // Apply title to the inbox item
                if let Some(inbox) = &mut app.project.inbox {
                    if let Some(item) = inbox.items.get_mut(index) {
                        item.title = title.clone();
                        item.dirty = true;
                    }
                    app.undo_stack.push(Operation::InboxAdd { index, title });
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
                if let Some(inbox) = &mut app.project.inbox
                    && let Some(item) = inbox.items.get_mut(index)
                {
                    item.title = new_title.clone();
                    item.dirty = true;
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

            if let Some(inbox) = &mut app.project.inbox
                && let Some(item) = inbox.items.get_mut(index)
            {
                item.tags = new_tags.clone();
                item.dirty = true;
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
        EditTarget::NewTrackName => {
            let name = app.edit_buffer.clone();
            if name.trim().is_empty() {
                // Empty name: cancelled
                app.new_track_insert_pos = None;
                return;
            }
            let track_id = crate::ops::track_ops::generate_track_id(&name);
            if track_id.is_empty() {
                app.status_message = Some("invalid track name".to_string());
                return;
            }
            // Check for ID collision
            if app.project.config.tracks.iter().any(|tc| tc.id == track_id) {
                app.status_message = Some(format!("track \"{}\" already exists", name));
                return;
            }

            // Generate prefix from ID
            let existing_prefixes: Vec<String> =
                app.project.config.ids.prefixes.values().cloned().collect();
            let prefix = crate::ops::track_ops::generate_prefix(&track_id, &existing_prefixes);

            // Create track file and add to config
            let tc = crate::model::TrackConfig {
                id: track_id.clone(),
                name: name.clone(),
                state: "active".to_string(),
                file: format!("tracks/{}.md", track_id),
            };

            // Write track file
            let track_content = format!("# {}\n\n## Backlog\n\n## Done\n", name);
            let track_path = app.project.frame_dir.join(&tc.file);
            let _ = crate::io::recovery::atomic_write(&track_path, track_content.as_bytes());

            // Add to config — insert among active tracks at the stored position
            // so that p/- placement is respected (a/= use active_count = end).
            let insert_pos = app.new_track_insert_pos.take().unwrap_or(usize::MAX);
            let active_indices: Vec<usize> = app
                .project
                .config
                .tracks
                .iter()
                .enumerate()
                .filter(|(_, t)| t.state == "active")
                .map(|(i, _)| i)
                .collect();
            let insert_config_idx = if insert_pos < active_indices.len() {
                active_indices[insert_pos]
            } else {
                // After last active track (or end if no active tracks)
                active_indices.last().map_or(0, |&last| last + 1)
            };
            app.project.config.tracks.insert(insert_config_idx, tc);
            app.project
                .config
                .ids
                .prefixes
                .insert(track_id.clone(), prefix);
            save_config(app);

            // Load the new track into memory
            if let Ok(text) = std::fs::read_to_string(&track_path) {
                let track = crate::parse::parse_track(&text);
                app.project.tracks.push((track_id.clone(), track));
            }

            rebuild_active_track_ids(app);

            app.undo_stack.push(Operation::TrackAdd {
                track_id: track_id.clone(),
            });

            // Move cursor to the new track
            if let Some(pos) = tracks_find_cursor_pos(app, &track_id) {
                app.tracks_cursor = pos;
            }

            app.status_message = Some(format!("created track \"{}\"", name));
        }
        EditTarget::ExistingTrackName {
            track_id,
            original_name,
        } => {
            let new_name = app.edit_buffer.clone();
            if new_name.trim().is_empty() || new_name == original_name {
                return;
            }

            // Update config name
            if let Some(tc) = app
                .project
                .config
                .tracks
                .iter_mut()
                .find(|t| t.id == track_id)
            {
                tc.name = new_name.clone();
            }
            save_config(app);

            // Update track file header (first line: "# Name")
            update_track_header(app, &track_id, &new_name);
            let _ = app.save_track(&track_id);

            app.undo_stack.push(Operation::TrackNameEdit {
                track_id: track_id.clone(),
                old_name: original_name,
                new_name: new_name.clone(),
            });

            app.status_message = Some(format!("renamed → \"{}\"", new_name));
        }
        EditTarget::FilterTag => {
            // Accept the tag from the edit buffer (may have been selected from autocomplete)
            let tag_text = app.edit_buffer.clone();
            let tag = tag_text
                .trim()
                .strip_prefix('#')
                .unwrap_or(tag_text.trim())
                .to_string();
            if !tag.is_empty() {
                let prev_task_id = get_cursor_task_id(app);
                app.filter_state.tag_filter = Some(tag);
                reset_cursor_for_filter(app, prev_task_id.as_deref());
            }
        }
        EditTarget::BulkTags => {
            confirm_bulk_tag_edit(app);
        }
        EditTarget::BulkDeps => {
            confirm_bulk_dep_edit(app);
        }
        EditTarget::JumpTo => {
            // Extract the task ID (from buffer or autocomplete selection)
            let task_id = app.edit_buffer.trim().to_string();
            // If the buffer looks like "ID  title" (from autocomplete), extract just the ID
            let task_id = task_id.split_whitespace().next().unwrap_or("").to_string();
            if !task_id.is_empty() && !app.jump_to_task(&task_id) {
                app.status_message = Some(format!("task {} not found", task_id));
                app.status_is_error = true;
            }
        }
        EditTarget::ExistingPrefix {
            track_id,
            original_prefix,
        } => {
            let new_prefix = app.edit_buffer.clone();

            // Same as current → no-op
            if new_prefix == original_prefix {
                app.prefix_rename = None;
                return;
            }

            // Validate
            let error = validate_prefix(&new_prefix, &track_id, &app.project.config);
            if !error.is_empty() {
                // Don't confirm — put the target back and stay in Edit mode
                app.edit_target = Some(EditTarget::ExistingPrefix {
                    track_id,
                    original_prefix,
                });
                app.mode = Mode::Edit;
                return;
            }

            // Compute blast radius and transition to confirmation
            let archive_dir = app.project.frame_dir.join("archive");
            let archive_opt = if archive_dir.exists() {
                Some(archive_dir.as_path())
            } else {
                None
            };
            let impact = crate::ops::track_ops::prefix_rename_impact(
                &app.project.tracks,
                &track_id,
                &original_prefix,
                archive_opt,
            );

            if let Some(ref mut pr) = app.prefix_rename {
                pr.new_prefix = new_prefix;
                pr.confirming = true;
                pr.task_id_count = impact.task_id_count;
                pr.dep_ref_count = impact.dep_ref_count;
                pr.affected_track_count = impact.affected_track_count;
            }

            // Stay in Navigate mode — the confirmation popup renders as an overlay
            // and intercepts Enter/Esc in handle_navigate
        }
        EditTarget::ImportFilePath { track_id } => {
            let file_path = app.edit_buffer.trim().to_string();
            if file_path.is_empty() {
                return;
            }

            // Try to read and parse the file
            let content = match std::fs::read_to_string(&file_path) {
                Ok(c) => c,
                Err(e) => {
                    app.status_message = Some(format!("Cannot read file: {}", e));
                    app.status_is_error = true;
                    return;
                }
            };

            // Parse to count tasks
            let parsed = crate::parse::parse_track(&content);
            let task_count: usize = parsed
                .backlog()
                .iter()
                .map(task_ops::count_subtree_size)
                .sum();

            if task_count == 0 {
                app.status_message = Some("No tasks found in file".into());
                app.status_is_error = true;
                return;
            }

            let track_name = app.track_name(&track_id).to_string();
            app.confirm_state = Some(crate::tui::app::ConfirmState {
                message: format!(
                    "Import {} tasks from \"{}\" into {}? (y/n)",
                    task_count, file_path, track_name,
                ),
                action: crate::tui::app::ConfirmAction::ImportTasks {
                    track_id,
                    file_path,
                },
            });
            app.mode = Mode::Confirm;
        }
    }
}

pub(super) fn cancel_edit(app: &mut App) {
    let target = app.edit_target.take();
    let saved_cursor = app.pre_edit_cursor.take();
    app.mode = if app.selection.is_empty() {
        Mode::Navigate
    } else {
        Mode::Select
    };
    app.autocomplete = None;
    app.edit_is_fresh = false;

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
                        parent
                            .subtasks
                            .retain(|t| t.id.as_deref() != Some(&task_id));
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
            if let Some(inbox) = &mut app.project.inbox
                && index < inbox.items.len()
            {
                inbox.items.remove(index);
            }
            // Restore cursor
            if let Some(cursor) = saved_cursor {
                app.inbox_cursor = cursor;
            }
        }
        // New track add — just restore cursor (no placeholder to remove)
        Some(EditTarget::NewTrackName) => {
            app.new_track_insert_pos = None;
            if let Some(cursor) = saved_cursor {
                app.tracks_cursor = cursor;
            }
        }
        // FilterTag: cancel clears the tag filter
        Some(EditTarget::FilterTag) => {
            let prev_task_id = get_cursor_task_id(app);
            app.filter_state.tag_filter = None;
            reset_cursor_for_filter(app, prev_task_id.as_deref());
        }
        // BulkTags/BulkDeps: cancel just returns to Select mode (no cleanup needed)
        Some(EditTarget::BulkTags) | Some(EditTarget::BulkDeps) => {
            // Selection persists, mode already set to Select above
        }
        // JumpTo: cancel just returns to previous mode (no cleanup needed)
        Some(EditTarget::JumpTo) => {}
        // Prefix edit: cancel clears the prefix rename state
        Some(EditTarget::ExistingPrefix { .. }) => {
            app.prefix_rename = None;
        }
        // For existing title/tags edit, cancel means revert (unchanged since we didn't write)
        _ => {}
    }
}

/// Move between regions in the detail view (up/down)
pub(super) fn detail_move_region(app: &mut App, delta: i32) {
    let ds = match &mut app.detail_state {
        Some(ds) => ds,
        None => return,
    };

    if ds.regions.is_empty() {
        return;
    }

    let current_idx = ds.regions.iter().position(|r| *r == ds.region).unwrap_or(0);

    // Special handling when on Subtasks region with subtasks
    let has_note_region = ds.regions.contains(&DetailRegion::Note);
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
            // At first subtask: if Note region exists, go back to Note with
            // note_view_line at the end of note content (for scroll continuity)
            if has_note_region {
                ds.region = DetailRegion::Note;
                ds.note_view_line = Some(ds.note_content_end);
                return;
            }
            // No note region — move to previous region normally
            let new_idx = current_idx.saturating_sub(1);
            ds.region = ds.regions[new_idx];
            return;
        }
    }

    // Note region: j/k move a virtual cursor by 8 lines through note content.
    // The renderer places the indicator at note_view_line and scroll follows it.
    // When scrolling past note content, transition to Subtasks region.
    let has_subtasks_region = ds.regions.contains(&DetailRegion::Subtasks);
    if ds.region == DetailRegion::Note && !ds.editing {
        let note_header = ds.note_header_line.unwrap_or(0);
        let note_end = ds.note_content_end;
        let current_vl = ds.note_view_line.unwrap_or(note_header);

        if delta > 0 {
            let new_vl = current_vl + 8;
            if new_vl > note_end && has_subtasks_region {
                // Past note content — transition to Subtasks region
                ds.note_view_line = None;
                ds.region = DetailRegion::Subtasks;
                ds.subtask_cursor = 0;
                return;
            }
            // Clamp to note content end
            let clamped = new_vl.min(note_end);
            if clamped > current_vl {
                ds.note_view_line = Some(clamped);
                return;
            }
            // Already at end and no subtasks — stay put
            return;
        } else if delta < 0 {
            if current_vl > note_header {
                let new_vl = current_vl.saturating_sub(8).max(note_header);
                ds.note_view_line = Some(new_vl);
                return;
            }
            // At note header — reset virtual cursor and fall through to prev region
            ds.note_view_line = None;
        }
    }

    let new_idx = (current_idx as i32 + delta).clamp(0, ds.regions.len() as i32 - 1) as usize;
    let new_region = ds.regions[new_idx];

    // Reset note_view_line when leaving Note region
    if ds.region == DetailRegion::Note && new_region != DetailRegion::Note {
        ds.note_view_line = None;
    }

    ds.region = new_region;

    // When entering Subtasks from another region via Down, reset subtask_cursor
    if ds.region == DetailRegion::Subtasks && delta > 0 {
        ds.subtask_cursor = 0;
    }
}

/// Jump to next/prev region in the detail view (Tab/Shift+Tab)
pub(super) fn detail_jump_editable(app: &mut App, direction: i32) {
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
        let new_idx = if current_idx == 0 {
            len - 1
        } else {
            current_idx - 1
        };
        ds.region = ds.regions[new_idx];
    }

    // Reset subtask_cursor when landing on Subtasks
    if ds.region == DetailRegion::Subtasks {
        ds.subtask_cursor = 0;
    }
}

/// Enter EDIT mode on the current region in the detail view.
/// When `cursor_at_end` is true, the cursor starts at the end of multiline content (notes).
pub(super) fn detail_enter_edit(app: &mut App, cursor_at_end: bool) {
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
            let tag_str = task
                .tags
                .iter()
                .map(|t| format!("#{}", t))
                .collect::<Vec<_>>()
                .join(" ");
            (tag_str, false)
        }
        DetailRegion::Deps => {
            let deps: Vec<String> = task
                .metadata
                .iter()
                .flat_map(|m| {
                    if let Metadata::Dep(d) = m {
                        d.clone()
                    } else {
                        Vec::new()
                    }
                })
                .collect();
            (deps.join(", "), false)
        }
        DetailRegion::Spec => {
            let spec = task
                .metadata
                .iter()
                .find_map(|m| {
                    if let Metadata::Spec(s) = m {
                        Some(s.clone())
                    } else {
                        None
                    }
                })
                .unwrap_or_default();
            (spec, false)
        }
        DetailRegion::Refs => {
            let refs: Vec<String> = task
                .metadata
                .iter()
                .flat_map(|m| {
                    if let Metadata::Ref(r) = m {
                        r.clone()
                    } else {
                        Vec::new()
                    }
                })
                .collect();
            (refs.join(" "), false)
        }
        DetailRegion::Note => {
            let note = task
                .metadata
                .iter()
                .find_map(|m| {
                    if let Metadata::Note(n) = m {
                        Some(n.clone())
                    } else {
                        None
                    }
                })
                .unwrap_or_default();
            (note, true)
        }
        _ => return,
    };

    if is_multiline {
        // Multi-line editing (note): use detail_state's edit fields
        let (cursor_line, cursor_col) = if cursor_at_end {
            let line_count = initial_value.split('\n').count();
            let last_line_len = initial_value.split('\n').next_back().map_or(0, |l| l.len());
            (line_count.saturating_sub(1), last_line_len)
        } else {
            (0, 0)
        };
        if let Some(ds) = &mut app.detail_state {
            ds.editing = true;
            ds.note_h_scroll = 0;
            ds.note_view_line = None;
            ds.edit_buffer = initial_value.clone();
            ds.edit_cursor_line = cursor_line;
            ds.edit_cursor_col = cursor_col;
            ds.edit_original = initial_value.clone();
        }
        app.edit_history = Some(EditHistory::new(&initial_value, cursor_col, cursor_line));
        app.mode = Mode::Edit;
    } else {
        // Single-line editing: use the existing edit_buffer/edit_cursor on App
        app.edit_buffer = initial_value.clone();
        app.edit_cursor = app.edit_buffer.len();
        app.edit_h_scroll = 0;
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

/// Jump to a specific region and enter EDIT mode (for #, @, d, n/N shortcuts).
/// When `cursor_at_end` is true, the cursor starts at the end of multiline content.
pub(super) fn detail_jump_to_region_and_edit(
    app: &mut App,
    target_region: DetailRegion,
    cursor_at_end: bool,
) {
    if let Some(ds) = &mut app.detail_state {
        ds.region = target_region;
    }
    detail_enter_edit(app, cursor_at_end);
}

/// Handle multi-line editing (note field) in detail view
pub(super) fn handle_detail_multiline_edit(app: &mut App, key: KeyEvent) {
    // Selection pre-pass: manage multiline_selection_anchor for movement keys
    let has_shift = key.modifiers.contains(KeyModifiers::SHIFT);
    let is_movement = matches!(
        key.code,
        KeyCode::Left | KeyCode::Right | KeyCode::Up | KeyCode::Down
    );
    if is_movement && let Some(ds) = &mut app.detail_state {
        if has_shift {
            if ds.multiline_selection_anchor.is_none() {
                ds.multiline_selection_anchor = Some((ds.edit_cursor_line, ds.edit_cursor_col));
            }
        } else {
            ds.multiline_selection_anchor = None;
        }
    }

    match (key.modifiers, key.code) {
        // Esc: clear selection first, or finish editing (save)
        (_, KeyCode::Esc) => {
            if let Some(ds) = &mut app.detail_state
                && ds.multiline_selection_anchor.is_some()
            {
                ds.multiline_selection_anchor = None;
                return;
            }
            app.edit_history = None;
            confirm_detail_multiline(app);
        }
        // Home / Ctrl+A (macOS Cmd+Left sends ^A): jump to start of current visual row
        (m, KeyCode::Char('a')) if m.contains(KeyModifiers::CONTROL) => {
            if let Some(ds) = &mut app.detail_state {
                ds.multiline_selection_anchor = None;
                ds.sticky_col = None;
                if app.note_wrap {
                    let edit_lines: Vec<&str> = ds.edit_buffer.split('\n').collect();
                    let note_width = app.last_edit_available_width as usize;
                    if note_width > 0 {
                        let vls = wrap::wrap_lines_for_edit(
                            &edit_lines,
                            note_width,
                            ds.edit_cursor_line,
                            ds.edit_cursor_col,
                        );
                        let row = wrap::logical_to_visual_row(
                            &vls,
                            ds.edit_cursor_line,
                            ds.edit_cursor_col,
                        );
                        if let Some(vl) = vls.get(row) {
                            ds.edit_cursor_col = vl.char_start;
                        }
                    }
                } else {
                    ds.edit_cursor_col = 0;
                }
            }
        }
        // End / Ctrl+E (macOS Cmd+Right sends ^E): jump to end of current visual row
        (m, KeyCode::Char('e')) if m.contains(KeyModifiers::CONTROL) => {
            if let Some(ds) = &mut app.detail_state {
                ds.multiline_selection_anchor = None;
                ds.sticky_col = None;
                let edit_lines: Vec<&str> = ds.edit_buffer.split('\n').collect();
                if app.note_wrap {
                    let note_width = app.last_edit_available_width as usize;
                    if note_width > 0 {
                        let vls = wrap::wrap_lines_for_edit(
                            &edit_lines,
                            note_width,
                            ds.edit_cursor_line,
                            ds.edit_cursor_col,
                        );
                        let row = wrap::logical_to_visual_row(
                            &vls,
                            ds.edit_cursor_line,
                            ds.edit_cursor_col,
                        );
                        if let Some(vl) = vls.get(row) {
                            ds.edit_cursor_col = vl.char_end;
                        }
                    }
                } else {
                    let line_len = edit_lines.get(ds.edit_cursor_line).map_or(0, |l| l.len());
                    ds.edit_cursor_col = line_len;
                }
            }
        }
        // Kill to start of line: Ctrl+U (macOS Cmd+Backspace sends ^U)
        (m, KeyCode::Char('u')) if m.contains(KeyModifiers::CONTROL) => {
            if let Some(ds) = &mut app.detail_state {
                if ds.multiline_selection_anchor.is_some() {
                    delete_multiline_selection(ds);
                } else if ds.edit_cursor_col > 0 {
                    let mut edit_lines: Vec<String> =
                        ds.edit_buffer.split('\n').map(String::from).collect();
                    if let Some(line) = edit_lines.get_mut(ds.edit_cursor_line) {
                        line.drain(..ds.edit_cursor_col);
                    }
                    ds.edit_cursor_col = 0;
                    ds.edit_buffer = edit_lines.join("\n");
                }
                ds.multiline_selection_anchor = None;
            }
            snapshot_multiline(app);
        }
        // Copy (Ctrl+C or Super+C)
        (m, KeyCode::Char('c'))
            if m.contains(KeyModifiers::CONTROL) || m.contains(KeyModifiers::SUPER) =>
        {
            if let Some(ds) = &app.detail_state
                && let Some(text) = get_multiline_selection_text(ds)
            {
                clipboard_set(&text);
            }
        }
        // Cut (Ctrl+X or Super+X)
        (m, KeyCode::Char('x'))
            if m.contains(KeyModifiers::CONTROL) || m.contains(KeyModifiers::SUPER) =>
        {
            if let Some(ds) = &mut app.detail_state
                && let Some(text) = delete_multiline_selection(ds)
            {
                clipboard_set(&text);
            }
            snapshot_multiline(app);
        }
        // Paste (Ctrl+V or Super+V)
        (m, KeyCode::Char('v'))
            if m.contains(KeyModifiers::CONTROL) || m.contains(KeyModifiers::SUPER) =>
        {
            if let Some(paste_text) = clipboard_get() {
                // Capture current cursor before mutating (may be stale from arrow keys)
                snapshot_multiline(app);
                if let Some(ds) = &mut app.detail_state {
                    delete_multiline_selection(ds);
                    let offset = multiline_pos_to_offset(
                        &ds.edit_buffer,
                        ds.edit_cursor_line,
                        ds.edit_cursor_col,
                    );
                    ds.edit_buffer.insert_str(offset, &paste_text);
                    let new_offset = offset + paste_text.len();
                    let (new_line, new_col) = offset_to_multiline_pos(&ds.edit_buffer, new_offset);
                    ds.edit_cursor_line = new_line;
                    ds.edit_cursor_col = new_col;
                }
                snapshot_multiline(app);
            }
        }
        // Inline undo (Ctrl+Z or Super+Z)
        (m, KeyCode::Char('z'))
            if m.contains(KeyModifiers::CONTROL) || m.contains(KeyModifiers::SUPER) =>
        {
            if let Some(ds) = &mut app.detail_state {
                ds.multiline_selection_anchor = None;
            }
            if let Some(eh) = &mut app.edit_history
                && let Some((buf, col, line)) = eh.undo()
            {
                let buf = buf.to_string();
                if let Some(ds) = &mut app.detail_state {
                    ds.edit_buffer = buf;
                    ds.edit_cursor_col = col;
                    ds.edit_cursor_line = line;
                }
            }
        }
        // Inline redo (Ctrl+Y, Ctrl+Shift+Z, or Super+Shift+Z)
        (m, KeyCode::Char('y')) if m.contains(KeyModifiers::CONTROL) => {
            if let Some(ds) = &mut app.detail_state {
                ds.multiline_selection_anchor = None;
            }
            if let Some(eh) = &mut app.edit_history
                && let Some((buf, col, line)) = eh.redo()
            {
                let buf = buf.to_string();
                if let Some(ds) = &mut app.detail_state {
                    ds.edit_buffer = buf;
                    ds.edit_cursor_col = col;
                    ds.edit_cursor_line = line;
                }
            }
        }
        (m, KeyCode::Char('Z'))
            if (m.contains(KeyModifiers::CONTROL) || m.contains(KeyModifiers::SUPER))
                && m.contains(KeyModifiers::SHIFT) =>
        {
            if let Some(ds) = &mut app.detail_state {
                ds.multiline_selection_anchor = None;
            }
            if let Some(eh) = &mut app.edit_history
                && let Some((buf, col, line)) = eh.redo()
            {
                let buf = buf.to_string();
                if let Some(ds) = &mut app.detail_state {
                    ds.edit_buffer = buf;
                    ds.edit_cursor_col = col;
                    ds.edit_cursor_line = line;
                }
            }
        }
        // Enter / Shift+Enter / Ctrl+Enter: newline (delete selection first)
        (KeyModifiers::NONE | KeyModifiers::SHIFT | KeyModifiers::CONTROL, KeyCode::Enter) => {
            if let Some(ds) = &mut app.detail_state {
                delete_multiline_selection(ds);
                let mut edit_lines: Vec<String> =
                    ds.edit_buffer.split('\n').map(String::from).collect();
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
                let mut edit_lines: Vec<String> =
                    ds.edit_buffer.split('\n').map(String::from).collect();
                let line = ds.edit_cursor_line.min(edit_lines.len().saturating_sub(1));
                let col = ds.edit_cursor_col.min(edit_lines[line].len());
                edit_lines[line].insert_str(col, "    ");
                ds.edit_buffer = edit_lines.join("\n");
                ds.edit_cursor_col = col + 4;
            }
            snapshot_multiline(app);
        }
        // Cursor movement: Left (plain or Shift for selection)
        (_, KeyCode::Left)
            if !key.modifiers.contains(KeyModifiers::ALT)
                && !key.modifiers.contains(KeyModifiers::CONTROL)
                && !key.modifiers.contains(KeyModifiers::SUPER) =>
        {
            if let Some(ds) = &mut app.detail_state {
                ds.sticky_col = None;
                let edit_lines: Vec<&str> = ds.edit_buffer.split('\n').collect();
                let cur_line = edit_lines.get(ds.edit_cursor_line).unwrap_or(&"");
                if let Some(prev) = unicode::prev_grapheme_boundary(cur_line, ds.edit_cursor_col) {
                    ds.edit_cursor_col = prev;
                } else if ds.edit_cursor_line > 0 {
                    ds.edit_cursor_line -= 1;
                    ds.edit_cursor_col = edit_lines.get(ds.edit_cursor_line).map_or(0, |l| l.len());
                }
            }
        }
        // Cursor movement: Right (plain or Shift for selection)
        (_, KeyCode::Right)
            if !key.modifiers.contains(KeyModifiers::ALT)
                && !key.modifiers.contains(KeyModifiers::CONTROL)
                && !key.modifiers.contains(KeyModifiers::SUPER) =>
        {
            if let Some(ds) = &mut app.detail_state {
                ds.sticky_col = None;
                let edit_lines: Vec<&str> = ds.edit_buffer.split('\n').collect();
                let cur_line = edit_lines.get(ds.edit_cursor_line).unwrap_or(&"");
                if let Some(next) = unicode::next_grapheme_boundary(cur_line, ds.edit_cursor_col) {
                    ds.edit_cursor_col = next;
                } else if ds.edit_cursor_line + 1 < edit_lines.len() {
                    ds.edit_cursor_line += 1;
                    ds.edit_cursor_col = 0;
                }
            }
        }
        // Cursor movement: Up (plain or Shift for selection)
        (_, KeyCode::Up)
            if !key.modifiers.contains(KeyModifiers::ALT)
                && !key.modifiers.contains(KeyModifiers::CONTROL)
                && !key.modifiers.contains(KeyModifiers::SUPER) =>
        {
            if let Some(ds) = &mut app.detail_state {
                let edit_lines: Vec<&str> = ds.edit_buffer.split('\n').collect();
                if app.note_wrap {
                    let note_width = app.last_edit_available_width as usize;
                    if note_width > 0 {
                        let vls = wrap::wrap_lines_for_edit(
                            &edit_lines,
                            note_width,
                            ds.edit_cursor_line,
                            ds.edit_cursor_col,
                        );
                        let cur_row = wrap::logical_to_visual_row(
                            &vls,
                            ds.edit_cursor_line,
                            ds.edit_cursor_col,
                        );
                        if cur_row > 0 {
                            let vcol = ds.sticky_col.unwrap_or_else(|| {
                                wrap::logical_to_visual_col(
                                    &vls,
                                    ds.edit_cursor_line,
                                    ds.edit_cursor_col,
                                    &edit_lines,
                                )
                            });
                            let (new_line, new_col) =
                                wrap::visual_row_to_logical(&vls, cur_row - 1, vcol, &edit_lines);
                            ds.edit_cursor_line = new_line;
                            ds.edit_cursor_col = new_col;
                            ds.sticky_col = Some(vcol);
                        }
                    }
                } else if ds.edit_cursor_line > 0 {
                    ds.edit_cursor_line -= 1;
                    let line_len = edit_lines.get(ds.edit_cursor_line).map_or(0, |l| l.len());
                    ds.edit_cursor_col = ds.edit_cursor_col.min(line_len);
                }
            }
        }
        // Cursor movement: Down (plain or Shift for selection)
        (_, KeyCode::Down)
            if !key.modifiers.contains(KeyModifiers::ALT)
                && !key.modifiers.contains(KeyModifiers::CONTROL)
                && !key.modifiers.contains(KeyModifiers::SUPER) =>
        {
            if let Some(ds) = &mut app.detail_state {
                let edit_lines: Vec<&str> = ds.edit_buffer.split('\n').collect();
                if app.note_wrap {
                    let note_width = app.last_edit_available_width as usize;
                    if note_width > 0 {
                        let vls = wrap::wrap_lines_for_edit(
                            &edit_lines,
                            note_width,
                            ds.edit_cursor_line,
                            ds.edit_cursor_col,
                        );
                        let cur_row = wrap::logical_to_visual_row(
                            &vls,
                            ds.edit_cursor_line,
                            ds.edit_cursor_col,
                        );
                        if cur_row + 1 < vls.len() {
                            let vcol = ds.sticky_col.unwrap_or_else(|| {
                                wrap::logical_to_visual_col(
                                    &vls,
                                    ds.edit_cursor_line,
                                    ds.edit_cursor_col,
                                    &edit_lines,
                                )
                            });
                            let (new_line, new_col) =
                                wrap::visual_row_to_logical(&vls, cur_row + 1, vcol, &edit_lines);
                            ds.edit_cursor_line = new_line;
                            ds.edit_cursor_col = new_col;
                            ds.sticky_col = Some(vcol);
                        }
                    }
                } else if ds.edit_cursor_line + 1 < edit_lines.len() {
                    ds.edit_cursor_line += 1;
                    let line_len = edit_lines.get(ds.edit_cursor_line).map_or(0, |l| l.len());
                    ds.edit_cursor_col = ds.edit_cursor_col.min(line_len);
                }
            }
        }
        // Paragraph movement: Alt+Up — jump to previous blank line boundary
        (m, KeyCode::Up) if m.contains(KeyModifiers::ALT) => {
            if let Some(ds) = &mut app.detail_state {
                let edit_lines: Vec<&str> = ds.edit_buffer.split('\n').collect();
                let target = prev_paragraph_line(&edit_lines, ds.edit_cursor_line);
                ds.edit_cursor_line = target;
                let line_len = edit_lines.get(target).map_or(0, |l| l.len());
                ds.edit_cursor_col = ds.edit_cursor_col.min(line_len);
            }
        }
        // Paragraph movement: Alt+Down — jump to next blank line boundary
        (m, KeyCode::Down) if m.contains(KeyModifiers::ALT) => {
            if let Some(ds) = &mut app.detail_state {
                let edit_lines: Vec<&str> = ds.edit_buffer.split('\n').collect();
                let target = next_paragraph_line(&edit_lines, ds.edit_cursor_line);
                ds.edit_cursor_line = target;
                let line_len = edit_lines.get(target).map_or(0, |l| l.len());
                ds.edit_cursor_col = ds.edit_cursor_col.min(line_len);
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
        // Ctrl+Left/Right: jump to start/end of line (Ctrl+arrow in terminals)
        (m, KeyCode::Left)
            if m.contains(KeyModifiers::CONTROL) && !m.contains(KeyModifiers::ALT) =>
        {
            if let Some(ds) = &mut app.detail_state {
                ds.multiline_selection_anchor = None;
                ds.edit_cursor_col = 0;
            }
        }
        (m, KeyCode::Right)
            if m.contains(KeyModifiers::CONTROL) && !m.contains(KeyModifiers::ALT) =>
        {
            if let Some(ds) = &mut app.detail_state {
                ds.multiline_selection_anchor = None;
                let edit_lines: Vec<&str> = ds.edit_buffer.split('\n').collect();
                let line_len = edit_lines.get(ds.edit_cursor_line).map_or(0, |l| l.len());
                ds.edit_cursor_col = line_len;
            }
        }
        // Home/End keys: jump to start/end of current visual row
        (_, KeyCode::Home) => {
            if let Some(ds) = &mut app.detail_state {
                ds.multiline_selection_anchor = None;
                ds.sticky_col = None;
                if app.note_wrap {
                    let edit_lines: Vec<&str> = ds.edit_buffer.split('\n').collect();
                    let note_width = app.last_edit_available_width as usize;
                    if note_width > 0 {
                        let vls = wrap::wrap_lines_for_edit(
                            &edit_lines,
                            note_width,
                            ds.edit_cursor_line,
                            ds.edit_cursor_col,
                        );
                        let row = wrap::logical_to_visual_row(
                            &vls,
                            ds.edit_cursor_line,
                            ds.edit_cursor_col,
                        );
                        if let Some(vl) = vls.get(row) {
                            ds.edit_cursor_col = vl.char_start;
                        }
                    }
                } else {
                    ds.edit_cursor_col = 0;
                }
            }
        }
        (_, KeyCode::End) => {
            if let Some(ds) = &mut app.detail_state {
                ds.multiline_selection_anchor = None;
                ds.sticky_col = None;
                let edit_lines: Vec<&str> = ds.edit_buffer.split('\n').collect();
                if app.note_wrap {
                    let note_width = app.last_edit_available_width as usize;
                    if note_width > 0 {
                        let vls = wrap::wrap_lines_for_edit(
                            &edit_lines,
                            note_width,
                            ds.edit_cursor_line,
                            ds.edit_cursor_col,
                        );
                        let row = wrap::logical_to_visual_row(
                            &vls,
                            ds.edit_cursor_line,
                            ds.edit_cursor_col,
                        );
                        if let Some(vl) = vls.get(row) {
                            ds.edit_cursor_col = vl.char_end;
                        }
                    }
                } else {
                    let line_len = edit_lines.get(ds.edit_cursor_line).map_or(0, |l| l.len());
                    ds.edit_cursor_col = line_len;
                }
            }
        }
        // Word movement (Alt+arrow, with or without Shift); crosses line boundaries
        (m, KeyCode::Left) if m.contains(KeyModifiers::ALT) => {
            if let Some(ds) = &mut app.detail_state {
                let edit_lines: Vec<&str> = ds.edit_buffer.split('\n').collect();
                let line_idx = ds.edit_cursor_line.min(edit_lines.len().saturating_sub(1));
                let new_col = word_boundary_left(edit_lines[line_idx], ds.edit_cursor_col);
                if new_col == ds.edit_cursor_col && ds.edit_cursor_col == 0 && line_idx > 0 {
                    // At start of line: jump to end of previous line
                    ds.edit_cursor_line = line_idx - 1;
                    ds.edit_cursor_col = edit_lines[line_idx - 1].len();
                } else {
                    ds.edit_cursor_col = new_col;
                }
            }
        }
        (m, KeyCode::Right) if m.contains(KeyModifiers::ALT) => {
            if let Some(ds) = &mut app.detail_state {
                let edit_lines: Vec<&str> = ds.edit_buffer.split('\n').collect();
                let line_idx = ds.edit_cursor_line.min(edit_lines.len().saturating_sub(1));
                let line_len = edit_lines[line_idx].len();
                let new_col = word_boundary_right(edit_lines[line_idx], ds.edit_cursor_col);
                if new_col == ds.edit_cursor_col
                    && ds.edit_cursor_col == line_len
                    && line_idx + 1 < edit_lines.len()
                {
                    // At end of line: jump to start of next line
                    ds.edit_cursor_line = line_idx + 1;
                    ds.edit_cursor_col = 0;
                } else {
                    ds.edit_cursor_col = new_col;
                }
            }
        }
        // Readline word movement: Alt+B (backward) / Alt+F (forward); crosses line boundaries
        (m, KeyCode::Char('b')) if m.contains(KeyModifiers::ALT) => {
            if let Some(ds) = &mut app.detail_state {
                let edit_lines: Vec<&str> = ds.edit_buffer.split('\n').collect();
                let line_idx = ds.edit_cursor_line.min(edit_lines.len().saturating_sub(1));
                let new_col = word_boundary_left(edit_lines[line_idx], ds.edit_cursor_col);
                if new_col == ds.edit_cursor_col && ds.edit_cursor_col == 0 && line_idx > 0 {
                    ds.edit_cursor_line = line_idx - 1;
                    ds.edit_cursor_col = edit_lines[line_idx - 1].len();
                } else {
                    ds.edit_cursor_col = new_col;
                }
            }
        }
        (m, KeyCode::Char('f')) if m.contains(KeyModifiers::ALT) => {
            if let Some(ds) = &mut app.detail_state {
                let edit_lines: Vec<&str> = ds.edit_buffer.split('\n').collect();
                let line_idx = ds.edit_cursor_line.min(edit_lines.len().saturating_sub(1));
                let line_len = edit_lines[line_idx].len();
                let new_col = word_boundary_right(edit_lines[line_idx], ds.edit_cursor_col);
                if new_col == ds.edit_cursor_col
                    && ds.edit_cursor_col == line_len
                    && line_idx + 1 < edit_lines.len()
                {
                    ds.edit_cursor_line = line_idx + 1;
                    ds.edit_cursor_col = 0;
                } else {
                    ds.edit_cursor_col = new_col;
                }
            }
        }
        // Word backspace (Alt or Ctrl): delete selection or word (joins lines at col 0)
        (m, KeyCode::Backspace)
            if m.contains(KeyModifiers::ALT) || m.contains(KeyModifiers::CONTROL) =>
        {
            if let Some(ds) = &mut app.detail_state
                && delete_multiline_selection(ds).is_none()
            {
                let mut edit_lines: Vec<String> =
                    ds.edit_buffer.split('\n').map(String::from).collect();
                let line_idx = ds.edit_cursor_line.min(edit_lines.len().saturating_sub(1));
                let col = ds.edit_cursor_col.min(edit_lines[line_idx].len());
                if col == 0 && line_idx > 0 {
                    // At start of line: join with previous line
                    let current_line = edit_lines.remove(line_idx);
                    let prev_len = edit_lines[line_idx - 1].len();
                    edit_lines[line_idx - 1].push_str(&current_line);
                    ds.edit_cursor_line = line_idx - 1;
                    ds.edit_cursor_col = prev_len;
                } else {
                    let new_pos = word_boundary_left(&edit_lines[line_idx], col);
                    edit_lines[line_idx].drain(new_pos..col);
                    ds.edit_cursor_col = new_pos;
                }
                ds.edit_buffer = edit_lines.join("\n");
            }
            snapshot_multiline(app);
        }
        // Backspace: delete selection or single char
        (KeyModifiers::NONE, KeyCode::Backspace) => {
            if let Some(ds) = &mut app.detail_state
                && delete_multiline_selection(ds).is_none()
            {
                let mut edit_lines: Vec<String> =
                    ds.edit_buffer.split('\n').map(String::from).collect();
                let line = ds.edit_cursor_line.min(edit_lines.len().saturating_sub(1));
                let col = ds.edit_cursor_col.min(edit_lines[line].len());

                if col > 0 {
                    if let Some(prev) = unicode::prev_grapheme_boundary(&edit_lines[line], col) {
                        edit_lines[line].drain(prev..col);
                        ds.edit_cursor_col = prev;
                    }
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
            snapshot_multiline(app);
        }
        // Toggle note wrap (Alt+w)
        (m, KeyCode::Char('w')) if m.contains(KeyModifiers::ALT) => {
            app.toggle_note_wrap();
            app.status_message = Some(
                if app.note_wrap {
                    "wrap: on"
                } else {
                    "wrap: off"
                }
                .into(),
            );
        }
        // Type character: delete selection first, then insert
        (KeyModifiers::NONE | KeyModifiers::SHIFT, KeyCode::Char(c)) => {
            if let Some(ds) = &mut app.detail_state {
                ds.sticky_col = None;
                delete_multiline_selection(ds);
                let mut edit_lines: Vec<String> =
                    ds.edit_buffer.split('\n').map(String::from).collect();
                let line = ds.edit_cursor_line.min(edit_lines.len().saturating_sub(1));
                let col = ds.edit_cursor_col.min(edit_lines[line].len());
                edit_lines[line].insert(col, c);
                ds.edit_buffer = edit_lines.join("\n");
                ds.edit_cursor_col = col + c.len_utf8();
            }
            snapshot_multiline(app);
        }
        _ => {}
    }
}

/// Snapshot the current multiline edit state for inline undo/redo
pub(super) fn snapshot_multiline(app: &mut App) {
    if let Some(ds) = &app.detail_state
        && let Some(eh) = &mut app.edit_history
    {
        eh.snapshot(&ds.edit_buffer, ds.edit_cursor_col, ds.edit_cursor_line);
    }
}

/// Confirm a detail view or inbox multi-line edit (note)
pub(super) fn confirm_detail_multiline(app: &mut App) {
    // Check if this is an inbox note edit
    if let Some(item_index) = app.inbox_note_index.take() {
        confirm_inbox_note_edit(app, item_index);
        return;
    }

    let (track_id, task_id) = match &app.view {
        View::Detail { track_id, task_id } => (track_id.clone(), task_id.clone()),
        _ => {
            app.mode = Mode::Navigate;
            return;
        }
    };

    let new_value = app
        .detail_state
        .as_ref()
        .map(|ds| ds.edit_buffer.clone())
        .unwrap_or_default();
    let original = app
        .detail_state
        .as_ref()
        .map(|ds| ds.edit_original.clone())
        .unwrap_or_default();

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

        // Record repeatable action
        app.last_action = Some(RepeatableAction::EnterEdit(RepeatEditRegion::Note));
    }

    // Exit edit mode
    app.mode = Mode::Navigate;
    app.autocomplete = None;
    if let Some(ds) = &mut app.detail_state {
        ds.editing = false;
    }
}

/// Confirm an inbox item note edit (called from confirm_detail_multiline)
pub(super) fn confirm_inbox_note_edit(app: &mut App, item_index: usize) {
    let new_body_raw = app
        .detail_state
        .as_ref()
        .map(|ds| ds.edit_buffer.clone())
        .unwrap_or_default();
    let original_raw = app
        .detail_state
        .as_ref()
        .map(|ds| ds.edit_original.clone())
        .unwrap_or_default();

    // Normalize: all-whitespace becomes None
    let new_body = if new_body_raw.trim().is_empty() {
        None
    } else {
        Some(new_body_raw.clone())
    };
    let old_body = if original_raw.trim().is_empty() {
        None
    } else {
        Some(original_raw)
    };

    // Apply the body change
    if let Some(inbox) = &mut app.project.inbox
        && let Some(item) = inbox.items.get_mut(item_index)
    {
        item.body = new_body.clone();
        item.dirty = true;
    }
    let _ = app.save_inbox();

    if new_body != old_body {
        app.undo_stack.push(Operation::InboxNoteEdit {
            index: item_index,
            old_body,
            new_body,
        });
    }

    // Exit edit mode
    app.mode = Mode::Navigate;
    app.detail_state = None;
    app.inbox_note_editor_scroll = 0;
}

/// Confirm a detail view single-line edit (title, tags, deps, spec, refs)
pub(super) fn confirm_detail_edit(app: &mut App) {
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
    let original = app
        .detail_state
        .as_ref()
        .map(|ds| ds.edit_original.clone())
        .unwrap_or_default();

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

    // Record repeatable action for detail view edits
    let repeat_region = match region {
        DetailRegion::Title => Some(RepeatEditRegion::Title),
        DetailRegion::Tags => Some(RepeatEditRegion::Tags),
        DetailRegion::Deps => Some(RepeatEditRegion::Deps),
        DetailRegion::Refs => Some(RepeatEditRegion::Refs),
        DetailRegion::Note => Some(RepeatEditRegion::Note),
        _ => None,
    };
    if let Some(r) = repeat_region {
        app.last_action = Some(RepeatableAction::EnterEdit(r));
    }

    // Exit edit mode
    app.mode = Mode::Navigate;
    app.edit_target = None;
    app.edit_buffer.clear();
    app.edit_cursor = 0;
    app.edit_h_scroll = 0;
    app.autocomplete = None;
    if let Some(ds) = &mut app.detail_state {
        ds.editing = false;
    }
}

/// Cancel a detail view edit
pub(super) fn cancel_detail_edit(app: &mut App) {
    app.mode = Mode::Navigate;
    app.edit_target = None;
    app.edit_buffer.clear();
    app.edit_cursor = 0;
    app.edit_h_scroll = 0;
    app.autocomplete = None;
    if let Some(ds) = &mut app.detail_state {
        ds.editing = false;
        ds.edit_buffer.clear();
    }
}

/// Switch to the next/prev tab. Direction: 1 = forward, -1 = backward.
pub(super) fn switch_tab(app: &mut App, direction: i32) {
    let total_tracks = app.active_track_ids.len();
    // All views in order: Track(0)..Track(N-1), Tracks, Inbox, Recent
    let total_views = total_tracks + 3;

    let current_idx = match &app.view {
        View::Track(i) => *i,
        View::Detail { track_id, .. } => {
            // When in detail view, tab switching goes back to track view
            app.active_track_ids
                .iter()
                .position(|id| id == track_id)
                .unwrap_or(0)
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

    // Refresh match count for the new view
    update_match_count(app);
}

/// Activate autocomplete for the given detail region
pub(super) fn activate_autocomplete_for_region(app: &mut App, region: DetailRegion) {
    let (kind, candidates) = match region {
        DetailRegion::Tags => (AutocompleteKind::Tag, app.collect_all_tags()),
        DetailRegion::Deps => (AutocompleteKind::TaskId, app.collect_all_task_ids()),
        DetailRegion::Spec | DetailRegion::Refs => {
            (AutocompleteKind::FilePath, app.collect_file_paths())
        }
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
pub(super) fn autocomplete_filter_text(buffer: &str, kind: AutocompleteKind) -> String {
    match kind {
        AutocompleteKind::Tag => {
            // Get last word (which may start with #, +, or -)
            let word = buffer.rsplit_once(' ').map(|(_, w)| w).unwrap_or(buffer);
            let word = word
                .strip_prefix('+')
                .or_else(|| word.strip_prefix('-'))
                .unwrap_or(word);
            word.strip_prefix('#').unwrap_or(word).to_string()
        }
        AutocompleteKind::TaskId => {
            // Get last entry (after comma or space), strip +/- prefix for bulk edit
            let word = buffer
                .rsplit(|c: char| c == ',' || c.is_whitespace())
                .next()
                .unwrap_or(buffer)
                .trim();
            let word = word
                .strip_prefix('+')
                .or_else(|| word.strip_prefix('-'))
                .unwrap_or(word);
            word.to_string()
        }
        AutocompleteKind::FilePath => {
            // Get current entry (after last space for multi-value refs)
            let word = buffer.rsplit(' ').next().unwrap_or(buffer).trim();
            word.to_string()
        }
        AutocompleteKind::JumpTaskId => {
            // Whole buffer is the filter query
            buffer.trim().to_string()
        }
    }
}

/// Update autocomplete filter when text changes
pub(super) fn update_autocomplete_filter(app: &mut App) {
    if let Some(ac) = &mut app.autocomplete {
        let kind = ac.kind;
        let filter_text = autocomplete_filter_text(&app.edit_buffer, kind);
        ac.filter(&filter_text);
        // Hide if no matches
        ac.visible = !ac.filtered.is_empty();
    }
}

/// Accept the currently selected autocomplete entry
pub(super) fn autocomplete_accept(app: &mut App) {
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
            let existing: Vec<String> = app
                .edit_buffer
                .split_whitespace()
                .map(|s| s.strip_prefix('#').unwrap_or(s).to_string())
                .collect();
            let buf = &app.edit_buffer;
            let last_space = buf.rfind(' ');
            if existing.contains(&selected) {
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
            let existing: Vec<&str> = app
                .edit_buffer
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
            } else if let Some(pos) = last_sep {
                app.edit_buffer.truncate(pos + 1);
                if !app.edit_buffer.ends_with(' ') {
                    app.edit_buffer.push(' ');
                }
                app.edit_buffer.push_str(&selected);
            } else {
                app.edit_buffer = selected;
            }
            app.edit_cursor = app.edit_buffer.len();
        }
        AutocompleteKind::JumpTaskId => {
            // Extract just the task ID from "ID  title" format
            let id = selected
                .split_whitespace()
                .next()
                .unwrap_or(&selected)
                .to_string();
            app.edit_buffer = id;
            app.edit_cursor = app.edit_buffer.len();
        }
        AutocompleteKind::FilePath => {
            // Support space-separated entries (for refs); normalized to commas on confirm
            // Check for duplicate: skip if this path is already in the buffer
            let existing: Vec<&str> = app.edit_buffer.split_whitespace().collect();
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
