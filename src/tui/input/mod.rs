use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use regex::Regex;

use crate::model::SectionKind;
use crate::ops::search::{search_inbox, search_tasks};

use super::app::{App, FlatItem, Mode, View};

/// Handle a key event in the current mode
pub fn handle_key(app: &mut App, key: KeyEvent) {
    match app.mode {
        Mode::Navigate => handle_navigate(app, key),
        Mode::Search => handle_search(app, key),
    }
}

fn handle_navigate(app: &mut App, key: KeyEvent) {
    // Help overlay intercepts ? and Esc
    if app.show_help {
        match key.code {
            KeyCode::Char('?') | KeyCode::Esc => {
                app.show_help = false;
            }
            _ => {}
        }
        return;
    }

    // Clear any transient status message on keypress
    app.status_message = None;

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

        // Esc: clear search first, then normal behavior
        (_, KeyCode::Esc) => {
            if app.last_search.is_some() {
                app.last_search = None;
                app.search_match_idx = 0;
                app.search_wrap_message = None;
                app.search_match_count = None;
                app.search_zero_confirmed = false;
            }
        }

        // Help overlay
        (KeyModifiers::NONE, KeyCode::Char('?')) => {
            app.show_help = true;
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

        // Search: n/N for next/prev match
        (KeyModifiers::NONE, KeyCode::Char('n')) => {
            if app.last_search.is_some() {
                search_next(app, 1);
            }
        }
        (KeyModifiers::SHIFT, KeyCode::Char('N')) => {
            if app.last_search.is_some() {
                search_next(app, -1);
            }
        }

        // Tab switching: 1-9 for active tracks
        (KeyModifiers::NONE, KeyCode::Char(c @ '1'..='9')) => {
            let idx = (c as usize) - ('1' as usize);
            if idx < app.active_track_ids.len() {
                app.view = View::Track(idx);
            }
        }

        // Tab/Shift+Tab: next/prev tab
        (KeyModifiers::NONE, KeyCode::Tab) => {
            switch_tab(app, 1);
        }
        (KeyModifiers::SHIFT, KeyCode::BackTab) => {
            switch_tab(app, -1);
        }

        // View switching
        (KeyModifiers::NONE, KeyCode::Char('i')) => {
            app.view = View::Inbox;
        }
        (KeyModifiers::NONE, KeyCode::Char('r')) => {
            app.view = View::Recent;
        }
        (KeyModifiers::NONE, KeyCode::Char('0') | KeyCode::Char('`')) => {
            app.view = View::Tracks;
        }

        // Cursor movement: up/down
        (KeyModifiers::NONE, KeyCode::Up | KeyCode::Char('k')) => {
            move_cursor(app, -1);
        }
        (KeyModifiers::NONE, KeyCode::Down | KeyCode::Char('j')) => {
            move_cursor(app, 1);
        }

        // Jump to top: g or Cmd+Up
        (KeyModifiers::NONE, KeyCode::Char('g')) => {
            jump_to_top(app);
        }
        (m, KeyCode::Up) if m.contains(KeyModifiers::SUPER) => {
            jump_to_top(app);
        }

        // Jump to bottom: G or Cmd+Down
        (KeyModifiers::SHIFT, KeyCode::Char('G')) => {
            jump_to_bottom(app);
        }
        (m, KeyCode::Down) if m.contains(KeyModifiers::SUPER) => {
            jump_to_bottom(app);
        }

        // Expand/collapse (track view only)
        (KeyModifiers::NONE, KeyCode::Right | KeyCode::Char('l')) => {
            expand_or_enter(app);
        }
        (KeyModifiers::NONE, KeyCode::Left | KeyCode::Char('h')) => {
            collapse_or_parent(app);
        }

        _ => {}
    }
}

