use crossterm::event::{KeyCode, KeyEvent};

use crate::model::SectionKind;
use crate::model::task::{Metadata, Task};
use crate::ops::task_ops::{self};

use crate::tui::app::{
    App, DetailRegion, EditHistory, EditTarget, Mode, PendingMove, PendingMoveKind, StateFilter,
    View,
};
use crate::tui::undo::Operation;

use super::*;

use crate::tui::command_actions::CommandPaletteState;

pub(super) fn open_command_palette(app: &mut App) {
    app.show_help = false;
    app.command_palette = Some(CommandPaletteState::new(app));
    app.mode = Mode::Command;
}

pub(super) fn handle_command(app: &mut App, key: KeyEvent) {
    match (key.modifiers, key.code) {
        (_, KeyCode::Esc) => {
            app.command_palette = None;
            app.mode = Mode::Navigate;
        }
        (_, KeyCode::Enter) => {
            if let Some(cp) = app.command_palette.take() {
                if let Some(scored) = cp.results.get(cp.selected) {
                    let action_id = scored.action.id.to_string();
                    let track_index = cp.selected_track_index();
                    app.mode = Mode::Navigate;
                    dispatch_palette_action(app, &action_id, track_index);
                } else {
                    app.mode = Mode::Navigate;
                }
            } else {
                app.mode = Mode::Navigate;
            }
        }
        (_, KeyCode::Up) => {
            if let Some(cp) = &mut app.command_palette
                && cp.selected > 0
            {
                cp.selected -= 1;
            }
        }
        (_, KeyCode::Down) => {
            if let Some(cp) = &mut app.command_palette
                && !cp.results.is_empty()
                && cp.selected < cp.results.len() - 1
            {
                cp.selected += 1;
            }
        }
        (_, KeyCode::Backspace) => {
            let should_close = app
                .command_palette
                .as_ref()
                .is_some_and(|cp| cp.input.is_empty());
            if should_close {
                app.command_palette = None;
                app.mode = Mode::Navigate;
            } else if let Some(cp) = &mut app.command_palette {
                cp.input.pop();
                cp.cursor = cp.input.len();
                cp.selected = 0;
                // Take palette out, update, put back
                let mut cp = app.command_palette.take().unwrap();
                cp.update_filter(app);
                app.command_palette = Some(cp);
            }
        }
        (_, KeyCode::Char(c)) => {
            if app.command_palette.is_some() {
                let mut cp = app.command_palette.take().unwrap();
                cp.input.push(c);
                cp.cursor = cp.input.len();
                cp.selected = 0;
                cp.update_filter(app);
                app.command_palette = Some(cp);
            }
        }
        _ => {}
    }
}

