use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::model::SectionKind;
use crate::model::task::Metadata;
use crate::ops::task_ops::{self};

use crate::tui::app::{
    App, AutocompleteKind, AutocompleteState, DetailRegion, EditHistory, EditTarget, FlatItem,
    Mode, MoveState, PendingMove, PendingMoveKind, RepeatableAction, TriageSource, View,
    resolve_task_from_flat,
};
use crate::tui::undo::Operation;

use super::*;

/// Enter SELECT mode and toggle the task under the cursor.
pub(super) fn enter_select_mode(app: &mut App) {
    if let Some((_, task_id, _)) = app.cursor_task_id() {
        app.selection.insert(task_id);
        app.mode = Mode::Select;
    }
}

/// Begin range selection: set anchor at current cursor position.
pub(super) fn begin_range_select(app: &mut App) {
    let track_id = match app.current_track_id() {
        Some(id) => id.to_string(),
        None => return,
    };
    let cursor = app.track_states.get(&track_id).map_or(0, |s| s.cursor);

    // Select current task and set anchor
    if let Some((_, task_id, _)) = app.cursor_task_id() {
        app.selection.insert(task_id);
    }
    app.range_anchor = Some(cursor);
    app.mode = Mode::Select;
}

/// Finalize range selection: select all items between anchor and cursor, clear anchor.
pub(super) fn finalize_range_select(app: &mut App) {
    let track_id = match app.current_track_id() {
        Some(id) => id.to_string(),
        None => return,
    };
    let flat_items = app.build_flat_items(&track_id);
    let cursor = app.track_states.get(&track_id).map_or(0, |s| s.cursor);
    let anchor = match app.range_anchor {
        Some(a) => a,
        None => return,
    };

    let track = match App::find_track_in_project(&app.project, &track_id) {
        Some(t) => t,
        None => return,
    };

    let (start, end) = if cursor <= anchor {
        (cursor, anchor)
    } else {
        (anchor, cursor)
    };

    for i in start..=end {
        if let Some(FlatItem::Task {
            section,
            path,
            is_context,
            ..
        }) = flat_items.get(i)
        {
            if *is_context {
                continue;
            }
            if let Some(task) = resolve_task_from_flat(track, *section, path)
                && let Some(id) = &task.id
            {
                app.selection.insert(id.clone());
            }
        }
    }

    app.range_anchor = None;
}

/// Select all visible (non-context, non-separator) tasks in the current track view.
pub(super) fn select_all(app: &mut App) {
    let track_id = match app.current_track_id() {
        Some(id) => id.to_string(),
        None => return,
    };
    let flat_items = app.build_flat_items(&track_id);
    let track = match App::find_track_in_project(&app.project, &track_id) {
        Some(t) => t,
        None => return,
    };

    app.selection.clear();
    for item in &flat_items {
        if let FlatItem::Task {
            section,
            path,
            is_context,
            ..
        } = item
        {
            if *is_context {
                continue;
            }
            if let Some(task) = resolve_task_from_flat(track, *section, path)
                && let Some(id) = &task.id
            {
                app.selection.insert(id.clone());
            }
        }
    }

    if !app.selection.is_empty() {
        app.mode = Mode::Select;
    }
}

