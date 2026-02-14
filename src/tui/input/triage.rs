use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::model::SectionKind;
use crate::ops::task_ops::{self, InsertPosition};
use crate::util::unicode;

use crate::tui::app::{
    App, AutocompleteKind, AutocompleteState, DetailRegion, DetailState, EditHistory, EditTarget,
    Mode, MoveState, TriageSource, View,
};
use crate::tui::undo::Operation;

use super::*;

/// Add a new inbox item at the bottom and enter EDIT mode for its title.
pub(super) fn inbox_add_item(app: &mut App) {
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
pub(super) fn inbox_insert_after(app: &mut App) {
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

/// Insert a new inbox item at the top and enter EDIT mode.
pub(super) fn inbox_prepend_item(app: &mut App) {
    let inbox = match &mut app.project.inbox {
        Some(inbox) => inbox,
        None => return,
    };

    let item = crate::model::inbox::InboxItem::new(String::new());
    inbox.items.insert(0, item);

    // Move cursor to new item at top
    app.inbox_cursor = 0;

    // Enter EDIT mode for the title
    app.edit_buffer.clear();
    app.edit_cursor = 0;
    app.edit_target = Some(EditTarget::NewInboxItem { index: 0 });
    app.edit_history = Some(EditHistory::new("", 0, 0));
    app.mode = Mode::Edit;
}

/// Edit the title of the selected inbox item.
pub(super) fn inbox_edit_title(app: &mut App) {
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
pub(super) fn inbox_edit_tags(app: &mut App) {
    let inbox = match &app.project.inbox {
        Some(inbox) => inbox,
        None => return,
    };
    let item = match inbox.items.get(app.inbox_cursor) {
        Some(item) => item,
        None => return,
    };

    let original_tags: String = item
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

/// Edit the note/body of the selected inbox item (multi-line inline editor).
/// When `cursor_at_end` is true, the cursor starts at the end of the note.
pub(super) fn inbox_edit_note(app: &mut App, cursor_at_end: bool) {
    let inbox = match &app.project.inbox {
        Some(inbox) => inbox,
        None => return,
    };
    let item = match inbox.items.get(app.inbox_cursor) {
        Some(item) => item,
        None => return,
    };

    let body_text = item.body.as_deref().unwrap_or("").to_string();
    let (cursor_line, cursor_col) = if cursor_at_end {
        let line_count = body_text.split('\n').count();
        let last_line_len = body_text.split('\n').next_back().map_or(0, |l| l.len());
        (line_count.saturating_sub(1), last_line_len)
    } else {
        (0, 0)
    };

    // Create a DetailState to reuse the multiline edit infrastructure
    let ds = DetailState {
        region: DetailRegion::Note,
        scroll_offset: 0,
        regions: vec![DetailRegion::Note],
        return_view: crate::tui::app::ReturnView::Track(0),
        editing: true,
        edit_buffer: body_text.clone(),
        edit_cursor_line: cursor_line,
        edit_cursor_col: cursor_col,
        edit_original: body_text.clone(),
        subtask_cursor: 0,
        flat_subtask_ids: Vec::new(),
        multiline_selection_anchor: None,
        note_h_scroll: 0,
        sticky_col: None,
        total_lines: 0,
        note_view_line: None,
        note_header_line: None,
        note_content_end: 0,
        regions_populated: vec![true],
    };

    app.detail_state = Some(ds);
    app.inbox_note_index = Some(app.inbox_cursor);
    app.inbox_note_editor_scroll = 0;
    app.edit_target = None; // multiline pattern: edit_target is None
    app.edit_history = Some(EditHistory::new(&body_text, cursor_col, cursor_line));
    app.mode = Mode::Edit;
}

/// Delete the selected inbox item (with confirmation).
pub(super) fn inbox_delete_item(app: &mut App) {
    let inbox = match &app.project.inbox {
        Some(inbox) => inbox,
        None => return,
    };
    if inbox.items.is_empty() || app.inbox_cursor >= inbox.items.len() {
        return;
    }

    let title = &inbox.items[app.inbox_cursor].title;
    let short_title = if unicode::display_width(title) > 30 {
        unicode::truncate_to_width(title, 30)
    } else {
        title.clone()
    };

    app.confirm_state = Some(crate::tui::app::ConfirmState {
        message: format!("Delete \"{}\"? (y/n)", short_title),
        action: crate::tui::app::ConfirmAction::DeleteInboxItem {
            index: app.inbox_cursor,
        },
    });
    app.mode = Mode::Confirm;
}

/// Enter MOVE mode for inbox items.
pub(super) fn inbox_enter_move_mode(app: &mut App) {
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
pub(super) fn inbox_begin_triage(app: &mut App) {
    let count = app.inbox_count();
    if count == 0 || app.inbox_cursor >= count {
        return;
    }

    // Activate track selection autocomplete (show prefix from config)
    let active_tracks: Vec<String> = app
        .project
        .config
        .tracks
        .iter()
        .filter(|t| t.state == "active")
        .map(|t| {
            let prefix = app
                .project
                .config
                .ids
                .prefixes
                .get(&t.id)
                .map(|p| p.to_uppercase())
                .unwrap_or_else(|| t.id.to_uppercase());
            format!("{} ({})", t.name, prefix)
        })
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

    app.triage_state = Some(crate::tui::app::TriageState {
        source: TriageSource::Inbox {
            index: app.inbox_cursor,
        },
        step: crate::tui::app::TriageStep::SelectTrack,
        popup_anchor: None,
        position_cursor: 1, // default to Bottom
    });
    app.mode = Mode::Triage;
}

// ---------------------------------------------------------------------------
// Triage mode handler (Phase 7.3)

pub(super) fn handle_triage(app: &mut App, key: KeyEvent) {
    let step = match &app.triage_state {
        Some(ts) => ts.step.clone(),
        None => {
            app.mode = Mode::Navigate;
            return;
        }
    };

    match step {
        crate::tui::app::TriageStep::SelectTrack => handle_triage_select_track(app, key),
        crate::tui::app::TriageStep::SelectPosition { track_id } => {
            handle_triage_select_position(app, key, &track_id.clone())
        }
    }
}

pub(super) fn handle_triage_select_track(app: &mut App, key: KeyEvent) {
    match (key.modifiers, key.code) {
        // Cancel
        (_, KeyCode::Esc) => {
            app.mode = if app.selection.is_empty() {
                Mode::Navigate
            } else {
                Mode::Select
            };
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
            let selected = app
                .autocomplete
                .as_ref()
                .and_then(|ac| ac.selected_entry().map(|s| s.to_string()));
            if let Some(entry) = selected {
                // Extract prefix from "Track Name (PREFIX)" and find the matching track
                let prefix_str = entry
                    .rsplit('(')
                    .next()
                    .and_then(|s| s.strip_suffix(')'))
                    .unwrap_or(&entry);

                // Find track by prefix match (or fall back to treating it as a track ID)
                let track_id = app
                    .project
                    .config
                    .ids
                    .prefixes
                    .iter()
                    .find(|(_, p)| p.eq_ignore_ascii_case(prefix_str))
                    .map(|(tid, _)| tid.clone())
                    .unwrap_or_else(|| prefix_str.to_lowercase());

                // Verify track exists
                let valid = app.project.config.tracks.iter().any(|t| t.id == track_id);
                if valid {
                    // Capture anchor from autocomplete before clearing it
                    let anchor = app.autocomplete_anchor;
                    app.autocomplete = None;
                    app.edit_buffer.clear();
                    if let Some(ts) = &mut app.triage_state {
                        ts.popup_anchor = anchor;
                        ts.step = crate::tui::app::TriageStep::SelectPosition { track_id };
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

pub(super) fn handle_triage_select_position(app: &mut App, key: KeyEvent, track_id: &str) {
    match (key.modifiers, key.code) {
        // Cancel
        (_, KeyCode::Esc) => {
            app.mode = if app.selection.is_empty() {
                Mode::Navigate
            } else {
                Mode::Select
            };
            app.triage_state = None;
            app.autocomplete = None;
            app.edit_buffer.clear();
        }

        // Navigate between options: 0=Top, 1=Bottom, 2=Cancel
        (KeyModifiers::NONE, KeyCode::Up | KeyCode::Char('k')) => {
            if let Some(ts) = &mut app.triage_state {
                ts.position_cursor = ts.position_cursor.saturating_sub(1);
            }
        }
        (KeyModifiers::NONE, KeyCode::Down | KeyCode::Char('j')) => {
            if let Some(ts) = &mut app.triage_state {
                ts.position_cursor = (ts.position_cursor + 1).min(2);
            }
        }

        // Confirm selection
        (_, KeyCode::Enter) => {
            let cursor = app
                .triage_state
                .as_ref()
                .map(|ts| ts.position_cursor)
                .unwrap_or(1);
            match cursor {
                0 => dispatch_triage_or_move(app, track_id, InsertPosition::Top),
                1 => dispatch_triage_or_move(app, track_id, InsertPosition::Bottom),
                _ => {
                    // Cancel
                    app.mode = Mode::Navigate;
                    app.triage_state = None;
                    app.autocomplete = None;
                    app.edit_buffer.clear();
                }
            }
        }

        // Direct shortcuts still work
        (KeyModifiers::NONE, KeyCode::Char('t')) => {
            dispatch_triage_or_move(app, track_id, InsertPosition::Top);
        }
        (KeyModifiers::NONE, KeyCode::Char('b')) => {
            dispatch_triage_or_move(app, track_id, InsertPosition::Bottom);
        }

        _ => {}
    }
}

/// Dispatch to execute_triage or execute_cross_track_move based on the triage source
pub(super) fn dispatch_triage_or_move(app: &mut App, track_id: &str, position: InsertPosition) {
    let source = match &app.triage_state {
        Some(ts) => ts.source.clone(),
        None => return,
    };
    match source {
        TriageSource::Inbox { .. } => execute_triage(app, track_id, position),
        TriageSource::CrossTrackMove { .. } => execute_cross_track_move(app, track_id, position),
        TriageSource::BulkCrossTrackMove { .. } => {
            execute_bulk_cross_track_move(app, track_id, position)
        }
    }
}

pub(super) fn execute_triage(app: &mut App, track_id: &str, position: InsertPosition) {
    let inbox_index = match &app.triage_state {
        Some(ts) => match &ts.source {
            TriageSource::Inbox { index } => *index,
            _ => return,
        },
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

    let task_id = match crate::ops::inbox_ops::triage(inbox, inbox_index, track, position, &prefix)
    {
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

    // Save track first (new data), then inbox (deletion)
    app.save_track_logged(track_id);
    app.save_inbox_logged();

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
// Cross-track move (M key)

/// Begin cross-track move: enter triage-style track selection for moving a task
pub(super) fn begin_cross_track_move(app: &mut App) {
    // Determine source task
    let (source_track_id, task_id, section) = match &app.view {
        View::Track(_) => match app.cursor_task_id() {
            Some(info) => info,
            None => return,
        },
        View::Detail { track_id, task_id } => {
            // Find task to determine its section
            let section = if let Some(track) = App::find_track_in_project(&app.project, track_id) {
                if task_ops::is_top_level_in_section(track, task_id, SectionKind::Backlog) {
                    SectionKind::Backlog
                } else if task_ops::find_task_in_track(track, task_id).is_some() {
                    // Subtask in backlog — will be promoted
                    SectionKind::Backlog
                } else {
                    return;
                }
            } else {
                return;
            };
            (track_id.clone(), task_id.clone(), section)
        }
        _ => return,
    };

    // Only allow moving tasks from Backlog
    if section != SectionKind::Backlog {
        app.status_message = Some("Can only move backlog tasks".to_string());
        return;
    }

    // Build candidate tracks: all non-archived tracks except current (show prefix)
    let candidates: Vec<String> = app
        .project
        .config
        .tracks
        .iter()
        .filter(|t| t.state != "archived" && t.id != source_track_id)
        .map(|t| {
            let prefix = app
                .project
                .config
                .ids
                .prefixes
                .get(&t.id)
                .map(|p| p.to_uppercase())
                .unwrap_or_else(|| t.id.to_uppercase());
            format!("{} ({})", t.name, prefix)
        })
        .collect();

    if candidates.is_empty() {
        app.status_message = Some("No other tracks to move to".to_string());
        return;
    }

    app.edit_buffer.clear();
    app.edit_cursor = 0;
    app.autocomplete = Some(AutocompleteState::new(AutocompleteKind::Tag, candidates));
    if let Some(ac) = &mut app.autocomplete {
        ac.filter(""); // Show all
    }

    app.triage_state = Some(crate::tui::app::TriageState {
        source: TriageSource::CrossTrackMove {
            source_track_id,
            task_id,
        },
        step: crate::tui::app::TriageStep::SelectTrack,
        popup_anchor: None,
        position_cursor: 1, // default to Bottom
    });
    app.mode = Mode::Triage;
}

/// Execute the cross-track move after track and position are selected
pub(super) fn execute_cross_track_move(
    app: &mut App,
    target_track_id: &str,
    position: InsertPosition,
) {
    let (source_track_id, task_id) = match &app.triage_state {
        Some(ts) => match &ts.source {
            TriageSource::CrossTrackMove {
                source_track_id,
                task_id,
            } => (source_track_id.clone(), task_id.clone()),
            _ => return,
        },
        None => return,
    };

    let target_prefix = app.track_prefix(target_track_id).unwrap_or("").to_string();

    // Determine if task is a subtask (has a parent)
    let is_subtask = task_id.contains('.');
    let source_parent_id = if is_subtask {
        // Extract parent ID: everything before the last dot
        task_id
            .rsplit_once('.')
            .map(|(parent, _)| parent.to_string())
    } else {
        None
    };

    // Find old depth
    let old_depth = {
        let track = match App::find_track_in_project(&app.project, &source_track_id) {
            Some(t) => t,
            None => return,
        };
        task_ops::find_task_in_track(track, &task_id)
            .map(|t| t.depth)
            .unwrap_or(0)
    };

    // Remove task from source
    let (mut task, source_index) = if let Some(ref parent_id) = source_parent_id {
        // Subtask: remove from parent's subtask list
        let source_track = match app.find_track_mut(&source_track_id) {
            Some(t) => t,
            None => return,
        };
        let parent = match task_ops::find_task_mut_in_track(source_track, parent_id) {
            Some(p) => p,
            None => return,
        };
        let idx = match parent
            .subtasks
            .iter()
            .position(|t| t.id.as_deref() == Some(&task_id))
        {
            Some(i) => i,
            None => return,
        };
        let task = parent.subtasks.remove(idx);
        parent.mark_dirty();
        (task, idx)
    } else {
        // Top-level: remove from source backlog
        let source_track = match app.find_track_mut(&source_track_id) {
            Some(t) => t,
            None => return,
        };
        let source_tasks = match source_track.section_tasks_mut(SectionKind::Backlog) {
            Some(t) => t,
            None => return,
        };
        let idx = match source_tasks
            .iter()
            .position(|t| t.id.as_deref() == Some(&task_id))
        {
            Some(i) => i,
            None => return,
        };
        let task = source_tasks.remove(idx);
        (task, idx)
    };

    // Compute new ID
    let target_track = match App::find_track_in_project(&app.project, target_track_id) {
        Some(t) => t,
        None => return,
    };
    let new_num = task_ops::next_id_number(target_track, &target_prefix);
    let new_id = format!("{}-{:03}", target_prefix, new_num);
    let old_id = task_id.clone();

    // Set new ID and depth
    task.id = Some(new_id.clone());
    task.depth = 0;
    task.mark_dirty();
    task_ops::renumber_subtasks(&mut task, &new_id);

    // Insert into target backlog
    let target_track = match app.find_track_mut(target_track_id) {
        Some(t) => t,
        None => return,
    };
    let target_tasks = match target_track.section_tasks_mut(SectionKind::Backlog) {
        Some(t) => t,
        None => return,
    };
    let target_index = match &position {
        InsertPosition::Top => {
            target_tasks.insert(0, task);
            0
        }
        InsertPosition::Bottom => {
            let idx = target_tasks.len();
            target_tasks.push(task);
            idx
        }
        InsertPosition::After(after_id) => {
            let after_idx = target_tasks
                .iter()
                .position(|t| t.id.as_deref() == Some(after_id.as_str()))
                .unwrap_or(target_tasks.len().saturating_sub(1));
            target_tasks.insert(after_idx + 1, task);
            after_idx + 1
        }
    };

    // Update dep references across all tracks
    task_ops::update_dep_references(&mut app.project.tracks, &old_id, &new_id);

    // Push undo operation
    app.undo_stack.push(Operation::CrossTrackMove {
        source_track_id: source_track_id.clone(),
        target_track_id: target_track_id.to_string(),
        task_id_old: old_id.clone(),
        task_id_new: new_id.clone(),
        source_index,
        target_index,
        source_parent_id,
        old_depth,
    });

    // Save target first (new data), then source (deletion)
    app.save_track_logged(target_track_id);
    app.save_track_logged(&source_track_id);

    // Cursor management
    let was_detail = matches!(app.view, View::Detail { .. });
    if was_detail {
        // Close detail view, return to track view
        app.close_detail_fully();
        if let Some(idx) = app
            .active_track_ids
            .iter()
            .position(|id| id == &source_track_id)
        {
            app.view = View::Track(idx);
        }
    } else {
        // Advance cursor in track view (or clamp to last)
        if let Some(track_id) = app.current_track_id().map(|s| s.to_string()) {
            let flat_items = app.build_flat_items(&track_id);
            let state = app.get_track_state(&track_id);
            if state.cursor >= flat_items.len() && !flat_items.is_empty() {
                state.cursor = flat_items.len() - 1;
            }
        }
    }

    // Status message
    let target_name = app.track_name(target_track_id).to_string();
    app.status_message = Some(format!(
        "Moved to {} ({} → {})",
        target_name, old_id, new_id
    ));

    // Clean up triage state
    app.mode = Mode::Navigate;
    app.triage_state = None;
    app.autocomplete = None;
    app.edit_buffer.clear();
}

/// Execute bulk cross-track move: move all selected tasks to the target track
pub(super) fn execute_bulk_cross_track_move(
    app: &mut App,
    target_track_id: &str,
    position: InsertPosition,
) {
    let source_track_id = match &app.triage_state {
        Some(ts) => match &ts.source {
            TriageSource::BulkCrossTrackMove { source_track_id } => source_track_id.clone(),
            _ => return,
        },
        None => return,
    };

    let target_prefix = app.track_prefix(target_track_id).unwrap_or("").to_string();

    // Collect selected task IDs in backlog order
    let selected_ids: Vec<String> = {
        let track = match App::find_track_in_project(&app.project, &source_track_id) {
            Some(t) => t,
            None => return,
        };
        let backlog = track.backlog();
        backlog
            .iter()
            .filter_map(|t| {
                t.id.as_ref()
                    .filter(|id| app.selection.contains(*id))
                    .cloned()
            })
            .collect()
    };

    if selected_ids.is_empty() {
        app.triage_state = None;
        app.mode = if app.selection.is_empty() {
            Mode::Navigate
        } else {
            Mode::Select
        };
        return;
    }

    let mut ops: Vec<Operation> = Vec::new();
    let mut new_ids: Vec<String> = Vec::new();

    for task_id in &selected_ids {
        // Get next ID number (must re-query each time since we're inserting)
        let target_track = match App::find_track_in_project(&app.project, target_track_id) {
            Some(t) => t,
            None => continue,
        };
        let new_num = task_ops::next_id_number(target_track, &target_prefix);
        let new_id = format!("{}-{:03}", target_prefix, new_num);

        // Remove from source
        let source_track = match app.find_track_mut(&source_track_id) {
            Some(t) => t,
            None => continue,
        };
        let source_tasks = match source_track.section_tasks_mut(SectionKind::Backlog) {
            Some(t) => t,
            None => continue,
        };
        let idx = match source_tasks
            .iter()
            .position(|t| t.id.as_deref() == Some(task_id))
        {
            Some(i) => i,
            None => continue,
        };
        let mut task = source_tasks.remove(idx);
        let source_index = idx;

        // Set new ID and depth
        let old_id = task_id.clone();
        task.id = Some(new_id.clone());
        task.depth = 0;
        task.mark_dirty();
        task_ops::renumber_subtasks(&mut task, &new_id);

        // Insert into target backlog
        let target_track = match app.find_track_mut(target_track_id) {
            Some(t) => t,
            None => continue,
        };
        let target_tasks = match target_track.section_tasks_mut(SectionKind::Backlog) {
            Some(t) => t,
            None => continue,
        };
        let target_index = match &position {
            InsertPosition::Top => {
                // Insert at the front, but after previously inserted tasks
                let insert_at = ops.len().min(target_tasks.len());
                target_tasks.insert(insert_at, task);
                insert_at
            }
            InsertPosition::Bottom => {
                let idx = target_tasks.len();
                target_tasks.push(task);
                idx
            }
            InsertPosition::After(after_id) => {
                let after_idx = target_tasks
                    .iter()
                    .position(|t| t.id.as_deref() == Some(after_id.as_str()))
                    .unwrap_or(target_tasks.len().saturating_sub(1));
                target_tasks.insert(after_idx + 1, task);
                after_idx + 1
            }
        };

        // Update dep references across all tracks
        task_ops::update_dep_references(&mut app.project.tracks, &old_id, &new_id);

        ops.push(Operation::CrossTrackMove {
            source_track_id: source_track_id.clone(),
            target_track_id: target_track_id.to_string(),
            task_id_old: old_id,
            task_id_new: new_id.clone(),
            source_index,
            target_index,
            source_parent_id: None,
            old_depth: 0,
        });

        new_ids.push(new_id);
    }

    if !ops.is_empty() {
        // Save target first (new data), then source (deletion)
        app.save_track_logged(target_track_id);
        app.save_track_logged(&source_track_id);

        let count = ops.len();
        app.undo_stack.push(Operation::Bulk(ops));

        // Update selection to use new IDs
        for old_id in &selected_ids {
            app.selection.remove(old_id);
        }
        for new_id in &new_ids {
            app.selection.insert(new_id.clone());
        }

        // Adjust cursor
        if let Some(track_id) = app.current_track_id().map(|s| s.to_string()) {
            let flat_items = app.build_flat_items(&track_id);
            let state = app.get_track_state(&track_id);
            if state.cursor >= flat_items.len() && !flat_items.is_empty() {
                state.cursor = flat_items.len() - 1;
            }
        }

        let target_name = app.track_name(target_track_id).to_string();
        app.status_message = Some(format!("{} tasks moved to {}", count, target_name));
    }

    // Clean up triage state
    app.mode = if app.selection.is_empty() {
        Mode::Navigate
    } else {
        Mode::Select
    };
    app.triage_state = None;
    app.autocomplete = None;
    app.edit_buffer.clear();
}

// ---------------------------------------------------------------------------
// Confirm mode handler
