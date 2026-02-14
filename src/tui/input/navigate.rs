use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::tui::app::{
    App, AutocompleteKind, AutocompleteState, DetailRegion, EditHistory, EditTarget, FlatItem,
    Mode, StateFilter, View,
};

use super::*;

pub(super) fn handle_navigate(app: &mut App, key: KeyEvent) {
    // Clear recovery notification on any keypress (after 3s minimum display)
    if app.recovery_message.is_some()
        && let Some(at) = app.recovery_message_at
        && at.elapsed() >= std::time::Duration::from_secs(3)
    {
        app.recovery_message = None;
        app.recovery_message_at = None;
    }

    // Conflict popup intercepts Esc
    if app.conflict_text.is_some() {
        if matches!(key.code, KeyCode::Esc) {
            // Log conflict text to recovery log before clearing
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

    // Project picker intercepts all keys
    if app.project_picker.is_some() {
        handle_project_picker_key(app, key);
        return;
    }

    // Tag color popup intercepts all keys
    if app.tag_color_popup.is_some() {
        handle_tag_color_popup_key(app, key);
        return;
    }

    // Dep popup intercepts navigation keys
    if app.dep_popup.is_some() {
        handle_dep_popup_key(app, key);
        return;
    }

    // Prefix rename confirmation popup
    if let Some(ref pr) = app.prefix_rename
        && pr.confirming
    {
        match key.code {
            KeyCode::Enter => {
                execute_prefix_rename(app);
            }
            KeyCode::Esc => {
                // Go back to the prefix editor (not all the way back to Tracks view)
                if let Some(ref mut pr) = app.prefix_rename {
                    let old_prefix = pr.old_prefix.clone();
                    let track_id = pr.track_id.clone();
                    let new_prefix = pr.new_prefix.clone();
                    pr.confirming = false;

                    // Re-enter edit mode with the new_prefix in the buffer
                    app.edit_buffer = new_prefix.clone();
                    app.edit_cursor = new_prefix.len();
                    app.edit_target = Some(EditTarget::ExistingPrefix {
                        track_id,
                        original_prefix: old_prefix,
                    });
                    app.edit_history = Some(EditHistory::new(&new_prefix, new_prefix.len(), 0));
                    app.edit_selection_anchor = None;
                    app.mode = Mode::Edit;
                }
            }
            _ => {}
        }
        return;
    }

    // Filter prefix key: 'f' was pressed, now handle second key
    if app.filter_pending {
        app.filter_pending = false;
        handle_filter_key(app, key);
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

        // Toggle key debug overlay: Ctrl+D
        (m, KeyCode::Char('d')) if m.contains(KeyModifiers::CONTROL) => {
            app.key_debug = !app.key_debug;
            if !app.key_debug {
                app.last_key_event = None;
            }
        }

        // Quit: Q (first press shows confirmation)
        (KeyModifiers::SHIFT, KeyCode::Char('Q')) => {
            app.quit_pending = true;
            app.status_message = Some("press Q again to quit".to_string());
        }

        // Esc: pop detail stack, close detail view, or clear search
        (_, KeyCode::Esc) => {
            if let View::Detail { .. } = &app.view {
                if let Some((parent_track, parent_task)) = app.detail_stack.pop() {
                    // Return to parent detail view, focusing on Subtasks region
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
                    // Rebuild regions and focus on Subtasks
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
                    // Stack empty — return to origin view
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
            } else if app.last_search.is_some() {
                app.last_search = None;
                app.search_match_idx = 0;
                app.search_wrap_message = None;
                app.search_match_count = None;
                app.search_zero_confirmed = false;
            }
        }

        // Backspace: clear active search in Detail view (where Esc navigates back)
        (_, KeyCode::Backspace)
            if app.last_search.is_some() && matches!(app.view, View::Detail { .. }) =>
        {
            app.last_search = None;
            app.search_match_idx = 0;
            app.search_wrap_message = None;
            app.search_match_count = None;
            app.search_zero_confirmed = false;
        }

        // Help overlay
        (KeyModifiers::NONE, KeyCode::Char('?')) => {
            app.show_help = true;
            app.help_scroll = 0;
        }

        // Search: /
        (KeyModifiers::NONE, KeyCode::Char('/')) => {
            app.mode = Mode::Search;
            app.search_input.clear();
            app.search_draft.clear();
            app.search_history_index = None;
            app.search_wrap_message = None;
            app.search_match_count = None;
            app.search_zero_confirmed = false;
        }

        // n: search next when search active, otherwise note edit (cursor at end) in detail/inbox view
        (KeyModifiers::NONE, KeyCode::Char('n')) => {
            if app.last_search.is_some() {
                search_next(app, 1);
            } else if matches!(app.view, View::Detail { .. }) {
                detail_jump_to_region_and_edit(app, DetailRegion::Note, true);
            } else if matches!(app.view, View::Inbox) {
                inbox_edit_note(app, true);
            }
        }
        // N: search prev when search active, otherwise note edit (cursor at start) in detail/inbox view
        (KeyModifiers::SHIFT, KeyCode::Char('N')) => {
            if app.last_search.is_some() {
                search_next(app, -1);
            } else if matches!(app.view, View::Detail { .. }) {
                detail_jump_to_region_and_edit(app, DetailRegion::Note, false);
            } else if matches!(app.view, View::Inbox) {
                inbox_edit_note(app, false);
            }
        }

        // Tab switching: 1-9 for active tracks
        (KeyModifiers::NONE, KeyCode::Char(c @ '1'..='9')) => {
            let idx = (c as usize) - ('1' as usize);
            if idx < app.active_track_ids.len() {
                app.close_detail_fully();
                app.view = View::Track(idx);
            }
        }

        // Tab/Shift+Tab: next/prev editable region (detail view) or next/prev tab
        (KeyModifiers::NONE, KeyCode::Tab) => {
            if matches!(app.view, View::Detail { .. }) {
                detail_jump_editable(app, 1);
            } else {
                switch_tab(app, 1);
            }
        }
        (KeyModifiers::SHIFT, KeyCode::BackTab) => {
            if matches!(app.view, View::Detail { .. }) {
                detail_jump_editable(app, -1);
            } else {
                switch_tab(app, -1);
            }
        }

        // View switching
        (KeyModifiers::NONE, KeyCode::Char('i')) => {
            app.close_detail_fully();
            app.view = View::Inbox;
        }
        (KeyModifiers::NONE, KeyCode::Char('r')) => {
            app.close_detail_fully();
            app.view = View::Recent;
        }
        (KeyModifiers::NONE, KeyCode::Char('0') | KeyCode::Char('`')) => {
            app.close_detail_fully();
            app.tracks_name_col_min = 0;
            app.view = View::Tracks;
        }

        // Cursor movement: up/down
        (KeyModifiers::NONE, KeyCode::Up | KeyCode::Char('k')) => {
            move_cursor(app, -1);
        }
        (KeyModifiers::NONE, KeyCode::Down | KeyCode::Char('j')) => {
            move_cursor(app, 1);
        }

        // Paragraph movement: Alt+Up/Down — jump between top-level tasks
        (m, KeyCode::Up) if m.contains(KeyModifiers::ALT) => {
            move_paragraph(app, -1);
        }
        (m, KeyCode::Down) if m.contains(KeyModifiers::ALT) => {
            move_paragraph(app, 1);
        }

        // Jump to top: g, Cmd+Up, or Home
        (KeyModifiers::NONE, KeyCode::Char('g')) => {
            jump_to_top(app);
        }
        (m, KeyCode::Up) if m.contains(KeyModifiers::SUPER) => {
            jump_to_top(app);
        }
        (_, KeyCode::Home) => {
            jump_to_top(app);
        }

        // Jump to bottom: G, Cmd+Down, or End
        (KeyModifiers::SHIFT, KeyCode::Char('G')) => {
            jump_to_bottom(app);
        }
        (m, KeyCode::Down) if m.contains(KeyModifiers::SUPER) => {
            jump_to_bottom(app);
        }
        (_, KeyCode::End) => {
            jump_to_bottom(app);
        }

        // Enter: open detail view (track/recent view), triage (inbox), or edit region (detail view)
        (KeyModifiers::NONE, KeyCode::Enter) => {
            if matches!(app.view, View::Inbox) {
                inbox_begin_triage(app);
            } else if matches!(app.view, View::Recent) {
                open_recent_detail(app);
            } else {
                handle_enter(app);
            }
        }

        // Expand/collapse (track view) or recent view
        (KeyModifiers::NONE, KeyCode::Right | KeyCode::Char('l')) => {
            if matches!(app.view, View::Recent) {
                expand_recent(app);
            } else {
                expand_or_enter(app);
            }
        }
        (KeyModifiers::NONE, KeyCode::Left | KeyCode::Char('h')) => {
            if matches!(app.view, View::Recent) {
                collapse_recent(app);
            } else {
                collapse_or_parent(app);
            }
        }

        // Task state changes (track view) or reopen (recent view)
        (KeyModifiers::NONE, KeyCode::Char(' ')) => {
            if matches!(app.view, View::Recent) {
                reopen_recent_task(app);
            } else {
                task_state_action(app, StateAction::Cycle);
            }
        }
        (KeyModifiers::NONE, KeyCode::Char('x')) => {
            if matches!(app.view, View::Inbox) {
                inbox_delete_item(app);
            } else {
                task_state_action(app, StateAction::Done);
            }
        }
        (KeyModifiers::NONE, KeyCode::Char('b')) => {
            task_state_action(app, StateAction::ToggleBlocked);
        }
        (KeyModifiers::NONE, KeyCode::Char('~')) => {
            task_state_action(app, StateAction::ToggleParked);
        }

        // Add task (track view), add inbox item (inbox view), or add track (tracks view)
        (KeyModifiers::NONE, KeyCode::Char('a')) => {
            if matches!(app.view, View::Inbox) {
                inbox_add_item(app);
            } else if matches!(app.view, View::Tracks) {
                tracks_add_track(app);
            } else {
                add_task_action(app, AddPosition::Bottom);
            }
        }
        (KeyModifiers::NONE, KeyCode::Char('o')) => {
            task_state_action(app, StateAction::SetTodo);
        }
        (KeyModifiers::NONE, KeyCode::Char('-')) => {
            if matches!(app.view, View::Inbox) {
                inbox_insert_after(app);
            } else if matches!(app.view, View::Tracks) {
                tracks_insert_after(app);
            } else {
                add_task_action(app, AddPosition::AfterCursor);
            }
        }
        (KeyModifiers::NONE, KeyCode::Char('p')) => {
            if matches!(app.view, View::Inbox) {
                inbox_prepend_item(app);
            } else if matches!(app.view, View::Tracks) {
                tracks_prepend(app);
            } else {
                add_task_action(app, AddPosition::Top);
            }
        }
        (KeyModifiers::SHIFT, KeyCode::Char('A')) => {
            add_subtask_action(app);
        }
        (KeyModifiers::NONE, KeyCode::Char('=')) => {
            if matches!(app.view, View::Inbox) {
                inbox_add_item(app);
            } else if matches!(app.view, View::Tracks) {
                tracks_add_track(app);
            } else {
                append_sibling_action(app);
            }
        }

        // Inline title edit (track view), edit (inbox view), edit track name (tracks view), or enter edit mode (detail view)
        (KeyModifiers::NONE, KeyCode::Char('e')) => {
            if matches!(app.view, View::Detail { .. }) {
                detail_enter_edit(app, false);
            } else if matches!(app.view, View::Inbox) {
                inbox_edit_title(app);
            } else if matches!(app.view, View::Tracks) {
                tracks_edit_name(app);
            } else {
                enter_title_edit(app);
            }
        }

        // Tag edit: t — detail view jump to tags region, inbox tag edit, or inline tag edit in track view
        (KeyModifiers::NONE, KeyCode::Char('t')) => {
            if matches!(app.view, View::Detail { .. }) {
                detail_jump_to_region_and_edit(app, DetailRegion::Tags, false);
            } else if matches!(app.view, View::Inbox) {
                inbox_edit_tags(app);
            } else {
                enter_tag_edit(app);
            }
        }
        (KeyModifiers::NONE, KeyCode::Char('@')) => {
            if matches!(app.view, View::Detail { .. }) {
                detail_jump_to_region_and_edit(app, DetailRegion::Refs, false);
            }
        }
        (KeyModifiers::NONE, KeyCode::Char('d')) => {
            if matches!(app.view, View::Detail { .. }) {
                detail_jump_to_region_and_edit(app, DetailRegion::Deps, false);
            }
        }

        // Shelve toggle (tracks view only)
        (KeyModifiers::NONE, KeyCode::Char('s')) => {
            if matches!(app.view, View::Tracks) {
                tracks_toggle_shelve(app);
            }
        }

        // Dep popup (track/detail view)
        (KeyModifiers::SHIFT, KeyCode::Char('D')) => {
            if matches!(app.view, View::Track(_)) {
                open_dep_popup_from_track_view(app);
            } else if matches!(app.view, View::Detail { .. }) {
                open_dep_popup_from_detail_view(app);
            }
        }

        // Toggle cc tag on task
        (KeyModifiers::NONE, KeyCode::Char('c')) => {
            toggle_cc_tag(app);
        }

        // Set cc-focus to current track
        (KeyModifiers::SHIFT, KeyCode::Char('C')) => {
            set_cc_focus_current(app);
        }

        // Toggle note wrap (detail view only)
        (KeyModifiers::NONE, KeyCode::Char('w')) if matches!(app.view, View::Detail { .. }) => {
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

        // Move mode (track, tracks, or inbox view)
        (KeyModifiers::NONE, KeyCode::Char('m')) => {
            if matches!(app.view, View::Inbox) {
                inbox_enter_move_mode(app);
            } else {
                enter_move_mode(app);
            }
        }

        // Cross-track move (track view or detail view)
        (KeyModifiers::SHIFT, KeyCode::Char('M')) => {
            begin_cross_track_move(app);
        }

        // Redo: Z, Ctrl+Y, Ctrl+Shift+Z, or Super+Shift+Z (must be checked BEFORE undo)
        (m, KeyCode::Char('y')) if m.contains(KeyModifiers::CONTROL) => {
            perform_redo(app);
        }
        (m, KeyCode::Char('z') | KeyCode::Char('Z'))
            if m.contains(KeyModifiers::CONTROL) && m.contains(KeyModifiers::SHIFT) =>
        {
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

        // Undo: u, z, Ctrl+Z, or Super+Z
        (KeyModifiers::NONE, KeyCode::Char('u') | KeyCode::Char('z')) => {
            perform_undo(app);
        }
        (m, KeyCode::Char('z')) if m.contains(KeyModifiers::CONTROL) => {
            perform_undo(app);
        }
        (m, KeyCode::Char('z')) if m.contains(KeyModifiers::SUPER) => {
            perform_undo(app);
        }

        // Filter prefix key (track view only)
        (KeyModifiers::NONE, KeyCode::Char('f')) => {
            if matches!(app.view, View::Track(_)) {
                app.filter_pending = true;
            }
        }

        // SELECT mode: v enters select and toggles current task
        (KeyModifiers::NONE, KeyCode::Char('v')) => {
            if matches!(app.view, View::Track(_)) {
                enter_select_mode(app);
            }
        }

        // Range select: V begins range selection mode
        (KeyModifiers::SHIFT, KeyCode::Char('V')) => {
            if matches!(app.view, View::Track(_)) {
                begin_range_select(app);
            }
        }

        // Select all: Ctrl+A
        (m, KeyCode::Char('a')) if m.contains(KeyModifiers::CONTROL) => {
            if matches!(app.view, View::Track(_)) {
                select_all(app);
            }
        }

        // Jump to task by ID: J (available from any view)
        (KeyModifiers::SHIFT, KeyCode::Char('J')) => {
            begin_jump_to(app);
        }

        // Repeat last action: .
        (KeyModifiers::NONE, KeyCode::Char('.')) => {
            repeat_last_action(app);
        }

        // Tag color editor: T (available from any view)
        (KeyModifiers::SHIFT, KeyCode::Char('T')) => {
            app.open_tag_color_popup();
        }

        // Project picker: P (Shift+P, available from any view)
        (KeyModifiers::SHIFT, KeyCode::Char('P')) => {
            open_project_picker(app);
        }

        // Command palette: > (Shift+. reports as NONE or SHIFT depending on terminal)
        (_, KeyCode::Char('>')) => {
            open_command_palette(app);
        }

        _ => {}
    }
}

/// Handle the second key after 'f' prefix for filtering
pub(super) fn handle_filter_key(app: &mut App, key: KeyEvent) {
    // Only applies to track view
    if !matches!(app.view, View::Track(_)) {
        return;
    }

    // Capture current task ID before changing filter so we can try to stay on it
    let prev_task_id = get_cursor_task_id(app);

    match key.code {
        KeyCode::Char('a') => {
            app.filter_state.state_filter = Some(StateFilter::Active);
            reset_cursor_for_filter(app, prev_task_id.as_deref());
        }
        KeyCode::Char('o') => {
            app.filter_state.state_filter = Some(StateFilter::Todo);
            reset_cursor_for_filter(app, prev_task_id.as_deref());
        }
        KeyCode::Char('b') => {
            app.filter_state.state_filter = Some(StateFilter::Blocked);
            reset_cursor_for_filter(app, prev_task_id.as_deref());
        }
        KeyCode::Char('p') => {
            app.filter_state.state_filter = Some(StateFilter::Parked);
            reset_cursor_for_filter(app, prev_task_id.as_deref());
        }
        KeyCode::Char('r') => {
            app.filter_state.state_filter = Some(StateFilter::Ready);
            reset_cursor_for_filter(app, prev_task_id.as_deref());
        }
        KeyCode::Char('t') => {
            // Open tag autocomplete for filter tag selection
            begin_filter_tag_select(app);
        }
        KeyCode::Char(' ') => {
            // Clear state filter only, keep tag filter
            app.filter_state.clear_state();
            reset_cursor_for_filter(app, prev_task_id.as_deref());
        }
        KeyCode::Char('f') => {
            // Clear all filters
            app.filter_state.clear_all();
            reset_cursor_for_filter(app, prev_task_id.as_deref());
        }
        _ => {
            // Unknown second key — ignore silently
        }
    }
}

/// Begin tag filter selection using tag autocomplete
pub(super) fn begin_filter_tag_select(app: &mut App) {
    let candidates = app.collect_all_tags();
    if candidates.is_empty() {
        return;
    }
    // Enter Edit mode with a special edit target for filter tag selection
    app.mode = Mode::Edit;
    app.edit_buffer = String::new();
    app.edit_cursor = 0;
    app.edit_selection_anchor = None;
    app.edit_target = Some(EditTarget::FilterTag);
    app.edit_history = Some(EditHistory::new("", 0, 0));

    let mut ac = AutocompleteState::new(AutocompleteKind::Tag, candidates);
    ac.filter("");
    app.autocomplete = Some(ac);
}

/// Begin jump-to-task prompt: enter Edit mode with task ID autocomplete
pub(super) fn begin_jump_to(app: &mut App) {
    let candidates = app.collect_active_track_task_ids();
    if candidates.is_empty() {
        app.status_message = Some("no tasks to jump to".to_string());
        return;
    }
    app.mode = Mode::Edit;
    app.edit_buffer = String::new();
    app.edit_cursor = 0;
    app.edit_selection_anchor = None;
    app.edit_target = Some(EditTarget::JumpTo);
    app.edit_history = Some(EditHistory::new("", 0, 0));

    let mut ac = AutocompleteState::new(AutocompleteKind::JumpTaskId, candidates);
    ac.filter("");
    app.autocomplete = Some(ac);
}

// ---------------------------------------------------------------------------
// SELECT mode (bulk operations)

/// Expand current node or move to first child (track view)
pub(super) fn expand_or_enter(app: &mut App) {
    if let View::Track(idx) = &app.view {
        let track_id = match app.active_track_ids.get(*idx) {
            Some(id) => id.clone(),
            None => return,
        };
        let flat_items = app.build_flat_items(&track_id);
        let cursor = app.track_states.get(&track_id).map_or(0, |s| s.cursor);

        if cursor >= flat_items.len() {
            return;
        }

        if let FlatItem::Task {
            has_children,
            is_expanded,
            section,
            path,
            ..
        } = &flat_items[cursor]
        {
            if *has_children && !is_expanded {
                // Expand this node
                let track = match app.current_track() {
                    Some(t) => t,
                    None => return,
                };
                if let Some(task) = resolve_task_from_track(track, *section, path) {
                    let key = crate::tui::app::task_expand_key(task, *section, path);
                    let state = app.get_track_state(&track_id);
                    state.expanded.insert(key);
                }
            } else if *has_children && *is_expanded && cursor + 1 < flat_items.len() {
                // Already expanded: move to first child
                let state = app.get_track_state(&track_id);
                state.cursor = cursor + 1;
            }
        }
    }
}

/// Collapse current node or move to parent (track view)
pub(super) fn collapse_or_parent(app: &mut App) {
    if let View::Track(idx) = &app.view {
        let track_id = match app.active_track_ids.get(*idx) {
            Some(id) => id.clone(),
            None => return,
        };
        let flat_items = app.build_flat_items(&track_id);
        let cursor = app.track_states.get(&track_id).map_or(0, |s| s.cursor);

        if cursor >= flat_items.len() {
            return;
        }

        if let FlatItem::Task {
            has_children: _,
            is_expanded,
            section,
            path,
            depth,
            ..
        } = &flat_items[cursor]
        {
            if *is_expanded {
                // Collapse this node
                let track = match app.current_track() {
                    Some(t) => t,
                    None => return,
                };
                if let Some(task) = resolve_task_from_track(track, *section, path) {
                    let key = crate::tui::app::task_expand_key(task, *section, path);
                    let state = app.get_track_state(&track_id);
                    state.expanded.remove(&key);
                }
            } else if *depth > 0 {
                // Move to parent: find the previous item at depth - 1
                let parent_depth = depth - 1;
                for i in (0..cursor).rev() {
                    if let FlatItem::Task { depth: d, .. } = &flat_items[i]
                        && *d == parent_depth
                    {
                        app.get_track_state(&track_id).cursor = i;
                        break;
                    }
                }
            }
        }
    }
}

pub(super) fn resolve_task_from_track<'a>(
    track: &'a crate::model::Track,
    section: crate::model::SectionKind,
    path: &[usize],
) -> Option<&'a crate::model::Task> {
    let tasks = track.section_tasks(section);
    if path.is_empty() {
        return None;
    }
    let mut current = tasks.get(path[0])?;
    for &idx in &path[1..] {
        current = current.subtasks.get(idx)?;
    }
    Some(current)
}

/// Count total tracks (for cursor navigation in tracks view)
pub(super) fn count_tracks(app: &App) -> usize {
    app.project.config.tracks.len()
}

/// Count total done tasks across all tracks
pub(super) fn count_recent_tasks(app: &App) -> usize {
    app.project
        .tracks
        .iter()
        .map(|(_, track)| track.section_tasks(crate::model::SectionKind::Done).len())
        .sum()
}
