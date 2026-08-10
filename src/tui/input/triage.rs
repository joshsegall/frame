use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::model::SectionKind;
use crate::model::task_id::TaskId;
use crate::ops::ids::Mint;
use crate::ops::task_ops::{self, InsertPosition};
use crate::util::unicode;

use crate::tui::app::{
    App, AutocompleteKind, AutocompleteState, DetailRegion, DetailState, EditHistory, EditTarget,
    Mode, MoveState, TriageSource, View,
};
use crate::tui::undo::Operation;

use super::*;

/// Write out every track a cross-track move's dep rewrite touched, beyond the
/// two the move itself wrote.
///
/// A move renames a task, so any track holding a `dep:` on it is edited too —
/// and the TUI wrote only the source and the target. The rewrite lived in
/// memory and reached disk only if something else happened to save that track
/// later; on the next reload the dep pointed at the retired id and `fr check`
/// called it dangling. The CLI has always done this (`cmd_mv`), and
/// `task_ops::track_has_dirty_task` was written for it — its doc comment names
/// this exact case. Only the TUI never called it.
///
/// Runs **after** the in-flight marker commits, for the CLI's stated reason:
/// the move itself is already durable once source and target are written, so a
/// failure here leaves a recoverable dangling dep rather than a half-moved
/// task, and it has no business inside the two-file window.
fn save_tracks_with_rewritten_deps(app: &mut App, already_saved: &[&str]) {
    let touched: Vec<String> = app
        .project
        .tracks
        .iter()
        .filter(|(id, track)| {
            !already_saved.contains(&id.as_str()) && task_ops::track_has_dirty_task(track)
        })
        .map(|(id, _)| id.clone())
        .collect();
    for id in &touched {
        app.save_track_logged(id);
    }
}

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

    // Captured before `inbox_item` is moved into the undo entry.
    let triaged_title = inbox_item.title.clone();
    let prefix = app.track_prefix(track_id).unwrap_or("").to_string();
    let token = match app.resolve_mint_namespace() {
        Ok(t) => t,
        Err(()) => return,
    };

    let frame_dir = app.project.frame_dir.clone();
    let mint = Mint::new(&frame_dir, track_id, &prefix, token.as_ref());
    let inbox = match &mut app.project.inbox {
        Some(inbox) => inbox,
        None => return,
    };
    let track = match app.project.tracks.iter_mut().find(|(id, _)| id == track_id) {
        Some((_, track)) => track,
        None => return,
    };

    let task_id = match crate::ops::inbox_ops::triage(inbox, inbox_index, track, position, mint) {
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

    // Track first (new data), then inbox (deletion), under one lock: an
    // interruption must leave the item duplicated rather than lost, and a
    // writer slipping between the two would defeat that. The marker lets the
    // next command finish the deletion.
    let marker = crate::io::inflight::InFlight::begin(
        &app.project.frame_dir,
        crate::io::inflight::Operation::Triage {
            index: inbox_index + 1,
            title: triaged_title,
            track_id: track_id.to_string(),
        },
        "triage (TUI)",
    )
    .ok();
    app.save_batch_logged(&[track_id], true);
    if let Some(marker) = marker {
        marker.commit();
    }

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
            // Find the section holding the task (or its top-level ancestor).
            let section = match App::find_track_in_project(&app.project, track_id)
                .and_then(|track| task_ops::find_task_location_any_section(track, task_id))
            {
                Some(loc) => loc.section,
                None => return,
            };
            (track_id.clone(), task_id.clone(), section)
        }
        _ => return,
    };

    // Build candidate tracks: every track that accepts new tasks except the
    // current one (show prefix). Shelved tracks are excluded for the same reason
    // `fr mv --track` refuses them.
    let candidates: Vec<String> = app
        .project
        .config
        .tracks
        .iter()
        .filter(|t| crate::ops::track_ops::accepts_new_tasks(&t.state) && t.id != source_track_id)
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
            section,
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
    let (source_track_id, task_id, section) = match &app.triage_state {
        Some(ts) => match &ts.source {
            TriageSource::CrossTrackMove {
                source_track_id,
                task_id,
                section,
            } => (source_track_id.clone(), task_id.clone(), *section),
            _ => return,
        },
        None => return,
    };

    let target_prefix = app.track_prefix(target_track_id).unwrap_or("").to_string();
    // Resolve the mover's namespace before any mutation so a frontier-empty
    // abort leaves both source and target untouched.
    let token = match app.resolve_mint_namespace() {
        Ok(t) => t,
        Err(()) => return,
    };

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
        // Top-level: remove from the section it lives in (Backlog, Parked, or
        // Done) so a completed task can be relocated without reopening it.
        let source_track = match app.find_track_mut(&source_track_id) {
            Some(t) => t,
            None => return,
        };
        let source_tasks = match source_track.section_tasks_mut(section) {
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
    // Re-mint the new id (and subtree) in the mover's namespace.
    let mint = Mint::new(
        &app.project.frame_dir,
        target_track_id,
        &target_prefix,
        token.as_ref(),
    );
    let new_num = mint.next(target_track);
    let new_id = TaskId::with_number(&target_prefix, new_num, token.as_ref());
    let old_id = task_id.clone();

    // Set new ID and depth
    task.id = Some(new_id.clone());
    task.depth = 0;
    task.mark_dirty();
    // Take the descendants' ids either side of the re-key, so undo can put the
    // originals back rather than renumbering by position — which is not the
    // inverse when the subtree was not numbered in order.
    let old_subtree_ids = task_ops::subtree_ids(&task);
    task_ops::renumber_subtasks(&mut task, &new_id, token.as_ref());
    let subtree_ids: Vec<(String, String)> = old_subtree_ids
        .into_iter()
        .zip(task_ops::subtree_ids(&task))
        .collect();

    // Insert into the same section on the target (a subtask promotes into the
    // target Backlog), creating the section if the target lacks it.
    let target_section = if source_parent_id.is_some() {
        SectionKind::Backlog
    } else {
        section
    };
    let target_track = match app.find_track_mut(target_track_id) {
        Some(t) => t,
        None => return,
    };
    target_track.ensure_section(target_section);
    let target_tasks = match target_track.section_tasks_mut(target_section) {
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

    // Update dep references across all tracks — the descendants too, not just
    // the root. A dep on a moved subtask points at an id the re-key retired, and
    // `fr check --fix` deliberately will not repair a dangling dep, so this is
    // the only chance to keep it.
    let id_map = task_ops::cross_track_id_map(&old_id, &new_id.to_string(), &subtree_ids);
    task_ops::apply_id_map_to_deps(&mut app.project.tracks, &id_map);

    // Push undo operation
    app.undo_stack.push(Operation::CrossTrackMove {
        source_track_id: source_track_id.clone(),
        target_track_id: target_track_id.to_string(),
        task_id_old: old_id.clone(),
        task_id_new: new_id.to_string(),
        source_index,
        target_index,
        source_parent_id,
        old_depth,
        section,
        subtree_ids,
    });

    // Target before source, under one lock — see doc/architecture.md,
    // "Multi-file writes": whichever write creates must run before the one that
    // destroys, and dropping the lock between them makes the ordering correct
    // but not atomic.
    //
    // The marker records the intent first, so a crash between the two writes is
    // completed by the next command rather than leaving the task in both tracks
    // under different ids — a state nothing can detect. Failing to write it is
    // not worth aborting the move over: degrade to no marker.
    let marker = crate::io::inflight::InFlight::begin(
        &app.project.frame_dir,
        crate::io::inflight::Operation::CrossTrackMove {
            moves: vec![crate::io::inflight::MovedTask {
                old_id: old_id.clone(),
                new_id: new_id.to_string(),
            }],
            source_track: source_track_id.clone(),
            target_track: target_track_id.to_string(),
        },
        "cross-track move (TUI)",
    )
    .ok();
    app.save_batch_logged(&[target_track_id, &source_track_id], false);
    if let Some(marker) = marker {
        marker.commit();
    }
    save_tracks_with_rewritten_deps(app, &[target_track_id, &source_track_id]);

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
    // Resolve the mover's namespace before any mutation so a frontier-empty
    // abort leaves both source and target untouched.
    let token = match app.resolve_mint_namespace() {
        Ok(t) => t,
        Err(()) => return,
    };

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
                t.id.as_ref().and_then(|id| {
                    if app.selection.contains(&**id) {
                        Some(id.to_string())
                    } else {
                        None
                    }
                })
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
    // (old, new) for every task in the batch, for the in-flight marker.
    let mut moved_pairs: Vec<crate::io::inflight::MovedTask> = Vec::new();

    for task_id in &selected_ids {
        // Get next ID number (must re-query each time since we're inserting)
        let target_track = match App::find_track_in_project(&app.project, target_track_id) {
            Some(t) => t,
            None => continue,
        };
        // Re-mint each moved id in the mover's namespace.
        let mint = Mint::new(
            &app.project.frame_dir,
            target_track_id,
            &target_prefix,
            token.as_ref(),
        );
        let new_num = mint.next(target_track);
        let new_id = TaskId::with_number(&target_prefix, new_num, token.as_ref());

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
        let old_subtree_ids = task_ops::subtree_ids(&task);
        task_ops::renumber_subtasks(&mut task, &new_id, token.as_ref());
        let subtree_ids: Vec<(String, String)> = old_subtree_ids
            .into_iter()
            .zip(task_ops::subtree_ids(&task))
            .collect();

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

        // Update dep references across all tracks, descendants included — see
        // the single-move path for why the root pair alone is not enough.
        let id_map = task_ops::cross_track_id_map(&old_id, &new_id.to_string(), &subtree_ids);
        task_ops::apply_id_map_to_deps(&mut app.project.tracks, &id_map);

        moved_pairs.push(crate::io::inflight::MovedTask {
            old_id: old_id.clone(),
            new_id: new_id.to_string(),
        });

        ops.push(Operation::CrossTrackMove {
            source_track_id: source_track_id.clone(),
            target_track_id: target_track_id.to_string(),
            task_id_old: old_id,
            task_id_new: new_id.to_string(),
            source_index,
            target_index,
            source_parent_id: None,
            old_depth: 0,
            // Bulk move collects only Backlog selections.
            section: SectionKind::Backlog,
            subtree_ids,
        });

        new_ids.push(new_id.to_string());
    }

    if !ops.is_empty() {
        // Target before source, under one lock, with the intent recorded — as
        // above. A bulk move carries every task in one marker, so an
        // interruption partway through the batch is recovered task by task.
        let marker = crate::io::inflight::InFlight::begin(
            &app.project.frame_dir,
            crate::io::inflight::Operation::CrossTrackMove {
                moves: moved_pairs,
                source_track: source_track_id.clone(),
                target_track: target_track_id.to_string(),
            },
            "bulk cross-track move (TUI)",
        )
        .ok();
        app.save_batch_logged(&[target_track_id, &source_track_id], false);
        if let Some(marker) = marker {
            marker.commit();
        }
        save_tracks_with_rewritten_deps(app, &[target_track_id, &source_track_id]);

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

#[cfg(test)]
mod cross_track_dep_tests {
    use super::*;
    use crate::model::config::{
        CleanConfig, IdConfig, ProjectConfig, ProjectInfo, TrackConfig, UiConfig,
    };
    use crate::tui::app::{TriageState, TriageStep};

    const SRC: &str =
        "# Src\n\n## Backlog\n\n- [ ] `S-001` Parent\n  - [ ] `S-001.1` Child\n\n## Done\n";
    const TGT: &str = "# Tgt\n\n## Backlog\n\n## Done\n";
    const OTH: &str =
        "# Oth\n\n## Backlog\n\n- [ ] `O-001` Dependent\n  - dep: S-001.1\n\n## Done\n";

    /// Three tracks on disk: the move's source and target, and a third holding
    /// the only dep — the one the move has to rewrite *and* write out.
    fn app_with_three_tracks(dir: &std::path::Path) -> App {
        let frame_dir = dir.join("frame");
        std::fs::create_dir_all(frame_dir.join("tracks")).unwrap();
        for (file, body) in [("src.md", SRC), ("tgt.md", TGT), ("oth.md", OTH)] {
            std::fs::write(frame_dir.join("tracks").join(file), body).unwrap();
        }
        std::fs::write(frame_dir.join("inbox.md"), "# Inbox\n").unwrap();

        let track_cfg = |id: &str, prefix: &str| TrackConfig {
            id: id.into(),
            name: prefix.into(),
            state: "active".into(),
            file: format!("tracks/{id}.md"),
        };
        let mut ids = IdConfig::default();
        for (id, prefix) in [("src", "S"), ("tgt", "T"), ("oth", "O")] {
            ids.prefixes.insert(id.to_string(), prefix.to_string());
        }
        let config = ProjectConfig {
            project: ProjectInfo {
                name: "deps".into(),
            },
            agent: Default::default(),
            tracks: vec![
                track_cfg("src", "S"),
                track_cfg("tgt", "T"),
                track_cfg("oth", "O"),
            ],
            clean: CleanConfig::default(),
            ids,
            ui: UiConfig::default(),
            recovery: Default::default(),
            limits: Default::default(),
        };
        let project = crate::model::project::Project {
            root: dir.to_path_buf(),
            frame_dir,
            config,
            tracks: vec![
                ("src".into(), crate::parse::parse_track(SRC)),
                ("tgt".into(), crate::parse::parse_track(TGT)),
                ("oth".into(), crate::parse::parse_track(OTH)),
            ],
            inbox: Some(crate::parse::parse_inbox("# Inbox\n").0),
        };
        App::new(project)
    }

    fn begin_move(app: &mut App) {
        app.triage_state = Some(TriageState {
            source: TriageSource::CrossTrackMove {
                source_track_id: "src".into(),
                task_id: "S-001".into(),
                section: SectionKind::Backlog,
            },
            step: TriageStep::SelectTrack,
            popup_anchor: None,
            position_cursor: 0,
        });
    }

    /// The id the move minted for the child, read back rather than assumed —
    /// this clone resolves an actor token, so the new ids carry it.
    fn moved_child_id(app: &App) -> String {
        let (_, tgt) = app
            .project
            .tracks
            .iter()
            .find(|(id, _)| id == "tgt")
            .expect("target track");
        let parent = tgt
            .section_tasks(SectionKind::Backlog)
            .first()
            .expect("the moved parent");
        parent.subtasks[0]
            .id
            .as_deref()
            .expect("the moved child has an id")
            .to_string()
    }

    /// The TUI's cross-track move, on the two counts it got wrong at once.
    ///
    /// **The dep is on a descendant**, so rewriting only the root rename left
    /// it pointing at the retired `S-001.1`. **The dependent is in a third
    /// track**, which the TUI never saved — it wrote source and target and
    /// nothing else, so even the root rewrite stayed in memory and the next
    /// reload turned it into a dangling dep. Reading the file off disk rather
    /// than the in-memory track is what makes the second half of that visible.
    #[test]
    fn a_cross_track_move_rewrites_and_saves_a_third_tracks_dep() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut app = app_with_three_tracks(tmp.path());
        let oth_path = app.project.frame_dir.join("tracks/oth.md");

        begin_move(&mut app);
        execute_cross_track_move(&mut app, "tgt", InsertPosition::Bottom);

        let child = moved_child_id(&app);
        assert!(child.starts_with("T-"), "the child was re-keyed: {child}");

        let on_disk = std::fs::read_to_string(&oth_path).unwrap();
        assert!(
            on_disk.contains(&format!("dep: {child}")),
            "the third track's dep must be rewritten *and reach disk*: {on_disk}"
        );
        assert!(
            !on_disk.contains("S-001"),
            "a retired id survived on disk: {on_disk}"
        );
    }

    /// The bulk path is a separate loop over the same steps, and had the same
    /// two holes. One selected task carries the subtree; the dep is on its
    /// child and lives in the third track.
    #[test]
    fn a_bulk_cross_track_move_rewrites_and_saves_a_third_tracks_dep() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut app = app_with_three_tracks(tmp.path());
        let oth_path = app.project.frame_dir.join("tracks/oth.md");

        app.selection.insert("S-001".to_string());
        app.triage_state = Some(TriageState {
            source: TriageSource::BulkCrossTrackMove {
                source_track_id: "src".into(),
            },
            step: TriageStep::SelectTrack,
            popup_anchor: None,
            position_cursor: 0,
        });
        execute_bulk_cross_track_move(&mut app, "tgt", InsertPosition::Bottom);

        let child = moved_child_id(&app);
        let on_disk = std::fs::read_to_string(&oth_path).unwrap();
        assert!(
            on_disk.contains(&format!("dep: {child}")),
            "bulk move: the third track's dep must be rewritten and saved: {on_disk}"
        );
    }

    /// Undo restores the dep in the third track too, and writes it back out.
    /// The undo arm reversed only the root rename and saved only source and
    /// target, so both halves of the forward fix need their counterpart here.
    #[test]
    fn undoing_a_cross_track_move_restores_a_third_tracks_dep() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut app = app_with_three_tracks(tmp.path());
        let oth_path = app.project.frame_dir.join("tracks/oth.md");

        begin_move(&mut app);
        execute_cross_track_move(&mut app, "tgt", InsertPosition::Bottom);
        let child = moved_child_id(&app);
        assert!(
            std::fs::read_to_string(&oth_path)
                .unwrap()
                .contains(&format!("dep: {child}"))
        );

        perform_undo(&mut app);

        let on_disk = std::fs::read_to_string(&oth_path).unwrap();
        assert!(
            on_disk.contains("dep: S-001.1"),
            "undo must put the dep back on disk: {on_disk}"
        );
        assert!(
            !on_disk.contains(&child),
            "the id the move assigned is gone again: {on_disk}"
        );
    }
}