fn handle_search(app: &mut App, key: KeyEvent) {
    match (key.modifiers, key.code) {
        // Cancel search
        (_, KeyCode::Esc) => {
            app.mode = Mode::Navigate;
            app.search_input.clear();
            app.search_history_index = None;
            // Recompute match count for last_search (mode is now Navigate)
            if let Some(re) = app.active_search_re() {
                app.search_match_count = Some(count_matches_for_pattern(app, &re));
            } else {
                app.search_match_count = None;
            }
        }

        // Execute search
        (_, KeyCode::Enter) => {
            if !app.search_input.is_empty() {
                let query = app.search_input.clone();
                // Add to history (dedup: remove previous occurrence, then push to front)
                app.search_history.retain(|s| s != &query);
                app.search_history.insert(0, query);
                app.search_history.truncate(200);

                app.last_search = Some(app.search_input.clone());
                execute_search_dir(app, 0);
                app.search_zero_confirmed = app.search_match_count == Some(0);
            }
            app.mode = Mode::Navigate;
            app.search_input.clear();
            app.search_history_index = None;
            app.search_wrap_message = None;
        }

        // History navigation: Up = older
        (_, KeyCode::Up) => {
            if !app.search_history.is_empty() {
                match app.search_history_index {
                    None => {
                        app.search_draft = app.search_input.clone();
                        app.search_history_index = Some(0);
                        app.search_input = app.search_history[0].clone();
                    }
                    Some(idx) => {
                        let next = idx + 1;
                        if next < app.search_history.len() {
                            app.search_history_index = Some(next);
                            app.search_input = app.search_history[next].clone();
                        }
                    }
                }
                update_match_count(app);
            }
        }

        // History navigation: Down = newer
        (_, KeyCode::Down) => {
            let changed = match app.search_history_index {
                None => false,
                Some(0) => {
                    app.search_history_index = None;
                    app.search_input = app.search_draft.clone();
                    true
                }
                Some(idx) => {
                    let prev = idx - 1;
                    app.search_history_index = Some(prev);
                    app.search_input = app.search_history[prev].clone();
                    true
                }
            };
            if changed {
                update_match_count(app);
            }
        }

        // Backspace
        (_, KeyCode::Backspace) => {
            app.search_input.pop();
            if app.search_history_index.is_some() {
                app.search_history_index = None;
                app.search_draft.clear();
            }
            update_match_count(app);
        }

        // Type character
        (KeyModifiers::NONE | KeyModifiers::SHIFT, KeyCode::Char(c)) => {
            app.search_input.push(c);
            if app.search_history_index.is_some() {
                app.search_history_index = None;
                app.search_draft.clear();
            }
            update_match_count(app);
        }

        _ => {}
    }
}

/// Move cursor by delta in the current view
fn move_cursor(app: &mut App, delta: i32) {
    match &app.view {
        View::Track(idx) => {
            let track_id = match app.active_track_ids.get(*idx) {
                Some(id) => id.clone(),
                None => return,
            };
            let flat_items = app.build_flat_items(&track_id);
            let item_count = flat_items.len();
            if item_count == 0 {
                return;
            }

            let state = app.get_track_state(&track_id);
            let mut new_cursor = state.cursor as i32 + delta;
            new_cursor = new_cursor.clamp(0, item_count as i32 - 1);

            // Skip ParkedSeparator
            let new_cursor = new_cursor as usize;
            let new_cursor = skip_separator(&flat_items, new_cursor, delta);

            state.cursor = new_cursor;
        }
        View::Tracks => {
            let total = count_tracks(app);
            if total == 0 {
                return;
            }
            let mut new_cursor = app.tracks_cursor as i32 + delta;
            new_cursor = new_cursor.clamp(0, total as i32 - 1);
            app.tracks_cursor = new_cursor as usize;
        }
        View::Inbox => {
            let count = app.inbox_count();
            if count == 0 {
                return;
            }
            let mut new_cursor = app.inbox_cursor as i32 + delta;
            new_cursor = new_cursor.clamp(0, count as i32 - 1);
            app.inbox_cursor = new_cursor as usize;
        }
        View::Recent => {
            let count = count_recent_tasks(app);
            if count == 0 {
                return;
            }
            let mut new_cursor = app.recent_cursor as i32 + delta;
            new_cursor = new_cursor.clamp(0, count as i32 - 1);
            app.recent_cursor = new_cursor as usize;
        }
    }
}