pub(super) fn dispatch_palette_action(app: &mut App, action_id: &str, track_index: Option<usize>) {
    match action_id {
        // Global
        "switch_track" => {
            if let Some(idx) = track_index
                && idx < app.active_track_ids.len()
            {
                app.close_detail_fully();
                app.view = View::Track(idx);
            }
        }
        "next_track" => {
            switch_tab(app, 1);
        }
        "open_inbox" => {
            app.close_detail_fully();
            app.view = View::Inbox;
        }
        "open_recent" => {
            app.close_detail_fully();
            app.view = View::Recent;
        }
        "open_tracks" => {
            app.close_detail_fully();
            app.tracks_name_col_min = 0;
            app.view = View::Tracks;
        }
        "search" => {
            app.mode = Mode::Search;
            app.search_input.clear();
            app.search_draft.clear();
            app.search_history_index = None;
            app.search_wrap_message = None;
            app.search_match_count = None;
            app.search_zero_confirmed = false;
        }
        "jump_to_task" => {
            begin_jump_to(app);
        }
        "show_deps" => {
            if matches!(app.view, View::Track(_)) {
                open_dep_popup_from_track_view(app);
            } else if matches!(app.view, View::Detail { .. }) {
                open_dep_popup_from_detail_view(app);
            }
        }
        "tag_colors" => {
            app.open_tag_color_popup();
        }
        "projects" => {
            open_project_picker(app);
        }
        "toggle_help" => {
            app.show_help = !app.show_help;
            app.help_scroll = 0;
        }
        "toggle_note_wrap" => {
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
        "undo" => {
            perform_undo(app);
        }
        "redo" => {
            perform_redo(app);
        }
        "quit" => {
            app.should_quit = true;
        }

        // Track view: state changes
        "cycle_state" => {
            if matches!(app.view, View::Recent) {
                reopen_recent_task(app);
            } else {
                task_state_action(app, StateAction::Cycle);
            }
        }
        "set_todo" => {
            task_state_action(app, StateAction::SetTodo);
        }
        "mark_done" => {
            if matches!(app.view, View::Inbox) {
                inbox_delete_item(app);
            } else {
                task_state_action(app, StateAction::Done);
            }
        }
        "set_blocked" => {
            task_state_action(app, StateAction::ToggleBlocked);
        }
        "set_parked" => {
            task_state_action(app, StateAction::ToggleParked);
        }
        "toggle_cc" => {
            toggle_cc_tag(app);
        }
        "mark_done_wontdo" => {
            compound_done_with_tag(app, "wontdo");
        }
        "mark_done_duplicate" => {
            compound_done_with_tag(app, "duplicate");
        }

        // Track view: create
        "add_task_bottom" | "add_inbox_item" => {
            if matches!(app.view, View::Inbox) {
                inbox_add_item(app);
            } else if matches!(app.view, View::Tracks) {
                tracks_add_track(app);
            } else {
                add_task_action(app, AddPosition::Bottom);
            }
        }
        "append_to_group" => {
            append_sibling_action(app);
        }
        "insert_after" => {
            if matches!(app.view, View::Inbox) {
                inbox_insert_after(app);
            } else if matches!(app.view, View::Tracks) {
                tracks_insert_after(app);
            } else {
                add_task_action(app, AddPosition::AfterCursor);
            }
        }
        "push_to_top" => {
            if matches!(app.view, View::Inbox) {
                inbox_prepend_item(app);
            } else if matches!(app.view, View::Tracks) {
                tracks_prepend(app);
            } else {
                add_task_action(app, AddPosition::Top);
            }
        }
        "add_subtask" => {
            add_subtask_action(app);
        }

        // Track view: edit
        "edit_title" => {
            if matches!(app.view, View::Inbox) {
                inbox_edit_title(app);
            } else if matches!(app.view, View::Tracks) {
                tracks_edit_name(app);
            } else {
                enter_title_edit(app);
            }
        }
        "edit_tags" => {
            if matches!(app.view, View::Detail { .. }) {
                detail_jump_to_region_and_edit(app, DetailRegion::Tags, false);
            } else if matches!(app.view, View::Inbox) {
                inbox_edit_tags(app);
            } else {
                enter_tag_edit(app);
            }
        }

        // Track view: move
        "move_task" => {
            if matches!(app.view, View::Inbox) {
                inbox_enter_move_mode(app);
            } else {
                enter_move_mode(app);
            }
        }
        "move_to_track" => {
            begin_cross_track_move(app);
        }
        "move_to_top" => {
            palette_move_to_boundary(app, true);
        }
        "move_to_bottom" => {
            palette_move_to_boundary(app, false);
        }

        // Track view: filter
        "filter_active" => {
            let prev = get_cursor_task_id(app);
            app.filter_state.state_filter = Some(StateFilter::Active);
            reset_cursor_for_filter(app, prev.as_deref());
        }
        "filter_todo" => {
            let prev = get_cursor_task_id(app);
            app.filter_state.state_filter = Some(StateFilter::Todo);
            reset_cursor_for_filter(app, prev.as_deref());
        }
        "filter_blocked" => {
            let prev = get_cursor_task_id(app);
            app.filter_state.state_filter = Some(StateFilter::Blocked);
            reset_cursor_for_filter(app, prev.as_deref());
        }
        "filter_ready" => {
            let prev = get_cursor_task_id(app);
            app.filter_state.state_filter = Some(StateFilter::Ready);
            reset_cursor_for_filter(app, prev.as_deref());
        }
        "filter_tag" => {
            begin_filter_tag_select(app);
        }
        "clear_state_filter" => {
            let prev = get_cursor_task_id(app);
            app.filter_state.state_filter = None;
            reset_cursor_for_filter(app, prev.as_deref());
        }
        "clear_all_filters" => {
            let prev = get_cursor_task_id(app);
            app.filter_state.state_filter = None;
            app.filter_state.tag_filter = None;
            reset_cursor_for_filter(app, prev.as_deref());
        }

        // Track view: select
        "toggle_select" => {
            enter_select_mode(app);
        }
        "range_select" => {
            begin_range_select(app);
        }
        "select_all" => {
            select_all(app);
        }
        "select_none" => {
            clear_selection(app);
        }

        // Track view: navigate
        "open_detail" => {
            if matches!(app.view, View::Inbox) {
                inbox_begin_triage(app);
            } else if matches!(app.view, View::Recent) {
                open_recent_detail(app);
            } else {
                handle_enter(app);
            }
        }
        "collapse_all" => {
            palette_collapse_all(app);
        }
        "expand_all" => {
            palette_expand_all(app);
        }

        // Track view: manage
        "set_cc_focus" => {
            set_cc_focus_current(app);
        }
        "repeat_action" => {
            repeat_last_action(app);
        }

        // Detail view
        "edit_region" => {
            detail_enter_edit(app, false);
        }
        "edit_refs" => {
            detail_jump_to_region_and_edit(app, DetailRegion::Refs, false);
        }
        "edit_deps" => {
            detail_jump_to_region_and_edit(app, DetailRegion::Deps, false);
        }
        "edit_note" => {
            if matches!(app.view, View::Inbox) {
                inbox_edit_note(app, true);
            } else {
                detail_jump_to_region_and_edit(app, DetailRegion::Note, true);
            }
        }
        "edit_note_from_start" => {
            if matches!(app.view, View::Inbox) {
                inbox_edit_note(app, false);
            } else {
                detail_jump_to_region_and_edit(app, DetailRegion::Note, false);
            }
        }
        "back_to_track" => {
            // Simulate Esc in detail view
            if let View::Detail { .. } = &app.view {
                if let Some((parent_track, parent_task)) = app.detail_stack.pop() {
                    app.detail_state = None;
                    app.view = View::Detail {
                        track_id: parent_track,
                        task_id: parent_task,
                    };
                } else {
                    let return_view = app
                        .detail_state
                        .as_ref()
                        .map(|ds| ds.return_view.clone())
                        .unwrap_or(crate::tui::app::ReturnView::Track(0));
                    match return_view {
                        crate::tui::app::ReturnView::Track(idx) => app.view = View::Track(idx),
                        crate::tui::app::ReturnView::Recent => app.view = View::Recent,
                    }
                    app.close_detail_fully();
                }
            }
        }

        // Inbox view
        "delete_inbox_item" => {
            inbox_delete_item(app);
        }
        "begin_triage" => {
            inbox_begin_triage(app);
        }

        // Recent view
        "reopen_todo" => {
            reopen_recent_task(app);
        }
        "expand_subtasks" => {
            expand_recent(app);
        }
        "collapse_subtasks" => {
            collapse_recent(app);
        }

        // Tracks view
        "open_track" => {
            handle_enter(app);
        }
        "add_track" => {
            tracks_add_track(app);
        }
        "edit_track_name" => {
            tracks_edit_name(app);
        }
        "shelve_activate" => {
            tracks_toggle_shelve(app);
        }
        "archive_track" => {
            palette_archive_track(app);
        }
        "delete_track" => {
            palette_delete_track(app);
        }
        "reorder_track" => {
            enter_move_mode(app);
        }
        "rename_prefix" => {
            tracks_rename_prefix(app);
        }

        "view_recovery_log" => {
            open_recovery_overlay(app);
        }
        "delete_task" => {
            palette_delete_task(app);
        }
        "check_project" => {
            palette_check_project(app);
        }
        "prune_recovery" => {
            palette_prune_recovery(app);
        }
        "unarchive_track" => {
            palette_unarchive_track(app);
        }
        "import_tasks" => {
            palette_import_tasks(app);
        }
        "preview_clean" => {
            palette_preview_clean(app);
        }

        _ => {}
    }
}

/// Compound action: add a tag and mark done in one step (palette-only).
pub(super) fn compound_done_with_tag(app: &mut App, tag: &str) {
    // Only works on track view tasks
    let (track_id, task_id, _section) = match app.cursor_task_id() {
        Some(info) => info,
        None => return,
    };

    let track = match app.find_track_mut(&track_id) {
        Some(t) => t,
        None => return,
    };
    let task = match crate::ops::task_ops::find_task_mut_in_track(track, &task_id) {
        Some(t) => t,
        None => return,
    };

    let old_state = task.state;
    let old_tags: Vec<String> = task.tags.clone();
    let old_resolved = task.metadata.iter().find_map(|m| {
        if let Metadata::Resolved(d) = m {
            Some(d.clone())
        } else {
            None
        }
    });

    // Add the tag if not present
    if !task.tags.iter().any(|t| t == tag) {
        task.tags.push(tag.to_string());
        task.dirty = true;
    }

    // Set done
    task_ops::set_done(task);

    let new_state = task.state;
    let new_tags = task.tags.clone();
    let new_resolved = task.metadata.iter().find_map(|m| {
        if let Metadata::Resolved(d) = m {
            Some(d.clone())
        } else {
            None
        }
    });

    // Push undo for tag change first (so it undoes second)
    if old_tags != new_tags {
        let old_val = old_tags
            .iter()
            .map(|t| format!("#{}", t))
            .collect::<Vec<_>>()
            .join(" ");
        let new_val = new_tags
            .iter()
            .map(|t| format!("#{}", t))
            .collect::<Vec<_>>()
            .join(" ");
        app.undo_stack.push(Operation::FieldEdit {
            track_id: track_id.clone(),
            task_id: task_id.clone(),
            field: "tags".to_string(),
            old_value: old_val,
            new_value: new_val,
        });
    }

    // Push undo for state change
    if old_state != new_state {
        app.undo_stack.push(Operation::StateChange {
            track_id: track_id.clone(),
            task_id: task_id.clone(),
            old_state,
            new_state,
            old_resolved,
            new_resolved,
        });

        // Schedule pending move to Done section if appropriate
        if new_state == crate::model::TaskState::Done {
            let is_top_level_backlog = task_ops::is_top_level_in_section(
                App::find_track_in_project(&app.project, &track_id).unwrap(),
                &task_id,
                SectionKind::Backlog,
            );
            if is_top_level_backlog {
                app.pending_moves.push(PendingMove {
                    kind: PendingMoveKind::ToDone,
                    track_id: track_id.clone(),
                    task_id: task_id.clone(),
                    deadline: std::time::Instant::now() + std::time::Duration::from_secs(5),
                });
            }
        }
    }

    let _ = app.save_track(&track_id);
}

/// Move the cursor task to the top or bottom of the backlog (palette-only, skips MOVE mode).
pub(super) fn palette_move_to_boundary(app: &mut App, to_top: bool) {
    let (track_id, task_id, section) = match app.cursor_task_id() {
        Some(info) => info,
        None => return,
    };
    if section != SectionKind::Backlog {
        return;
    }
    let track = match app.find_track_mut(&track_id) {
        Some(t) => t,
        None => return,
    };
    let backlog = match track.section_tasks_mut(SectionKind::Backlog) {
        Some(b) => b,
        None => return,
    };
    let current_idx = match backlog
        .iter()
        .position(|t| t.id.as_deref() == Some(&task_id))
    {
        Some(i) => i,
        None => return,
    };
    let target_idx = if to_top { 0 } else { backlog.len() - 1 };
    if current_idx == target_idx {
        return;
    }
    let task = backlog.remove(current_idx);
    let new_idx = if to_top { 0 } else { backlog.len() };
    backlog.insert(new_idx, task);
    let _ = app.save_track(&track_id);

    app.undo_stack.push(Operation::TaskMove {
        track_id,
        task_id,
        parent_id: None,
        old_index: current_idx,
        new_index: new_idx,
    });
}

/// Collapse all expanded tasks in the current track view.
pub(super) fn palette_collapse_all(app: &mut App) {
    let track_id = match app.current_track_id() {
        Some(id) => id.to_string(),
        None => return,
    };
    if let Some(state) = app.track_states.get_mut(&track_id) {
        state.expanded.clear();
    }
}

/// Expand all tasks in the current track view.
pub(super) fn palette_expand_all(app: &mut App) {
    let track_id = match app.current_track_id() {
        Some(id) => id.to_string(),
        None => return,
    };
    let track = match App::find_track_in_project(&app.project, &track_id) {
        Some(t) => t,
        None => return,
    };
    // Collect all expand keys for tasks with children
    let mut keys_to_expand = Vec::new();
    fn collect_expand_keys(
        tasks: &[crate::model::Task],
        section: SectionKind,
        path: &mut Vec<usize>,
        keys: &mut Vec<String>,
    ) {
        for (i, task) in tasks.iter().enumerate() {
            path.push(i);
            if !task.subtasks.is_empty() {
                keys.push(crate::tui::app::task_expand_key(task, section, path));
                collect_expand_keys(&task.subtasks, section, path, keys);
            }
            path.pop();
        }
    }
    let mut path = Vec::new();
    collect_expand_keys(
        track.backlog(),
        SectionKind::Backlog,
        &mut path,
        &mut keys_to_expand,
    );
    collect_expand_keys(
        track.parked(),
        SectionKind::Parked,
        &mut path,
        &mut keys_to_expand,
    );

    if let Some(state) = app.track_states.get_mut(&track_id) {
        for key in keys_to_expand {
            state.expanded.insert(key);
        }
    }
}

// ---------------------------------------------------------------------------
// Dep popup

pub(super) fn palette_check_project(app: &mut App) {
    use crate::ops::check;
    use ratatui::style::{Modifier, Style};
    use ratatui::text::{Line, Span};

    let result = check::check_project(&app.project);
    let bg = app.theme.background;
    let mut lines: Vec<Line<'static>> = Vec::new();

    let green = Style::default().fg(app.theme.green).bg(bg);
    let yellow = Style::default().fg(app.theme.highlight).bg(bg);
    let red = Style::default().fg(app.theme.red).bg(bg);
    let dim = Style::default().fg(app.theme.dim).bg(bg);
    let text = Style::default().fg(app.theme.text).bg(bg);

    if !result.errors.is_empty() {
        lines.push(Line::from(Span::styled(
            "Errors",
            red.add_modifier(Modifier::BOLD),
        )));
        for err in &result.errors {
            let msg = match err {
                check::CheckError::DanglingDep {
                    track_id,
                    task_id,
                    dep_id,
                } => format!("  [{}] {} has dangling dep: {}", track_id, task_id, dep_id),
                check::CheckError::BrokenRef {
                    track_id,
                    task_id,
                    path,
                } => format!("  [{}] {} has broken ref: {}", track_id, task_id, path),
                check::CheckError::BrokenSpec {
                    track_id,
                    task_id,
                    path,
                } => format!("  [{}] {} has broken spec: {}", track_id, task_id, path),
                check::CheckError::DuplicateId { task_id, track_ids } => {
                    format!("  {} duplicated in: {}", task_id, track_ids.join(", "))
                }
            };
            lines.push(Line::from(Span::styled(msg, red)));
        }
        lines.push(Line::from(""));
    }

    if !result.warnings.is_empty() {
        lines.push(Line::from(Span::styled(
            "Warnings",
            yellow.add_modifier(Modifier::BOLD),
        )));
        for warn in &result.warnings {
            let msg = match warn {
                check::CheckWarning::MissingId { track_id, title } => {
                    format!("  [{}] task missing ID: \"{}\"", track_id, title)
                }
                check::CheckWarning::MissingAddedDate { track_id, task_id } => {
                    format!("  [{}] {} missing added date", track_id, task_id)
                }
                check::CheckWarning::MissingResolvedDate { track_id, task_id } => {
                    format!("  [{}] {} (done) missing resolved date", track_id, task_id)
                }
                check::CheckWarning::DoneInBacklog { track_id, task_id } => {
                    format!("  [{}] {} done but in backlog section", track_id, task_id)
                }
                check::CheckWarning::LostTask { track_id, task_id } => {
                    format!("  [{}] {} has #lost tag", track_id, task_id)
                }
            };
            lines.push(Line::from(Span::styled(msg, yellow)));
        }
        lines.push(Line::from(""));
    }

    if !result.info.is_empty() {
        for info in &result.info {
            match info {
                check::CheckInfo::RecoveryLog {
                    entry_count,
                    oldest,
                } => {
                    let entry_word = if *entry_count == 1 {
                        "entry"
                    } else {
                        "entries"
                    };
                    lines.push(Line::from(Span::styled(
                        format!(
                            "Recovery log: {} {} (oldest: {})",
                            entry_count, entry_word, oldest
                        ),
                        dim,
                    )));
                }
            }
        }
        lines.push(Line::from(""));
    }

    if result.valid {
        lines.push(Line::from(Span::styled("Project is valid", green)));
    } else {
        lines.push(Line::from(Span::styled("Project has errors", red)));
    }

    // Summary counts
    lines.push(Line::from(Span::styled(
        format!(
            "{} errors, {} warnings",
            result.errors.len(),
            result.warnings.len()
        ),
        text,
    )));

    app.results_overlay_title = "Check Project".to_string();
    app.results_overlay_lines = lines;
    app.results_overlay_scroll = 0;
    app.show_results_overlay = true;
}

// ---------------------------------------------------------------------------
// Preview clean (palette action)

pub(super) fn palette_preview_clean(app: &mut App) {
    use crate::ops::clean;
    use ratatui::style::{Modifier, Style};
    use ratatui::text::{Line, Span};

    // Clone project so we don't mutate the real one
    let mut project_clone = app.project.clone();
    let result = clean::clean_project(&mut project_clone);

    let bg = app.theme.background;
    let mut lines: Vec<Line<'static>> = Vec::new();

    let highlight = Style::default().fg(app.theme.highlight).bg(bg);
    let text = Style::default().fg(app.theme.text).bg(bg);
    let dim = Style::default().fg(app.theme.dim).bg(bg);
    let green = Style::default().fg(app.theme.green).bg(bg);
    let bold = |s: Style| s.add_modifier(Modifier::BOLD);

    if !result.ids_assigned.is_empty() {
        lines.push(Line::from(Span::styled("IDs assigned", bold(highlight))));
        for a in &result.ids_assigned {
            lines.push(Line::from(Span::styled(
                format!("  [{}] {} <- \"{}\"", a.track_id, a.assigned_id, a.title),
                text,
            )));
        }
        lines.push(Line::from(""));
    }

    if !result.dates_assigned.is_empty() {
        lines.push(Line::from(Span::styled("Dates assigned", bold(highlight))));
        for d in &result.dates_assigned {
            lines.push(Line::from(Span::styled(
                format!("  [{}] {} <- {}", d.track_id, d.task_id, d.date),
                text,
            )));
        }
        lines.push(Line::from(""));
    }

    if !result.duplicates_resolved.is_empty() {
        lines.push(Line::from(Span::styled(
            "Duplicate IDs resolved",
            bold(highlight),
        )));
        for d in &result.duplicates_resolved {
            lines.push(Line::from(Span::styled(
                format!(
                    "  [{}] {} -> {} \"{}\"",
                    d.track_id, d.original_id, d.new_id, d.title
                ),
                text,
            )));
        }
        lines.push(Line::from(""));
    }

    if !result.sections_reconciled.is_empty() {
        lines.push(Line::from(Span::styled(
            "Sections reconciled",
            bold(highlight),
        )));
        for s in &result.sections_reconciled {
            lines.push(Line::from(Span::styled(
                format!(
                    "  [{}] {} moved {} -> {}",
                    s.track_id, s.task_id, s.from, s.to
                ),
                text,
            )));
        }
        lines.push(Line::from(""));
    }

    if !result.tasks_archived.is_empty() {
        lines.push(Line::from(Span::styled(
            "Tasks to archive",
            bold(highlight),
        )));
        for a in &result.tasks_archived {
            lines.push(Line::from(Span::styled(
                format!("  [{}] {} \"{}\"", a.track_id, a.task_id, a.title),
                text,
            )));
        }
        lines.push(Line::from(""));
    }

    if !result.dangling_deps.is_empty() {
        lines.push(Line::from(Span::styled(
            "Dangling dependencies",
            bold(highlight),
        )));
        for d in &result.dangling_deps {
            lines.push(Line::from(Span::styled(
                format!(
                    "  [{}] {} -> {} (not found)",
                    d.track_id, d.task_id, d.dep_id
                ),
                text,
            )));
        }
        lines.push(Line::from(""));
    }

    if !result.broken_refs.is_empty() {
        lines.push(Line::from(Span::styled(
            "Broken references",
            bold(highlight),
        )));
        for r in &result.broken_refs {
            lines.push(Line::from(Span::styled(
                format!("  [{}] {} -> {} (not found)", r.track_id, r.task_id, r.path),
                text,
            )));
        }
        lines.push(Line::from(""));
    }

    if !result.suggestions.is_empty() {
        lines.push(Line::from(Span::styled("Suggestions", bold(highlight))));
        for s in &result.suggestions {
            let msg = match s.kind {
                clean::SuggestionKind::AllSubtasksDone => "all subtasks done",
            };
            lines.push(Line::from(Span::styled(
                format!("  [{}] {} — {}", s.track_id, s.task_id, msg),
                text,
            )));
        }
        lines.push(Line::from(""));
    }

    let total = result.ids_assigned.len()
        + result.dates_assigned.len()
        + result.duplicates_resolved.len()
        + result.tasks_archived.len();

    if total == 0
        && result.dangling_deps.is_empty()
        && result.broken_refs.is_empty()
        && result.suggestions.is_empty()
    {
        lines.push(Line::from(Span::styled("Project is clean", green)));
    } else {
        lines.push(Line::from(Span::styled(
            format!("{} changes would be applied by `fr clean`", total),
            dim,
        )));
    }

    app.results_overlay_title = "Preview Clean".to_string();
    app.results_overlay_lines = lines;
    app.results_overlay_scroll = 0;
    app.show_results_overlay = true;
}

// ---------------------------------------------------------------------------
// Prune recovery log (palette action)

pub(super) fn palette_prune_recovery(app: &mut App) {
    use crate::io::recovery;

    let entries = recovery::read_recovery_entries(&app.project.frame_dir, None, None);
    if entries.is_empty() {
        app.status_message = Some("Recovery log is empty".into());
        return;
    }

    // Count how many are older than 30 days
    let cutoff = chrono::Utc::now() - chrono::Duration::days(30);
    let prunable = entries.iter().filter(|e| e.timestamp < cutoff).count();

    if prunable == 0 {
        app.status_message = Some(format!(
            "{} entries, all < 30 days — nothing to prune",
            entries.len()
        ));
        return;
    }

    let msg = format!(
        "Prune {} of {} entries older than 30 days? (y/n)",
        prunable,
        entries.len()
    );

    app.confirm_state = Some(crate::tui::app::ConfirmState {
        message: msg,
        action: crate::tui::app::ConfirmAction::PruneRecovery,
    });
    app.mode = Mode::Confirm;
}

pub(super) fn confirm_prune_recovery(app: &mut App) {
    use crate::io::recovery;

    match recovery::prune_recovery(&app.project.frame_dir, None, false) {
        Ok(count) => {
            app.status_message = Some(format!("Pruned {} recovery entries", count));
        }
        Err(e) => {
            app.status_message = Some(format!("Prune failed: {}", e));
        }
    }
}

// ---------------------------------------------------------------------------
// Unarchive track (palette action)

pub(super) fn palette_unarchive_track(app: &mut App) {
    let track_id = match tracks_cursor_track_id(app) {
        Some(id) => id,
        None => return,
    };

    // Only works on archived tracks
    let is_archived = app
        .project
        .config
        .tracks
        .iter()
        .any(|tc| tc.id == track_id && tc.state == "archived");
    if !is_archived {
        return;
    }

    let display_name = app.track_name(&track_id).to_string();
    app.confirm_state = Some(crate::tui::app::ConfirmState {
        message: format!("Unarchive track \"{}\"? [y/n]", display_name),
        action: crate::tui::app::ConfirmAction::UnarchiveTrack { track_id },
    });
    app.mode = Mode::Confirm;
}

pub(super) fn confirm_unarchive_track(app: &mut App, track_id: &str) {
    let track_name = app.track_name(track_id).to_string();

    // Set config state to "active"
    if let Some(tc) = app
        .project
        .config
        .tracks
        .iter_mut()
        .find(|t| t.id == track_id)
    {
        tc.state = "active".to_string();
    }
    save_config(app);

    // Restore track file from archive/_tracks/
    if let Some(file) = app.track_file(track_id).map(|f| f.to_string()) {
        let _ = crate::ops::track_ops::restore_track_file(&app.project.frame_dir, track_id, &file);
    }

    // Reload track into memory
    if let Some(new_track) = app.read_track_from_disk(track_id) {
        if !app.project.tracks.iter().any(|(id, _)| id == track_id) {
            app.project.tracks.push((track_id.to_string(), new_track));
        } else {
            app.replace_track(track_id, new_track);
        }
    }

    rebuild_active_track_ids(app);

    app.undo_stack.push(Operation::TrackUnarchive {
        track_id: track_id.to_string(),
    });

    app.status_message = Some(format!("unarchived \"{}\"", track_name));
}

// ---------------------------------------------------------------------------
// Import tasks (palette action)

pub(super) fn palette_import_tasks(app: &mut App) {
    let track_id = match app.current_track_id() {
        Some(id) => id.to_string(),
        None => return,
    };

    app.edit_target = Some(EditTarget::ImportFilePath { track_id });
    app.edit_buffer.clear();
    app.edit_cursor = 0;
    app.edit_history = Some(EditHistory::default());
    app.mode = Mode::Edit;
    app.status_message = Some("Import from: ".into());
}

pub(super) fn confirm_import_tasks(app: &mut App, track_id: &str, file_path: &str) {
    use crate::ops::import;

    let prefix = match app.track_prefix(track_id) {
        Some(p) => p.to_string(),
        None => {
            app.status_message = Some("No ID prefix configured for this track".into());
            app.status_is_error = true;
            return;
        }
    };

    let markdown = match std::fs::read_to_string(file_path) {
        Ok(c) => c,
        Err(e) => {
            app.status_message = Some(format!("Cannot read file: {}", e));
            app.status_is_error = true;
            return;
        }
    };

    let track = match app.project.tracks.iter_mut().find(|(id, _)| id == track_id) {
        Some((_, t)) => t,
        None => return,
    };

    // Record position before import (top of backlog)
    let position = track.backlog().len();

    match import::import_tasks(&markdown, track, task_ops::InsertPosition::Bottom, &prefix) {
        Ok(result) => {
            let count = result.total_count;
            let top_level = result.assigned_ids.len();

            // Collect tasks for undo
            let backlog = track.backlog();
            let imported_tasks: Vec<Task> = backlog[position..].to_vec();

            app.undo_stack.push(Operation::Import {
                track_id: track_id.to_string(),
                position,
                count: imported_tasks.len(),
                tasks: imported_tasks,
            });

            let _ = app.save_track(track_id);

            app.status_message = Some(format!(
                "Imported {} tasks ({} top-level)",
                count, top_level
            ));
        }
        Err(e) => {
            app.status_message = Some(format!("Import failed: {}", e));
            app.status_is_error = true;
        }
    }
}

// ---------------------------------------------------------------------------
// Task deletion (palette action)

pub(super) fn palette_delete_task(app: &mut App) {
    use crate::ops::task_ops;

    // Check for bulk selection first
    if !app.selection.is_empty() {
        let mut task_ids: Vec<(String, String)> = Vec::new();
        for selected_id in &app.selection {
            // Find which track contains this task
            for (track_id, track) in &app.project.tracks {
                if task_ops::find_task_in_track(track, selected_id).is_some() {
                    task_ids.push((track_id.clone(), selected_id.clone()));
                    break;
                }
            }
        }
        if task_ids.is_empty() {
            return;
        }
        let msg = format!("Delete {} tasks permanently? (y/n)", task_ids.len());
        app.confirm_state = Some(crate::tui::app::ConfirmState {
            message: msg,
            action: crate::tui::app::ConfirmAction::BulkDeleteTasks { task_ids },
        });
        app.mode = Mode::Confirm;
        return;
    }

    // Single task: get from current view
    let (track_id, task_id) = if let View::Detail { track_id, task_id } = &app.view {
        (track_id.clone(), task_id.clone())
    } else if let Some((track_id, task_id, _section)) = app.cursor_task_id() {
        (track_id, task_id)
    } else {
        return;
    };

    // Build confirmation message with subtree size
    let track = match App::find_track_in_project(&app.project, &track_id) {
        Some(t) => t,
        None => return,
    };
    let task = match task_ops::find_task_in_track(track, &task_id) {
        Some(t) => t,
        None => return,
    };
    let subtree_size = task_ops::count_subtree_size(task);
    let label = task.id.as_deref().unwrap_or(&task.title);
    let msg = if subtree_size > 1 {
        format!(
            "Delete {} and {} subtask(s) permanently? (y/n)",
            label,
            subtree_size - 1
        )
    } else {
        format!("Delete {} permanently? (y/n)", label)
    };

    app.confirm_state = Some(crate::tui::app::ConfirmState {
        message: msg,
        action: crate::tui::app::ConfirmAction::DeleteTask { track_id, task_id },
    });
    app.mode = Mode::Confirm;
}

pub(super) fn confirm_delete_task(app: &mut App, track_id: &str, task_id: &str) {
    use crate::io::recovery;
    use crate::ops::task_ops;

    // Serialize for recovery before deletion
    let track = match App::find_track_in_project(&app.project, track_id) {
        Some(t) => t,
        None => return,
    };
    let task = match task_ops::find_task_in_track(track, task_id) {
        Some(t) => t,
        None => return,
    };
    let source_text = crate::parse::serialize_tasks(std::slice::from_ref(task), 0).join("\n");

    // Perform deletion
    let track = match app.find_track_mut(track_id) {
        Some(t) => t,
        None => return,
    };
    let deleted = match task_ops::hard_delete_task(track, task_id, track_id) {
        Ok(d) => d,
        Err(_) => return,
    };

    // Log to recovery
    recovery::log_task_deletion(&app.project.frame_dir, task_id, track_id, &source_text);

    // Push undo
    app.undo_stack.push(Operation::TaskDelete {
        track_id: deleted.track_id.clone(),
        section: deleted.section,
        parent_id: deleted.parent_id,
        position: deleted.position,
        task: deleted.task,
    });

    // Save
    let _ = app.save_track(track_id);

    // Navigate away from detail view if we deleted the viewed task
    if let View::Detail {
        task_id: view_task_id,
        ..
    } = &app.view
        && view_task_id == task_id
    {
        if let Some((parent_track, parent_task)) = app.detail_stack.pop() {
            app.detail_state = None;
            app.view = View::Detail {
                track_id: parent_track,
                task_id: parent_task,
            };
        } else {
            let return_view = app
                .detail_state
                .as_ref()
                .map(|ds| ds.return_view.clone())
                .unwrap_or(crate::tui::app::ReturnView::Track(0));
            match return_view {
                crate::tui::app::ReturnView::Track(idx) => app.view = View::Track(idx),
                crate::tui::app::ReturnView::Recent => app.view = View::Recent,
            }
            app.close_detail_fully();
        }
    }

    app.status_message = Some(format!("Deleted {}", task_id));
}

pub(super) fn confirm_bulk_delete_tasks(app: &mut App, task_ids: &[(String, String)]) {
    use crate::io::recovery;
    use crate::ops::task_ops;

    let mut deletions: Vec<(String, SectionKind, Option<String>, usize, Task)> = Vec::new();
    let mut tracks_to_save = std::collections::HashSet::new();

    // Collect info and delete each task (process in order — we'll sort positions descending for undo)
    for (track_id, task_id) in task_ids {
        // Serialize for recovery
        let track = match App::find_track_in_project(&app.project, track_id) {
            Some(t) => t,
            None => continue,
        };
        let task = match task_ops::find_task_in_track(track, task_id) {
            Some(t) => t,
            None => continue,
        };
        let source_text = crate::parse::serialize_tasks(std::slice::from_ref(task), 0).join("\n");

        let track = match app.find_track_mut(track_id) {
            Some(t) => t,
            None => continue,
        };
        match task_ops::hard_delete_task(track, task_id, track_id) {
            Ok(deleted) => {
                recovery::log_task_deletion(
                    &app.project.frame_dir,
                    task_id,
                    track_id,
                    &source_text,
                );
                deletions.push((
                    deleted.track_id.clone(),
                    deleted.section,
                    deleted.parent_id,
                    deleted.position,
                    deleted.task,
                ));
                tracks_to_save.insert(track_id.clone());
            }
            Err(_) => continue,
        }
    }

    if !deletions.is_empty() {
        let count = deletions.len();
        // Sort by position descending for correct undo reinsertion order
        deletions.sort_by(|a, b| b.3.cmp(&a.3));
        app.undo_stack.push(Operation::BulkTaskDelete { deletions });

        for track_id in &tracks_to_save {
            let _ = app.save_track(track_id);
        }

        app.selection.clear();
        app.status_message = Some(format!("Deleted {} tasks", count));
    }
}