/// Handle keys in SELECT mode.
pub(super) fn handle_select(app: &mut App, key: KeyEvent) {
    // Conflict popup intercepts Esc
    if app.conflict_text.is_some() {
        if matches!(key.code, KeyCode::Esc) {
            if let Some(ref text) = app.conflict_text {
                crate::io::recovery::log_recovery(
                    &app.project.frame_dir,
                    crate::io::recovery::RecoveryEntry {
                        timestamp: chrono::Utc::now(),
                        category: crate::io::recovery::RecoveryCategory::Conflict,
                        description: "dismissed conflict".to_string(),
                        fields: vec![],
                        body: text.clone(),
                    },
                );
            }
            app.conflict_text = None;
        }
        return;
    }

    // Help overlay intercepts ? and Esc, plus scroll keys
    if app.show_help {
        match key.code {
            KeyCode::Char('?') | KeyCode::Esc => {
                app.show_help = false;
            }
            KeyCode::Char('j') | KeyCode::Down => {
                app.help_scroll = app.help_scroll.saturating_add(1);
            }
            KeyCode::Char('k') | KeyCode::Up => {
                app.help_scroll = app.help_scroll.saturating_sub(1);
            }
            KeyCode::Char('g') => {
                app.help_scroll = 0;
            }
            KeyCode::Char('G') => {
                app.help_scroll = usize::MAX;
            }
            _ => {}
        }
        return;
    }

    // Clear any transient status message on keypress
    app.status_message = None;
    app.status_is_error = false;

    // Track consecutive Esc presses; show quit hint after 5
    if matches!(key.code, KeyCode::Esc) {
        app.esc_streak = app.esc_streak.saturating_add(1);
        if app.esc_streak >= 5 {
            app.status_message = Some("type QQ to quit".to_string());
        }
    } else {
        app.esc_streak = 0;
    }

    // QQ quit: second Q confirms, any other key cancels
    if app.quit_pending {
        if matches!(
            (key.modifiers, key.code),
            (KeyModifiers::SHIFT, KeyCode::Char('Q'))
        ) {
            app.should_quit = true;
            return;
        } else {
            app.quit_pending = false;
            return;
        }
    }

    match (key.modifiers, key.code) {
        // Quit: Ctrl+Q
        (m, KeyCode::Char('q')) if m.contains(KeyModifiers::CONTROL) => {
            app.should_quit = true;
        }

        // Quit: Q (first press shows confirmation)
        (KeyModifiers::SHIFT, KeyCode::Char('Q')) => {
            app.quit_pending = true;
            app.status_message = Some("press Q again to quit".to_string());
        }

        // Esc: cancel range mode first; then detail nav; then clear selection
        (_, KeyCode::Esc) => {
            if app.range_anchor.is_some() {
                app.range_anchor = None;
                return;
            }
            if let View::Detail { .. } = &app.view {
                // Return from detail view but keep selection
                if let Some((parent_track, parent_task)) = app.detail_stack.pop() {
                    let return_view = app
                        .detail_state
                        .as_ref()
                        .map(|ds| ds.return_view.clone())
                        .unwrap_or(crate::tui::app::ReturnView::Track(0));
                    app.detail_state = None;
                    app.view = View::Detail {
                        track_id: parent_track.clone(),
                        task_id: parent_task.clone(),
                    };
                    let regions = if let Some(track) =
                        App::find_track_in_project(&app.project, &parent_track)
                    {
                        if let Some(task) =
                            crate::ops::task_ops::find_task_in_track(track, &parent_task)
                        {
                            App::build_detail_regions(task)
                        } else {
                            vec![DetailRegion::Title]
                        }
                    } else {
                        vec![DetailRegion::Title]
                    };
                    let region = if regions.contains(&DetailRegion::Subtasks) {
                        DetailRegion::Subtasks
                    } else {
                        regions.first().copied().unwrap_or(DetailRegion::Title)
                    };
                    app.detail_state = Some(crate::tui::app::DetailState {
                        region,
                        scroll_offset: 0,
                        regions,
                        return_view,
                        editing: false,
                        edit_buffer: String::new(),
                        edit_cursor_line: 0,
                        edit_cursor_col: 0,
                        edit_original: String::new(),
                        subtask_cursor: 0,
                        flat_subtask_ids: Vec::new(),
                        multiline_selection_anchor: None,
                        note_h_scroll: 0,
                        sticky_col: None,
                        total_lines: 0,
                        note_view_line: None,
                        note_header_line: None,
                        note_content_end: 0,
                        regions_populated: Vec::new(),
                    });
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
            } else {
                clear_selection(app);
            }
        }

        // v: toggle selection on cursor task
        (KeyModifiers::NONE, KeyCode::Char('v')) => {
            if let Some((_, task_id, _)) = app.cursor_task_id() {
                if app.selection.contains(&task_id) {
                    app.selection.remove(&task_id);
                    // Auto-exit if selection becomes empty
                    if app.selection.is_empty() {
                        app.mode = Mode::Navigate;
                    }
                } else {
                    app.selection.insert(task_id);
                }
            }
        }

        // V: toggle range select — if anchor active, finalize; otherwise begin
        (KeyModifiers::SHIFT, KeyCode::Char('V')) => {
            if app.range_anchor.is_some() {
                finalize_range_select(app);
            } else {
                begin_range_select(app);
            }
        }

        // Select none: N or Ctrl+Shift+A (must be before Ctrl+A)
        (KeyModifiers::SHIFT, KeyCode::Char('N')) => {
            clear_selection(app);
        }
        (m, KeyCode::Char('a'))
            if m.contains(KeyModifiers::CONTROL) && m.contains(KeyModifiers::SHIFT) =>
        {
            clear_selection(app);
        }
        (m, KeyCode::Char('A'))
            if m.contains(KeyModifiers::CONTROL) && m.contains(KeyModifiers::SHIFT) =>
        {
            clear_selection(app);
        }
        // Kitty protocol may report Ctrl+Shift+A as Char('A') with CONTROL only (shift implied by uppercase)
        (m, KeyCode::Char('A')) if m.contains(KeyModifiers::CONTROL) => {
            clear_selection(app);
        }

        // Select all: Ctrl+A or A (shift+a)
        (m, KeyCode::Char('a')) if m.contains(KeyModifiers::CONTROL) => {
            select_all(app);
        }
        (KeyModifiers::SHIFT, KeyCode::Char('A')) => {
            select_all(app);
        }

        // Cursor movement
        (KeyModifiers::NONE, KeyCode::Up | KeyCode::Char('k')) => {
            move_cursor(app, -1);
        }
        (KeyModifiers::NONE, KeyCode::Down | KeyCode::Char('j')) => {
            move_cursor(app, 1);
        }
        (KeyModifiers::NONE, KeyCode::Char('g')) => {
            jump_to_top(app);
        }
        (KeyModifiers::SHIFT, KeyCode::Char('G')) => {
            jump_to_bottom(app);
        }
        (_, KeyCode::Home) => {
            jump_to_top(app);
        }
        (_, KeyCode::End) => {
            jump_to_bottom(app);
        }

        // Expand/collapse
        (KeyModifiers::NONE, KeyCode::Right | KeyCode::Char('l')) => {
            expand_or_enter(app);
        }
        (KeyModifiers::NONE, KeyCode::Left | KeyCode::Char('h')) => {
            collapse_or_parent(app);
        }

        // Enter: open detail view (selection preserved)
        (KeyModifiers::NONE, KeyCode::Enter) => {
            // Temporarily switch to Navigate for the handler, then restore Select
            handle_enter(app);
            // Stay in Select mode (selection preserved across detail drill-in)
            if !matches!(app.view, View::Detail { .. }) {
                // If we didn't enter detail, stay in Select
                app.mode = Mode::Select;
            } else {
                // In detail view, mode stays Select so we return to Select on Esc
                app.mode = Mode::Select;
            }
        }

        // Bulk state changes
        (KeyModifiers::NONE, KeyCode::Char('x')) => {
            bulk_state_change(app, crate::model::TaskState::Done);
        }
        (KeyModifiers::NONE, KeyCode::Char('b')) => {
            bulk_state_change(app, crate::model::TaskState::Blocked);
        }
        (KeyModifiers::NONE, KeyCode::Char('o')) => {
            bulk_state_change(app, crate::model::TaskState::Todo);
        }
        (KeyModifiers::NONE, KeyCode::Char('~')) => {
            bulk_state_change(app, crate::model::TaskState::Parked);
        }

        // Bulk tagging
        (KeyModifiers::NONE, KeyCode::Char('t')) => {
            begin_bulk_tag_edit(app);
        }

        // Bulk dependency edit
        (KeyModifiers::NONE, KeyCode::Char('d')) => {
            begin_bulk_dep_edit(app);
        }

        // Bulk move within track
        (KeyModifiers::NONE, KeyCode::Char('m')) => {
            begin_bulk_move(app);
        }

        // Bulk move to track
        (KeyModifiers::SHIFT, KeyCode::Char('M')) => {
            begin_bulk_cross_track_move(app);
        }

        // Jump to task by ID (preserves selection)
        (KeyModifiers::SHIFT, KeyCode::Char('J')) => {
            begin_jump_to(app);
        }

        // Help overlay
        (KeyModifiers::NONE, KeyCode::Char('?')) => {
            app.show_help = true;
            app.help_scroll = 0;
        }

        // Search
        (KeyModifiers::NONE, KeyCode::Char('/')) => {
            // Allow searching while in select mode (mode will be restored)
            app.mode = Mode::Search;
            app.search_input.clear();
            app.search_draft.clear();
            app.search_history_index = None;
            app.search_wrap_message = None;
            app.search_match_count = None;
            app.search_zero_confirmed = false;
        }

        // Undo/redo
        (KeyModifiers::NONE, KeyCode::Char('u') | KeyCode::Char('z')) => {
            perform_undo(app);
        }
        (m, KeyCode::Char('z')) if m.contains(KeyModifiers::CONTROL) => {
            perform_undo(app);
        }
        (m, KeyCode::Char('z')) if m.contains(KeyModifiers::SUPER) => {
            perform_undo(app);
        }
        (m, KeyCode::Char('y')) if m.contains(KeyModifiers::CONTROL) => {
            perform_redo(app);
        }
        (m, KeyCode::Char('z') | KeyCode::Char('Z'))
            if m.contains(KeyModifiers::SUPER) && m.contains(KeyModifiers::SHIFT) =>
        {
            perform_redo(app);
        }
        (KeyModifiers::SHIFT, KeyCode::Char('Z')) => {
            perform_redo(app);
        }

        // Tab switching clears selection
        (KeyModifiers::NONE, KeyCode::Tab) => {
            clear_selection(app);
            switch_tab(app, 1);
        }
        (KeyModifiers::SHIFT, KeyCode::BackTab) => {
            clear_selection(app);
            switch_tab(app, -1);
        }
        (KeyModifiers::NONE, KeyCode::Char(c @ '1'..='9')) => {
            let idx = (c as usize) - ('1' as usize);
            if idx < app.active_track_ids.len() {
                clear_selection(app);
                app.view = View::Track(idx);
            }
        }
        (KeyModifiers::NONE, KeyCode::Char('i')) => {
            clear_selection(app);
            app.view = View::Inbox;
        }
        (KeyModifiers::NONE, KeyCode::Char('r')) => {
            clear_selection(app);
            app.view = View::Recent;
        }
        (KeyModifiers::NONE, KeyCode::Char('0') | KeyCode::Char('`')) => {
            clear_selection(app);
            app.tracks_name_col_min = 0;
            app.view = View::Tracks;
        }

        // Repeat last action: .
        (KeyModifiers::NONE, KeyCode::Char('.')) => {
            repeat_last_action(app);
        }

        // Quit: Ctrl+Q
        (m, KeyCode::Char('q')) if m.contains(KeyModifiers::CONTROL) => {
            app.should_quit = true;
        }

        _ => {}
    }
}

/// Apply an absolute state change to all selected tasks (B4).
pub(super) fn bulk_state_change(app: &mut App, target_state: crate::model::TaskState) {
    app.range_anchor = None;
    let track_id = match app.current_track_id() {
        Some(id) => id.to_string(),
        None => return,
    };
    let selected: Vec<String> = app.selection.iter().cloned().collect();
    if selected.is_empty() {
        return;
    }

    let mut ops: Vec<Operation> = Vec::new();
    let mut any_changed = false;

    for task_id in &selected {
        let track = match app.find_track_mut(&track_id) {
            Some(t) => t,
            None => continue,
        };
        let task = match task_ops::find_task_mut_in_track(track, task_id) {
            Some(t) => t,
            None => continue,
        };

        let old_state = task.state;
        if old_state == target_state {
            continue;
        }

        let old_resolved = task.metadata.iter().find_map(|m| {
            if let Metadata::Resolved(d) = m {
                Some(d.clone())
            } else {
                None
            }
        });

        // Apply the state change
        match target_state {
            crate::model::TaskState::Done => task_ops::set_done(task),
            crate::model::TaskState::Blocked => task_ops::set_blocked(task),
            crate::model::TaskState::Todo => {
                task_ops::set_state(task, crate::model::TaskState::Todo)
            }
            crate::model::TaskState::Parked => task_ops::set_parked(task),
            crate::model::TaskState::Active => {
                task_ops::set_state(task, crate::model::TaskState::Active)
            }
        }

        let new_state = task.state;
        let new_resolved = task.metadata.iter().find_map(|m| {
            if let Metadata::Resolved(d) = m {
                Some(d.clone())
            } else {
                None
            }
        });

        if old_state != new_state {
            // Cancel any pending ToDone move if un-doing Done
            if old_state == crate::model::TaskState::Done {
                app.cancel_pending_move(&track_id, task_id);
            }

            ops.push(Operation::StateChange {
                track_id: track_id.clone(),
                task_id: task_id.clone(),
                old_state,
                new_state,
                old_resolved,
                new_resolved,
            });

            // Schedule pending move if transitioning to Done
            if new_state == crate::model::TaskState::Done
                && let Some(track) = App::find_track_in_project(&app.project, &track_id)
                && task_ops::is_top_level_in_section(track, task_id, SectionKind::Backlog)
            {
                app.pending_moves.push(PendingMove {
                    kind: PendingMoveKind::ToDone,
                    track_id: track_id.clone(),
                    task_id: task_id.clone(),
                    deadline: std::time::Instant::now() + std::time::Duration::from_secs(5),
                });
            }

            any_changed = true;
        }
    }

    if any_changed {
        app.undo_stack.push(Operation::Bulk(ops));
        let _ = app.save_track(&track_id);

        // Record repeatable action
        app.last_action = Some(RepeatableAction::SetState(target_state));
    }
}

/// Open the inline editor for bulk tag editing (B5).
pub(super) fn begin_bulk_tag_edit(app: &mut App) {
    app.range_anchor = None;
    if app.selection.is_empty() {
        return;
    }
    app.edit_buffer = String::new();
    app.edit_cursor = 0;
    app.edit_selection_anchor = None;
    app.edit_target = Some(EditTarget::BulkTags);
    app.edit_history = Some(EditHistory::new("", 0, 0));

    // Activate tag autocomplete
    let candidates = app.collect_all_tags();
    if !candidates.is_empty() {
        let mut ac = AutocompleteState::new(AutocompleteKind::Tag, candidates);
        ac.filter("");
        app.autocomplete = Some(ac);
    }

    app.mode = Mode::Edit;
}

/// Confirm bulk tag edit: parse +tag/-tag tokens and apply to all selected tasks (B5).
pub(super) fn confirm_bulk_tag_edit(app: &mut App) {
    let buffer = app.edit_buffer.clone();
    let track_id = match app.current_track_id() {
        Some(id) => id.to_string(),
        None => return,
    };

    // Parse tokens: +tag adds, -tag removes, bare = add
    let (adds, removes) = parse_bulk_tokens(&buffer);
    if adds.is_empty() && removes.is_empty() {
        app.mode = Mode::Select;
        return;
    }

    let selected: Vec<String> = app.selection.iter().cloned().collect();
    let mut ops: Vec<Operation> = Vec::new();

    for task_id in &selected {
        let track = match App::find_track_in_project(&app.project, &track_id) {
            Some(t) => t,
            None => continue,
        };
        let task = match task_ops::find_task_in_track(track, task_id) {
            Some(t) => t,
            None => continue,
        };

        let old_tags = task
            .tags
            .iter()
            .map(|t| format!("#{}", t))
            .collect::<Vec<_>>()
            .join(" ");
        let mut new_tags = task.tags.clone();

        for tag in &adds {
            let clean = tag.strip_prefix('#').unwrap_or(tag).to_string();
            if !new_tags.contains(&clean) {
                new_tags.push(clean);
            }
        }
        for tag in &removes {
            let clean = tag.strip_prefix('#').unwrap_or(tag).to_string();
            new_tags.retain(|t| t != &clean);
        }

        let new_tags_str = new_tags
            .iter()
            .map(|t| format!("#{}", t))
            .collect::<Vec<_>>()
            .join(" ");
        if old_tags != new_tags_str {
            // Apply the change
            let track_mut = app.find_track_mut(&track_id).unwrap();
            let task_mut = task_ops::find_task_mut_in_track(track_mut, task_id).unwrap();
            task_mut.tags = new_tags;
            task_mut.mark_dirty();

            ops.push(Operation::FieldEdit {
                track_id: track_id.clone(),
                task_id: task_id.clone(),
                field: "tags".to_string(),
                old_value: old_tags,
                new_value: new_tags_str,
            });
        }
    }

    if !ops.is_empty() {
        app.undo_stack.push(Operation::Bulk(ops));
        let _ = app.save_track(&track_id);

        // Record repeatable action
        app.last_action = Some(RepeatableAction::TagEdit {
            adds: adds.clone(),
            removes: removes.clone(),
        });
    }

    app.mode = Mode::Select;
}

/// Open the inline editor for bulk dependency editing (B6).
pub(super) fn begin_bulk_dep_edit(app: &mut App) {
    app.range_anchor = None;
    if app.selection.is_empty() {
        return;
    }
    app.edit_buffer = String::new();
    app.edit_cursor = 0;
    app.edit_selection_anchor = None;
    app.edit_target = Some(EditTarget::BulkDeps);
    app.edit_history = Some(EditHistory::new("", 0, 0));

    // Activate task ID autocomplete
    let candidates = app.collect_all_task_ids();
    if !candidates.is_empty() {
        let mut ac = AutocompleteState::new(AutocompleteKind::TaskId, candidates);
        ac.filter("");
        app.autocomplete = Some(ac);
    }

    app.mode = Mode::Edit;
}

/// Confirm bulk dep edit: parse +ID/-ID tokens and apply to all selected tasks (B6).
pub(super) fn confirm_bulk_dep_edit(app: &mut App) {
    let buffer = app.edit_buffer.clone();
    let track_id = match app.current_track_id() {
        Some(id) => id.to_string(),
        None => return,
    };

    let (adds, removes) = parse_bulk_tokens(&buffer);
    if adds.is_empty() && removes.is_empty() {
        app.mode = Mode::Select;
        return;
    }

    let selected: Vec<String> = app.selection.iter().cloned().collect();
    let mut ops: Vec<Operation> = Vec::new();

    for task_id in &selected {
        let track = match App::find_track_in_project(&app.project, &track_id) {
            Some(t) => t,
            None => continue,
        };
        let task = match task_ops::find_task_in_track(track, task_id) {
            Some(t) => t,
            None => continue,
        };

        // Get current deps
        let old_deps: Vec<String> = task
            .metadata
            .iter()
            .find_map(|m| {
                if let Metadata::Dep(deps) = m {
                    Some(deps.clone())
                } else {
                    None
                }
            })
            .unwrap_or_default();
        let old_value = old_deps.join(", ");

        let mut new_deps = old_deps.clone();
        for dep in &adds {
            if !new_deps.contains(dep) {
                new_deps.push(dep.clone());
            }
        }
        for dep in &removes {
            new_deps.retain(|d| d != dep);
        }

        let new_value = new_deps.join(", ");
        if old_value != new_value {
            let track_mut = app.find_track_mut(&track_id).unwrap();
            let task_mut = task_ops::find_task_mut_in_track(track_mut, task_id).unwrap();
            task_mut.metadata.retain(|m| !matches!(m, Metadata::Dep(_)));
            if !new_deps.is_empty() {
                task_mut.metadata.push(Metadata::Dep(new_deps));
            }
            task_mut.mark_dirty();

            ops.push(Operation::FieldEdit {
                track_id: track_id.clone(),
                task_id: task_id.clone(),
                field: "deps".to_string(),
                old_value,
                new_value,
            });
        }
    }

    if !ops.is_empty() {
        app.undo_stack.push(Operation::Bulk(ops));
        let _ = app.save_track(&track_id);

        // Record repeatable action
        app.last_action = Some(RepeatableAction::DepEdit {
            adds: adds.clone(),
            removes: removes.clone(),
        });
    }

    app.mode = Mode::Select;
}

/// Parse a multi-token bulk edit string: "+foo -bar baz" → adds: [foo, baz], removes: [bar]
pub(super) fn parse_bulk_tokens(input: &str) -> (Vec<String>, Vec<String>) {
    let mut adds = Vec::new();
    let mut removes = Vec::new();
    for token in input.split_whitespace() {
        if let Some(tag) = token.strip_prefix('-') {
            let clean = tag.strip_prefix('#').unwrap_or(tag);
            if !clean.is_empty() {
                removes.push(clean.to_string());
            }
        } else if let Some(tag) = token.strip_prefix('+') {
            let clean = tag.strip_prefix('#').unwrap_or(tag);
            if !clean.is_empty() {
                adds.push(clean.to_string());
            }
        } else {
            let clean = token.strip_prefix('#').unwrap_or(token);
            if !clean.is_empty() {
                adds.push(clean.to_string());
            }
        }
    }
    (adds, removes)
}

/// Enter move mode for bulk-selected tasks (B7).
pub(super) fn begin_bulk_move(app: &mut App) {
    app.range_anchor = None;
    let track_id = match app.current_track_id() {
        Some(id) => id.to_string(),
        None => return,
    };
    if app.selection.is_empty() {
        return;
    }

    // Snapshot selected IDs before mutable borrow
    let selected_ids: Vec<String> = app.selection.iter().cloned().collect();
    let cursor = app.track_states.get(&track_id).map_or(0, |s| s.cursor);

    let track = match app.find_track_mut(&track_id) {
        Some(t) => t,
        None => return,
    };
    let backlog = match track.section_tasks_mut(SectionKind::Backlog) {
        Some(t) => t,
        None => return,
    };

    // Collect indices of selected top-level tasks
    let mut to_remove_indices: Vec<usize> = Vec::new();
    for (i, task) in backlog.iter().enumerate() {
        if let Some(id) = &task.id
            && selected_ids.contains(id)
        {
            to_remove_indices.push(i);
        }
    }

    if to_remove_indices.is_empty() {
        return;
    }

    // Remove tasks in reverse order to preserve indices
    let mut removed_tasks: Vec<(usize, crate::model::Task)> = Vec::new();
    for &idx in to_remove_indices.iter().rev() {
        let task = backlog.remove(idx);
        removed_tasks.push((idx, task));
    }
    removed_tasks.reverse(); // Restore original order

    // Determine initial insertion position (at the cursor's current position in the reduced list)
    let insert_pos = cursor.min(backlog.len());

    app.move_state = Some(MoveState::BulkTask {
        track_id: track_id.clone(),
        removed_tasks,
        insert_pos,
    });
    app.mode = Mode::Move;
}

pub(super) fn begin_bulk_cross_track_move(app: &mut App) {
    app.range_anchor = None;
    let source_track_id = match app.current_track_id() {
        Some(id) => id.to_string(),
        None => return,
    };
    if app.selection.is_empty() {
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
        ac.filter("");
    }

    app.triage_state = Some(crate::tui::app::TriageState {
        source: TriageSource::BulkCrossTrackMove { source_track_id },
        step: crate::tui::app::TriageStep::SelectTrack,
        popup_anchor: None,
        position_cursor: 1, // default to Bottom
    });
    app.mode = Mode::Triage;
}

/// Move the bulk-move stand-in position up or down by one.
pub(super) fn move_bulk_standin(app: &mut App, direction: i32) {
    let (track_id, max_pos) = match &app.move_state {
        Some(MoveState::BulkTask { track_id, .. }) => {
            let tid = track_id.clone();
            let backlog_len = App::find_track_in_project(&app.project, &tid)
                .map(|t| t.backlog().len())
                .unwrap_or(0);
            (tid, backlog_len)
        }
        _ => return,
    };
    if let Some(MoveState::BulkTask { insert_pos, .. }) = &mut app.move_state {
        let new_pos = (*insert_pos as i32 + direction).clamp(0, max_pos as i32) as usize;
        *insert_pos = new_pos;
    }
    // Update cursor to track the stand-in position
    if let Some(MoveState::BulkTask { insert_pos, .. }) = &app.move_state {
        let pos = *insert_pos;
        if let Some(state) = app.track_states.get_mut(&track_id) {
            state.cursor = pos;
        }
    }
}

/// Move the bulk-move stand-in to the top or bottom of the backlog.
pub(super) fn move_bulk_standin_to_boundary(app: &mut App, to_top: bool) {
    let (track_id, max_pos) = match &app.move_state {
        Some(MoveState::BulkTask { track_id, .. }) => {
            let tid = track_id.clone();
            let backlog_len = App::find_track_in_project(&app.project, &tid)
                .map(|t| t.backlog().len())
                .unwrap_or(0);
            (tid, backlog_len)
        }
        _ => return,
    };
    let new_pos = if to_top { 0 } else { max_pos };
    if let Some(MoveState::BulkTask { insert_pos, .. }) = &mut app.move_state {
        *insert_pos = new_pos;
    }
    if let Some(state) = app.track_states.get_mut(&track_id) {
        state.cursor = new_pos;
    }
}