/// Skip over ParkedSeparator items when navigating
fn skip_separator(items: &[FlatItem], cursor: usize, direction: i32) -> usize {
    if cursor >= items.len() {
        return cursor;
    }
    if matches!(items[cursor], FlatItem::ParkedSeparator) {
        let next = (cursor as i32 + direction).clamp(0, items.len() as i32 - 1) as usize;
        if next != cursor && !matches!(items[next], FlatItem::ParkedSeparator) {
            return next;
        }
        // If stuck, try the other direction
        let prev = (cursor as i32 - direction).clamp(0, items.len() as i32 - 1) as usize;
        if !matches!(items[prev], FlatItem::ParkedSeparator) {
            return prev;
        }
    }
    cursor
}

fn jump_to_top(app: &mut App) {
    match &app.view {
        View::Track(idx) => {
            let track_id = match app.active_track_ids.get(*idx) {
                Some(id) => id.clone(),
                None => return,
            };
            let state = app.get_track_state(&track_id);
            state.cursor = 0;
            state.scroll_offset = 0;
        }
        View::Tracks => {
            app.tracks_cursor = 0;
        }
        View::Inbox => {
            app.inbox_cursor = 0;
            app.inbox_scroll = 0;
        }
        View::Recent => {
            app.recent_cursor = 0;
            app.recent_scroll = 0;
        }
    }
}

fn jump_to_bottom(app: &mut App) {
    match &app.view {
        View::Track(idx) => {
            let track_id = match app.active_track_ids.get(*idx) {
                Some(id) => id.clone(),
                None => return,
            };
            let flat_items = app.build_flat_items(&track_id);
            let count = flat_items.len();
            if count == 0 {
                return;
            }
            let mut target = count - 1;
            // Skip separator at end
            target = skip_separator(&flat_items, target, -1);
            let state = app.get_track_state(&track_id);
            state.cursor = target;
        }
        View::Tracks => {
            let total = count_tracks(app);
            if total > 0 {
                app.tracks_cursor = total - 1;
            }
        }
        View::Inbox => {
            let count = app.inbox_count();
            if count > 0 {
                app.inbox_cursor = count - 1;
            }
        }
        View::Recent => {
            let count = count_recent_tasks(app);
            if count > 0 {
                app.recent_cursor = count - 1;
            }
        }
    }
}

/// Expand current node or move to first child (track view)
fn expand_or_enter(app: &mut App) {
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
fn collapse_or_parent(app: &mut App) {
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

fn resolve_task_from_track<'a>(
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
fn count_tracks(app: &App) -> usize {
    app.project.config.tracks.len()
}

/// Count total done tasks across all tracks
fn count_recent_tasks(app: &App) -> usize {
    app.project
        .tracks
        .iter()
        .map(|(_, track)| track.section_tasks(crate::model::SectionKind::Done).len())
        .sum()
}

/// Advance search by `direction` (+1 = next, -1 = prev) with proper wrapping.
fn search_next(app: &mut App, direction: i32) {
    app.search_wrap_message = None;
    execute_search_dir(app, direction);
}

/// Execute search: find matches in the current view and jump to the match.
/// `direction` is +1 (next) or -1 (prev) or 0 (first from cursor).
/// Matches are found relative to the current cursor position, not a stored match index.
/// Uses regex via ops::search for full-field matching. Auto-expands collapsed subtrees
/// in track view to reveal matching tasks.
fn execute_search_dir(app: &mut App, direction: i32) {
    let pattern = match &app.last_search {
        Some(p) => p.clone(),
        None => return,
    };
    // Build case-insensitive regex; fall back to escaped literal on invalid regex
    let re = match Regex::new(&format!("(?i){}", pattern)) {
        Ok(r) => r,
        Err(_) => match Regex::new(&format!("(?i){}", regex::escape(&pattern))) {
            Ok(r) => r,
            Err(_) => return,
        },
    };

    app.search_match_count = Some(count_matches_for_pattern(app, &re));

    match app.view.clone() {
        View::Track(idx) => search_in_track(app, idx, &re, direction),
        View::Tracks => search_in_tracks_view(app, &re, direction),
        View::Inbox => search_in_inbox(app, &re, direction),
        View::Recent => search_in_recent(app, &re, direction),
    }
}

/// Given a sorted list of cursor positions where matches occur,
/// find the next one relative to `current_cursor` in the given direction.
/// Returns (index into positions, wrapped: bool) or None if empty.
/// direction: 0 = at or after cursor, +1 = strictly after, -1 = strictly before.
fn find_next_match_position(
    positions: &[usize],
    current_cursor: usize,
    direction: i32,
) -> Option<(usize, bool)> {
    if positions.is_empty() {
        return None;
    }
    match direction {
        0 => {
            // Initial search: find first match at or after cursor, fallback to first
            if let Some(idx) = positions.iter().position(|&p| p >= current_cursor) {
                Some((idx, false))
            } else {
                Some((0, false))
            }
        }
        1 => {
            // Next: find first match strictly after cursor
            if let Some(idx) = positions.iter().position(|&p| p > current_cursor) {
                Some((idx, false))
            } else {
                Some((0, true)) // wrap to top
            }
        }
        -1 => {
            // Prev: find last match strictly before cursor
            if let Some(idx) = positions.iter().rposition(|&p| p < current_cursor) {
                Some((idx, false))
            } else {
                Some((positions.len() - 1, true)) // wrap to bottom
            }
        }
        _ => None,
    }
}

/// Search within a single track view. Uses ops::search to find matching task IDs,
/// then auto-expands ancestors and jumps the cursor to the next match relative
/// to the current cursor position.
fn search_in_track(app: &mut App, view_idx: usize, re: &Regex, direction: i32) {
    let track_id = match app.active_track_ids.get(view_idx) {
        Some(id) => id.clone(),
        None => return,
    };

    // Use ops::search scoped to this track to get all matching task IDs
    let hits = search_tasks(&app.project, re, Some(&track_id));
    if hits.is_empty() {
        return;
    }

    // Deduplicate: multiple hits per task → unique task IDs in order
    let mut matched_task_ids: Vec<String> = Vec::new();
    for hit in &hits {
        if !matched_task_ids.contains(&hit.task_id) {
            matched_task_ids.push(hit.task_id.clone());
        }
    }

    // Auto-expand ancestors of all matching tasks so they become visible
    for task_id in &matched_task_ids {
        auto_expand_for_task(app, &track_id, task_id);
    }

    // Rebuild flat items after expansion and collect flat indices of matching tasks
    let flat_items = app.build_flat_items(&track_id);
    let track = match App::find_track_in_project(&app.project, &track_id) {
        Some(t) => t,
        None => return,
    };

    let mut match_positions: Vec<usize> = Vec::new();
    for (fi, item) in flat_items.iter().enumerate() {
        if let FlatItem::Task { section, path, .. } = item {
            if let Some(task) = resolve_task_from_track(track, *section, path) {
                if matched_task_ids
                    .iter()
                    .any(|id| task.id.as_deref() == Some(id.as_str()))
                {
                    match_positions.push(fi);
                }
            }
        }
    }

    if match_positions.is_empty() {
        return;
    }

    let current_cursor = app.track_states.get(&track_id).map_or(0, |s| s.cursor);

    if let Some((idx, wrapped)) = find_next_match_position(&match_positions, current_cursor, direction) {
        app.search_match_idx = idx;
        if wrapped {
            app.search_wrap_message = Some(if direction == 1 {
                "Search wrapped to top".to_string()
            } else {
                "Search wrapped to bottom".to_string()
            });
        }
        let state = app.get_track_state(&track_id);
        state.cursor = match_positions[idx];
    }
}

/// Auto-expand all ancestor nodes of a task so it becomes visible in the flat list.
fn auto_expand_for_task(app: &mut App, track_id: &str, target_task_id: &str) {
    // First pass: collect expand keys immutably
    let keys_to_expand = {
        let track = match App::find_track_in_project(&app.project, track_id) {
            Some(t) => t,
            None => return,
        };

        let mut keys = Vec::new();
        for section_kind in [SectionKind::Backlog, SectionKind::Parked, SectionKind::Done] {
            let tasks = track.section_tasks(section_kind);
            if let Some(path) = find_task_path(tasks, target_task_id) {
                for depth in 0..path.len().saturating_sub(1) {
                    let ancestor_path = &path[..=depth];
                    let mut current = match tasks.get(ancestor_path[0]) {
                        Some(t) => t,
                        None => break,
                    };
                    for &pi in &ancestor_path[1..] {
                        current = match current.subtasks.get(pi) {
                            Some(t) => t,
                            None => break,
                        };
                    }
                    keys.push(crate::tui::app::task_expand_key(
                        current,
                        section_kind,
                        ancestor_path,
                    ));
                }
                break;
            }
        }
        keys
    };

    // Second pass: insert keys mutably
    let state = app.get_track_state(track_id);
    for key in keys_to_expand {
        state.expanded.insert(key);
    }
}

/// Find the index path to a task with the given ID within a task tree.
/// Returns e.g. [2, 0, 1] meaning tasks[2].subtasks[0].subtasks[1].
fn find_task_path(tasks: &[crate::model::Task], target_id: &str) -> Option<Vec<usize>> {
    for (i, task) in tasks.iter().enumerate() {
        if task.id.as_deref() == Some(target_id) {
            return Some(vec![i]);
        }
        if let Some(mut sub_path) = find_task_path(&task.subtasks, target_id) {
            sub_path.insert(0, i);
            return Some(sub_path);
        }
    }
    None
}

fn search_in_tracks_view(app: &mut App, re: &Regex, direction: i32) {
    let match_positions: Vec<usize> = app
        .project
        .config
        .tracks
        .iter()
        .enumerate()
        .filter(|(_, tc)| re.is_match(&tc.name) || re.is_match(&tc.id))
        .map(|(i, _)| i)
        .collect();

    if match_positions.is_empty() {
        return;
    }

    let current_cursor = app.tracks_cursor;
    if let Some((idx, wrapped)) = find_next_match_position(&match_positions, current_cursor, direction) {
        app.search_match_idx = idx;
        if wrapped {
            app.search_wrap_message = Some(if direction == 1 {
                "Search wrapped to top".to_string()
            } else {
                "Search wrapped to bottom".to_string()
            });
        }
        app.tracks_cursor = match_positions[idx];
    }
}

fn search_in_inbox(app: &mut App, re: &Regex, direction: i32) {
    let inbox = match &app.project.inbox {
        Some(inbox) => inbox,
        None => return,
    };

    let hits = search_inbox(inbox, re);
    if hits.is_empty() {
        return;
    }

    // Deduplicate by item index and sort by position
    let mut match_positions: Vec<usize> = Vec::new();
    for hit in &hits {
        if !match_positions.contains(&hit.item_index) {
            match_positions.push(hit.item_index);
        }
    }
    match_positions.sort();

    let current_cursor = app.inbox_cursor;
    if let Some((idx, wrapped)) = find_next_match_position(&match_positions, current_cursor, direction) {
        app.search_match_idx = idx;
        if wrapped {
            app.search_wrap_message = Some(if direction == 1 {
                "Search wrapped to top".to_string()
            } else {
                "Search wrapped to bottom".to_string()
            });
        }
        app.inbox_cursor = match_positions[idx];
    }
}

fn search_in_recent(app: &mut App, re: &Regex, direction: i32) {
    // Search done tasks across all tracks using ops::search
    let all_hits = search_tasks(&app.project, re, None);

    // Collect done task IDs that matched (search_tasks searches all sections)
    let mut matched_done_ids: Vec<String> = Vec::new();
    for hit in &all_hits {
        // Check if this task is actually in a Done section
        for (tid, track) in &app.project.tracks {
            if *tid != hit.track_id {
                continue;
            }
            for done_task in track.section_tasks(SectionKind::Done) {
                if done_task.id.as_deref() == Some(hit.task_id.as_str())
                    && !matched_done_ids.contains(&hit.task_id)
                {
                    matched_done_ids.push(hit.task_id.clone());
                }
            }
        }
    }

    if matched_done_ids.is_empty() {
        return;
    }

    // Build the same ordering as recent_view: collect all done tasks sorted by resolved date
    let mut done_tasks: Vec<(String, String)> = Vec::new();
    for (track_id, track) in &app.project.tracks {
        for task in track.section_tasks(SectionKind::Done) {
            let resolved = task
                .metadata
                .iter()
                .find_map(|m| {
                    if let crate::model::Metadata::Resolved(d) = m {
                        Some(d.clone())
                    } else {
                        None
                    }
                })
                .unwrap_or_default();
            done_tasks.push((
                task.id.clone().unwrap_or_default(),
                format!("{}:{}", track_id, resolved),
            ));
        }
    }
    done_tasks.sort_by(|a, b| b.1.cmp(&a.1));

    // Find flat indices of matching done tasks
    let match_positions: Vec<usize> = done_tasks
        .iter()
        .enumerate()
        .filter(|(_, (id, _))| matched_done_ids.contains(id))
        .map(|(i, _)| i)
        .collect();

    if match_positions.is_empty() {
        return;
    }

    let current_cursor = app.recent_cursor;
    if let Some((idx, wrapped)) = find_next_match_position(&match_positions, current_cursor, direction) {
        app.search_match_idx = idx;
        if wrapped {
            app.search_wrap_message = Some(if direction == 1 {
                "Search wrapped to top".to_string()
            } else {
                "Search wrapped to bottom".to_string()
            });
        }
        app.recent_cursor = match_positions[idx];
    }
}

/// Count unique matches for a regex pattern in the current view.
fn count_matches_for_pattern(app: &App, re: &Regex) -> usize {
    match &app.view {
        View::Track(idx) => {
            let track_id = match app.active_track_ids.get(*idx) {
                Some(id) => id.as_str(),
                None => return 0,
            };
            let hits = search_tasks(&app.project, re, Some(track_id));
            let mut seen: Vec<&str> = Vec::new();
            for hit in &hits {
                if !seen.contains(&hit.task_id.as_str()) {
                    seen.push(&hit.task_id);
                }
            }
            seen.len()
        }
        View::Tracks => app
            .project
            .config
            .tracks
            .iter()
            .filter(|tc| re.is_match(&tc.name) || re.is_match(&tc.id))
            .count(),
        View::Inbox => {
            let inbox = match &app.project.inbox {
                Some(inbox) => inbox,
                None => return 0,
            };
            let hits = search_inbox(inbox, re);
            let mut seen: Vec<usize> = Vec::new();
            for hit in &hits {
                if !seen.contains(&hit.item_index) {
                    seen.push(hit.item_index);
                }
            }
            seen.len()
        }
        View::Recent => {
            let all_hits = search_tasks(&app.project, re, None);
            let mut matched_done_ids: Vec<String> = Vec::new();
            for hit in &all_hits {
                for (tid, track) in &app.project.tracks {
                    if *tid != hit.track_id {
                        continue;
                    }
                    for done_task in track.section_tasks(SectionKind::Done) {
                        if done_task.id.as_deref() == Some(hit.task_id.as_str())
                            && !matched_done_ids.contains(&hit.task_id)
                        {
                            matched_done_ids.push(hit.task_id.clone());
                        }
                    }
                }
            }
            matched_done_ids.len()
        }
    }
}

/// Update search_match_count based on current search input (for real-time display in Search mode).
fn update_match_count(app: &mut App) {
    if let Some(re) = app.active_search_re() {
        app.search_match_count = Some(count_matches_for_pattern(app, &re));
    } else {
        app.search_match_count = None;
    }
}

/// Switch to the next/prev tab. Direction: 1 = forward, -1 = backward.
fn switch_tab(app: &mut App, direction: i32) {
    let total_tracks = app.active_track_ids.len();
    // All views in order: Track(0)..Track(N-1), Tracks, Inbox, Recent
    let total_views = total_tracks + 3;

    let current_idx = match &app.view {
        View::Track(i) => *i,
        View::Tracks => total_tracks,
        View::Inbox => total_tracks + 1,
        View::Recent => total_tracks + 2,
    };

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
}
