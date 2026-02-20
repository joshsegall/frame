use std::collections::HashSet;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use regex::Regex;

use crate::io::project_io;
use crate::model::SectionKind;
use crate::model::task::{Metadata, Task};
use crate::ops::search::{self as search_ops, MatchField, search_inbox, search_tasks};
use crate::ops::task_ops::{self};

use crate::tui::app::{
    App, DetailRegion, FlatItem, MatchAnnotation, Mode, PendingMove, PendingMoveKind,
    RepeatEditRegion, RepeatableAction, SearchResultItem, SearchResultKind, SearchResults, View,
    resolve_task_from_flat,
};
use crate::tui::undo::Operation;

use super::*;

pub(super) fn handle_search(app: &mut App, key: KeyEvent) {
    // Route to project search prompt handler if active
    if app.project_search_active {
        handle_project_search_prompt(app, key);
        return;
    }

    match (key.modifiers, key.code) {
        // Cancel search
        (_, KeyCode::Esc) => {
            app.mode = if app.selection.is_empty() {
                Mode::Navigate
            } else {
                Mode::Select
            };
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
            app.mode = if app.selection.is_empty() {
                Mode::Navigate
            } else {
                Mode::Select
            };
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

// ---------------------------------------------------------------------------
// Project-wide search prompt handling
// ---------------------------------------------------------------------------

fn handle_project_search_prompt(app: &mut App, key: KeyEvent) {
    match (key.modifiers, key.code) {
        // Cancel
        (_, KeyCode::Esc) => {
            app.project_search_active = false;
            app.mode = Mode::Navigate;
        }

        // Execute search
        (_, KeyCode::Enter) => {
            if !app.project_search_input.is_empty() {
                execute_project_search(app);
            } else {
                app.project_search_active = false;
                app.mode = Mode::Navigate;
            }
        }

        // History: Up = older
        (_, KeyCode::Up) => {
            if !app.project_search_history.is_empty() {
                match app.project_search_history_index {
                    None => {
                        app.project_search_draft = app.project_search_input.clone();
                        app.project_search_history_index = Some(0);
                        app.project_search_input = app.project_search_history[0].clone();
                    }
                    Some(idx) => {
                        let next = idx + 1;
                        if next < app.project_search_history.len() {
                            app.project_search_history_index = Some(next);
                            app.project_search_input = app.project_search_history[next].clone();
                        }
                    }
                }
            }
        }

        // History: Down = newer
        (_, KeyCode::Down) => match app.project_search_history_index {
            None => {}
            Some(0) => {
                app.project_search_history_index = None;
                app.project_search_input = app.project_search_draft.clone();
            }
            Some(idx) => {
                let prev = idx - 1;
                app.project_search_history_index = Some(prev);
                app.project_search_input = app.project_search_history[prev].clone();
            }
        },

        // Backspace
        (_, KeyCode::Backspace) => {
            app.project_search_input.pop();
            if app.project_search_history_index.is_some() {
                app.project_search_history_index = None;
                app.project_search_draft.clear();
            }
        }

        // Type character
        (KeyModifiers::NONE | KeyModifiers::SHIFT, KeyCode::Char(c)) => {
            app.project_search_input.push(c);
            if app.project_search_history_index.is_some() {
                app.project_search_history_index = None;
                app.project_search_draft.clear();
            }
        }

        _ => {}
    }
}

/// Execute project-wide search and populate search results
fn execute_project_search(app: &mut App) {
    let query = app.project_search_input.clone();

    // Compile regex (case-insensitive, fall back to escaped literal)
    let re = match Regex::new(&format!("(?i){}", &query)) {
        Ok(r) => r,
        Err(_) => match Regex::new(&format!("(?i){}", regex::escape(&query))) {
            Ok(r) => r,
            Err(_) => {
                app.project_search_active = false;
                app.mode = Mode::Navigate;
                return;
            }
        },
    };

    // Remember return view — if already in Search view, preserve the original return_view
    let return_view = if matches!(app.view, View::Search) {
        app.project_search_results
            .as_ref()
            .map(|sr| sr.return_view.clone())
            .unwrap_or_else(|| app.view.clone())
    } else {
        app.view.clone()
    };

    let mut items: Vec<SearchResultItem> = Vec::new();
    let mut groups: Vec<(usize, String, usize)> = Vec::new();

    // Search active tracks in tab order
    for (track_idx, track_id) in app.active_track_ids.iter().enumerate() {
        let hits = search_tasks(&app.project, &re, Some(track_id));
        if hits.is_empty() {
            continue;
        }

        // Deduplicate by task_id, collect all matching fields
        let mut seen: HashSet<String> = HashSet::new();
        let mut track_items: Vec<SearchResultItem> = Vec::new();

        for hit in &hits {
            if !seen.insert(hit.task_id.clone()) {
                continue;
            }
            let track = App::find_track_in_project(&app.project, track_id);
            let task = track.and_then(|t| task_ops::find_task_in_track(t, &hit.task_id));

            let (title, state, tags) = if let Some(task) = task {
                (task.title.clone(), Some(task.state), task.tags.clone())
            } else {
                (hit.task_id.clone(), None, Vec::new())
            };

            let task_hits: Vec<_> = hits.iter().filter(|h| h.task_id == hit.task_id).collect();
            let (annotations, title_matches, id_matches) = build_annotations(&task_hits, task, &re);

            track_items.push(SearchResultItem {
                kind: SearchResultKind::Track {
                    track_idx,
                    track_id: track_id.clone(),
                },
                task_id: hit.task_id.clone(),
                title,
                state,
                tags,
                annotations,
                title_matches,
                id_matches,
            });
        }

        if !track_items.is_empty() {
            let track_name = app.track_name(track_id).to_string();
            let prefix = app
                .project
                .config
                .ids
                .prefixes
                .get(track_id.as_str())
                .cloned();
            let label = if let Some(pfx) = prefix {
                format!("{} ({})", track_name, pfx)
            } else {
                track_name
            };
            groups.push((items.len(), label, track_items.len()));
            items.extend(track_items);
        }
    }

    // Search inbox
    if let Some(ref inbox) = app.project.inbox {
        let inbox_hits = search_ops::search_inbox(inbox, &re);
        if !inbox_hits.is_empty() {
            let mut seen_indices: HashSet<usize> = HashSet::new();
            let mut inbox_items: Vec<SearchResultItem> = Vec::new();

            for hit in &inbox_hits {
                if !seen_indices.insert(hit.item_index) {
                    continue;
                }
                if let Some(item) = inbox.items.get(hit.item_index) {
                    let item_hits: Vec<_> = inbox_hits
                        .iter()
                        .filter(|h| h.item_index == hit.item_index)
                        .collect();
                    let title_matches = item_hits.iter().any(|h| h.field == MatchField::Title);

                    // Build annotations for non-title fields
                    let mut seen_fields: HashSet<String> = HashSet::new();
                    let mut annotations: Vec<MatchAnnotation> = Vec::new();
                    for ih in &item_hits {
                        if ih.field == MatchField::Title {
                            continue;
                        }
                        let key = format!("{:?}", ih.field);
                        if !seen_fields.insert(key) {
                            continue;
                        }
                        let snippet = match &ih.field {
                            MatchField::Body => {
                                item.body.as_ref().map(|b| snippet_around_match(b, 80, &re))
                            }
                            MatchField::Tag => Some(
                                item.tags
                                    .iter()
                                    .map(|t| format!("#{}", t))
                                    .collect::<Vec<_>>()
                                    .join(" "),
                            ),
                            _ => None,
                        };
                        if let Some(s) = snippet {
                            annotations.push(MatchAnnotation {
                                field: ih.field.clone(),
                                snippet: s,
                            });
                        }
                    }

                    inbox_items.push(SearchResultItem {
                        kind: SearchResultKind::Inbox {
                            item_index: hit.item_index,
                        },
                        task_id: String::new(),
                        title: item.title.clone(),
                        state: None,
                        tags: item.tags.clone(),
                        annotations,
                        title_matches,
                        id_matches: false,
                    });
                }
            }

            if !inbox_items.is_empty() {
                groups.push((items.len(), "Inbox".to_string(), inbox_items.len()));
                items.extend(inbox_items);
            }
        }
    }

    // Search archives
    if let Ok(archives) = project_io::load_archives(&app.project.frame_dir) {
        let archive_hits = search_ops::search_archive_tasks(&archives, &re, None);
        if !archive_hits.is_empty() {
            let mut seen: HashSet<(String, String)> = HashSet::new();
            let mut archive_items: Vec<SearchResultItem> = Vec::new();

            for hit in &archive_hits {
                if !seen.insert((hit.track_id.clone(), hit.task_id.clone())) {
                    continue;
                }
                let task = archives
                    .iter()
                    .find(|(tid, _)| tid == &hit.track_id)
                    .and_then(|(_, tasks)| find_task_by_id_recursive(tasks, &hit.task_id));

                let (title, state, tags) = if let Some(task) = task {
                    (task.title.clone(), Some(task.state), task.tags.clone())
                } else {
                    (hit.task_id.clone(), None, Vec::new())
                };

                let task_hits: Vec<_> = archive_hits
                    .iter()
                    .filter(|h| h.task_id == hit.task_id && h.track_id == hit.track_id)
                    .collect();
                let (annotations, title_matches, id_matches) =
                    build_annotations(&task_hits, task, &re);

                archive_items.push(SearchResultItem {
                    kind: SearchResultKind::Archive {
                        track_id: hit.track_id.clone(),
                    },
                    task_id: hit.task_id.clone(),
                    title,
                    state,
                    tags,
                    annotations,
                    title_matches,
                    id_matches,
                });
            }

            if !archive_items.is_empty() {
                groups.push((items.len(), "Archive".to_string(), archive_items.len()));
                items.extend(archive_items);
            }
        }
    }

    // Add query to history (dedup, cap at 200)
    app.project_search_history.retain(|s| s != &query);
    app.project_search_history.insert(0, query.clone());
    app.project_search_history.truncate(200);

    // Set results and switch to search view
    app.project_search_results = Some(SearchResults {
        query,
        regex: re,
        items,
        groups,
        cursor: 0,
        scroll_offset: 0,
        return_view,
    });
    app.view = View::Search;
    app.project_search_active = false;
    app.mode = Mode::Navigate;
}

/// Build annotations for all matching non-title/non-ID fields.
/// Returns (annotations, title_matches, id_matches).
fn build_annotations(
    task_hits: &[&search_ops::SearchHit],
    task: Option<&Task>,
    re: &Regex,
) -> (Vec<MatchAnnotation>, bool, bool) {
    let title_matches = task_hits.iter().any(|h| h.field == MatchField::Title);
    let id_matches = task_hits.iter().any(|h| h.field == MatchField::Id);

    let mut annotations: Vec<MatchAnnotation> = Vec::new();
    let mut seen_fields: HashSet<String> = HashSet::new();

    // Collect unique non-title, non-ID fields in hit order
    for hit in task_hits {
        if hit.field == MatchField::Title || hit.field == MatchField::Id {
            continue;
        }
        let key = format!("{:?}", hit.field);
        if !seen_fields.insert(key) {
            continue;
        }
        if let Some(snippet) = build_snippet_for_field(&hit.field, task, re) {
            annotations.push(MatchAnnotation {
                field: hit.field.clone(),
                snippet,
            });
        }
    }

    (annotations, title_matches, id_matches)
}

/// Build a snippet window centered around the first regex match.
/// For multi-line text, finds the first line containing a match and windows within it.
fn snippet_around_match(text: &str, max_chars: usize, re: &Regex) -> String {
    // For multi-line text, find the first line with a match
    let target_line = text
        .lines()
        .find(|line| re.is_match(line))
        .unwrap_or_else(|| text.lines().next().unwrap_or(text));

    let chars: Vec<char> = target_line.chars().collect();
    let total = chars.len();

    if total <= max_chars {
        return target_line.to_string();
    }

    // Find the char offset of the first match in this line
    let match_start_byte = re.find(target_line).map_or(0, |m| m.start());
    // Convert byte offset to char offset
    let match_char = target_line[..match_start_byte].chars().count();

    // Center the window around the match
    let half = max_chars / 2;
    let window_start = if match_char <= half {
        0
    } else if match_char + half >= total {
        total.saturating_sub(max_chars)
    } else {
        match_char - half
    };
    let window_end = (window_start + max_chars).min(total);

    let content: String = chars[window_start..window_end].iter().collect();
    let prefix = if window_start > 0 { "..." } else { "" };
    let suffix = if window_end < total { "..." } else { "" };
    format!("{}{}{}", prefix, content, suffix)
}

fn find_task_by_id_recursive<'a>(tasks: &'a [Task], id: &str) -> Option<&'a Task> {
    for task in tasks {
        if task.id.as_deref() == Some(id) {
            return Some(task);
        }
        if let Some(found) = find_task_by_id_recursive(&task.subtasks, id) {
            return Some(found);
        }
    }
    None
}

fn build_snippet_for_field(field: &MatchField, task: Option<&Task>, re: &Regex) -> Option<String> {
    let task = task?;
    match field {
        MatchField::Note => {
            for meta in &task.metadata {
                if let Metadata::Note(text) = meta {
                    return Some(snippet_around_match(text, 80, re));
                }
            }
            None
        }
        MatchField::Tag => Some(
            task.tags
                .iter()
                .map(|t| format!("#{}", t))
                .collect::<Vec<_>>()
                .join(" "),
        ),
        MatchField::Dep => {
            for meta in &task.metadata {
                if let Metadata::Dep(deps) = meta {
                    return Some(deps.join(", "));
                }
            }
            None
        }
        MatchField::Ref => {
            for meta in &task.metadata {
                if let Metadata::Ref(refs) = meta {
                    return Some(refs.join(", "));
                }
            }
            None
        }
        MatchField::Spec => {
            for meta in &task.metadata {
                if let Metadata::Spec(spec) = meta {
                    return Some(spec.clone());
                }
            }
            None
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Undo / Redo

/// Repeat the last recorded action on the current task (or selection).
pub(super) fn repeat_last_action(app: &mut App) {
    // Only works in track view or detail view
    if !matches!(app.view, View::Track(_) | View::Detail { .. }) {
        return;
    }

    let action = match app.last_action.clone() {
        Some(a) => a,
        None => return, // No-op if no stored action
    };

    // If in SELECT mode with a selection, replay on all selected tasks
    let in_select = app.mode == Mode::Select && !app.selection.is_empty();

    match action {
        RepeatableAction::CycleState => {
            if in_select {
                // Bulk cycle: apply cycle to each selected task individually
                repeat_bulk_cycle(app);
            } else {
                // Save and restore last_action around the call
                let saved = app.last_action.take();
                task_state_action(app, StateAction::Cycle);
                app.last_action = saved;
            }
        }
        RepeatableAction::SetState(target_state) => {
            if in_select {
                let saved = app.last_action.take();
                bulk_state_change(app, target_state);
                app.last_action = saved;
            } else {
                let sa = match target_state {
                    crate::model::TaskState::Done => StateAction::Done,
                    crate::model::TaskState::Blocked => StateAction::ToggleBlocked,
                    crate::model::TaskState::Todo => StateAction::SetTodo,
                    crate::model::TaskState::Parked => StateAction::ToggleParked,
                    crate::model::TaskState::Active => StateAction::Cycle,
                };
                let saved = app.last_action.take();
                task_state_action(app, sa);
                app.last_action = saved;
            }
        }
        RepeatableAction::TagEdit { adds, removes } => {
            if in_select {
                repeat_bulk_tag_apply(app, &adds, &removes);
            } else {
                repeat_single_tag_apply(app, &adds, &removes);
            }
        }
        RepeatableAction::DepEdit { adds, removes } => {
            if in_select {
                repeat_bulk_dep_apply(app, &adds, &removes);
            } else {
                repeat_single_dep_apply(app, &adds, &removes);
            }
        }
        RepeatableAction::ToggleCcTag => {
            let saved = app.last_action.take();
            toggle_cc_tag(app);
            app.last_action = saved;
        }
        RepeatableAction::EnterEdit(region) => {
            // Re-enter edit mode on the same region (don't replay text)
            match region {
                RepeatEditRegion::Title => {
                    if matches!(app.view, View::Detail { .. }) {
                        detail_enter_edit(app, false);
                    } else {
                        enter_title_edit(app);
                    }
                }
                RepeatEditRegion::Tags => {
                    if matches!(app.view, View::Detail { .. }) {
                        detail_jump_to_region_and_edit(app, DetailRegion::Tags, false);
                    } else {
                        enter_tag_edit(app);
                    }
                }
                RepeatEditRegion::Deps => {
                    if matches!(app.view, View::Detail { .. }) {
                        detail_jump_to_region_and_edit(app, DetailRegion::Deps, false);
                    }
                }
                RepeatEditRegion::Refs => {
                    if matches!(app.view, View::Detail { .. }) {
                        detail_jump_to_region_and_edit(app, DetailRegion::Refs, false);
                    }
                }
                RepeatEditRegion::Note => {
                    if matches!(app.view, View::Detail { .. }) {
                        detail_jump_to_region_and_edit(app, DetailRegion::Note, true);
                    }
                }
            }
        }
    }
}

/// Apply tag adds/removes to a single task (for . repeat).
pub(super) fn repeat_single_tag_apply(app: &mut App, adds: &[String], removes: &[String]) {
    let (track_id, task_id) = if let View::Detail { track_id, task_id } = &app.view {
        (track_id.clone(), task_id.clone())
    } else if let Some((track_id, task_id, _)) = app.cursor_task_id() {
        (track_id, task_id)
    } else {
        return;
    };

    let old_tags = App::find_track_in_project(&app.project, &track_id)
        .and_then(|t| task_ops::find_task_in_track(t, &task_id))
        .map(|t| t.tags.clone())
        .unwrap_or_default();

    let mut new_tags = old_tags.clone();
    for tag in adds {
        if !new_tags.contains(tag) {
            new_tags.push(tag.clone());
        }
    }
    for tag in removes {
        new_tags.retain(|t| t != tag);
    }

    if old_tags == new_tags {
        return; // No change — silently skip
    }

    let old_value = old_tags
        .iter()
        .map(|t| format!("#{}", t))
        .collect::<Vec<_>>()
        .join(" ");
    let new_value = new_tags
        .iter()
        .map(|t| format!("#{}", t))
        .collect::<Vec<_>>()
        .join(" ");

    let track = match app.find_track_mut(&track_id) {
        Some(t) => t,
        None => return,
    };
    if let Some(task) = task_ops::find_task_mut_in_track(track, &task_id) {
        task.tags = new_tags;
        task.mark_dirty();
    }
    let _ = app.save_track(&track_id);

    app.undo_stack.push(Operation::FieldEdit {
        track_id,
        task_id,
        field: "tags".to_string(),
        old_value,
        new_value,
    });
}

/// Apply tag adds/removes to all selected tasks (for . repeat in SELECT mode).
pub(super) fn repeat_bulk_tag_apply(app: &mut App, adds: &[String], removes: &[String]) {
    let track_id = match app.current_track_id() {
        Some(id) => id.to_string(),
        None => return,
    };

    let selected: Vec<String> = app.selection.iter().cloned().collect();
    let mut ops: Vec<Operation> = Vec::new();

    for task_id in &selected {
        let old_tags = App::find_track_in_project(&app.project, &track_id)
            .and_then(|t| task_ops::find_task_in_track(t, task_id))
            .map(|t| t.tags.clone())
            .unwrap_or_default();

        let mut new_tags = old_tags.clone();
        for tag in adds {
            if !new_tags.contains(tag) {
                new_tags.push(tag.clone());
            }
        }
        for tag in removes {
            new_tags.retain(|t| t != tag);
        }

        if old_tags == new_tags {
            continue;
        }

        let old_value = old_tags
            .iter()
            .map(|t| format!("#{}", t))
            .collect::<Vec<_>>()
            .join(" ");
        let new_value = new_tags
            .iter()
            .map(|t| format!("#{}", t))
            .collect::<Vec<_>>()
            .join(" ");

        let track = match app.find_track_mut(&track_id) {
            Some(t) => t,
            None => continue,
        };
        if let Some(task) = task_ops::find_task_mut_in_track(track, task_id) {
            task.tags = new_tags;
            task.mark_dirty();
        }

        ops.push(Operation::FieldEdit {
            track_id: track_id.clone(),
            task_id: task_id.clone(),
            field: "tags".to_string(),
            old_value,
            new_value,
        });
    }

    if !ops.is_empty() {
        app.undo_stack.push(Operation::Bulk(ops));
        let _ = app.save_track(&track_id);
    }
}

/// Apply dep adds/removes to a single task (for . repeat).
pub(super) fn repeat_single_dep_apply(app: &mut App, adds: &[String], removes: &[String]) {
    let (track_id, task_id) = if let View::Detail { track_id, task_id } = &app.view {
        (track_id.clone(), task_id.clone())
    } else if let Some((track_id, task_id, _)) = app.cursor_task_id() {
        (track_id, task_id)
    } else {
        return;
    };

    let old_deps = App::find_track_in_project(&app.project, &track_id)
        .and_then(|t| task_ops::find_task_in_track(t, &task_id))
        .and_then(|t| {
            t.metadata.iter().find_map(|m| {
                if let Metadata::Dep(deps) = m {
                    Some(deps.clone())
                } else {
                    None
                }
            })
        })
        .unwrap_or_default();

    let mut new_deps = old_deps.clone();
    for dep in adds {
        if !new_deps.contains(dep) {
            new_deps.push(dep.clone());
        }
    }
    for dep in removes {
        new_deps.retain(|d| d != dep);
    }

    if old_deps == new_deps {
        return; // No change — silently skip
    }

    let old_value = old_deps.join(", ");
    let new_value = new_deps.join(", ");

    let track = match app.find_track_mut(&track_id) {
        Some(t) => t,
        None => return,
    };
    if let Some(task) = task_ops::find_task_mut_in_track(track, &task_id) {
        task.metadata.retain(|m| !matches!(m, Metadata::Dep(_)));
        if !new_deps.is_empty() {
            task.metadata.push(Metadata::Dep(new_deps));
        }
        task.mark_dirty();
    }
    let _ = app.save_track(&track_id);

    app.undo_stack.push(Operation::FieldEdit {
        track_id,
        task_id,
        field: "deps".to_string(),
        old_value,
        new_value,
    });
}

/// Apply dep adds/removes to all selected tasks (for . repeat in SELECT mode).
pub(super) fn repeat_bulk_dep_apply(app: &mut App, adds: &[String], removes: &[String]) {
    let track_id = match app.current_track_id() {
        Some(id) => id.to_string(),
        None => return,
    };

    let selected: Vec<String> = app.selection.iter().cloned().collect();
    let mut ops: Vec<Operation> = Vec::new();

    for task_id in &selected {
        let old_deps = App::find_track_in_project(&app.project, &track_id)
            .and_then(|t| task_ops::find_task_in_track(t, task_id))
            .and_then(|t| {
                t.metadata.iter().find_map(|m| {
                    if let Metadata::Dep(deps) = m {
                        Some(deps.clone())
                    } else {
                        None
                    }
                })
            })
            .unwrap_or_default();

        let mut new_deps = old_deps.clone();
        for dep in adds {
            if !new_deps.contains(dep) {
                new_deps.push(dep.clone());
            }
        }
        for dep in removes {
            new_deps.retain(|d| d != dep);
        }

        if old_deps == new_deps {
            continue;
        }

        let old_value = old_deps.join(", ");
        let new_value = new_deps.join(", ");

        let track = match app.find_track_mut(&track_id) {
            Some(t) => t,
            None => continue,
        };
        if let Some(task) = task_ops::find_task_mut_in_track(track, task_id) {
            task.metadata.retain(|m| !matches!(m, Metadata::Dep(_)));
            if !new_deps.is_empty() {
                task.metadata.push(Metadata::Dep(new_deps));
            }
            task.mark_dirty();
        }

        ops.push(Operation::FieldEdit {
            track_id: track_id.clone(),
            task_id: task_id.clone(),
            field: "deps".to_string(),
            old_value,
            new_value,
        });
    }

    if !ops.is_empty() {
        app.undo_stack.push(Operation::Bulk(ops));
        let _ = app.save_track(&track_id);
    }
}

/// Apply cycle state to each selected task individually (for . repeat of Space).
pub(super) fn repeat_bulk_cycle(app: &mut App) {
    let track_id = match app.current_track_id() {
        Some(id) => id.to_string(),
        None => return,
    };

    let selected: Vec<String> = app.selection.iter().cloned().collect();
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
        let old_resolved = task.metadata.iter().find_map(|m| {
            if let Metadata::Resolved(d) = m {
                Some(d.clone())
            } else {
                None
            }
        });

        task_ops::cycle_state(task);

        let new_state = task.state;
        let new_resolved = task.metadata.iter().find_map(|m| {
            if let Metadata::Resolved(d) = m {
                Some(d.clone())
            } else {
                None
            }
        });

        if old_state != new_state {
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
    }
}

// ---------------------------------------------------------------------------
// Task state changes
// ---------------------------------------------------------------------------

/// Advance search by `direction` (+1 = next, -1 = prev) with proper wrapping.
pub(super) fn search_next(app: &mut App, direction: i32) {
    app.search_wrap_message = None;
    execute_search_dir(app, direction);
}

/// Execute search: find matches in the current view and jump to the match.
/// `direction` is +1 (next) or -1 (prev) or 0 (first from cursor).
/// Matches are found relative to the current cursor position, not a stored match index.
/// Uses regex via ops::search for full-field matching. Auto-expands collapsed subtrees
/// in track view to reveal matching tasks.
pub(super) fn execute_search_dir(app: &mut App, direction: i32) {
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
        View::Detail { track_id, task_id } => {
            search_in_detail(app, &track_id, &task_id, &re, direction)
        }
        View::Tracks => search_in_tracks_view(app, &re, direction),
        View::Inbox => search_in_inbox(app, &re, direction),
        View::Recent => search_in_recent(app, &re, direction),
        View::Search => {} // View search not applicable in project search results
    }
}

/// Given a sorted list of cursor positions where matches occur,
/// find the next one relative to `current_cursor` in the given direction.
/// Returns (index into positions, wrapped: bool) or None if empty.
/// direction: 0 = at or after cursor, +1 = strictly after, -1 = strictly before.
pub(super) fn find_next_match_position(
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
pub(super) fn search_in_track(app: &mut App, view_idx: usize, re: &Regex, direction: i32) {
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
        if let FlatItem::Task { section, path, .. } = item
            && let Some(task) = resolve_task_from_track(track, *section, path)
            && matched_task_ids
                .iter()
                .any(|id| task.id.as_deref() == Some(id.as_str()))
        {
            match_positions.push(fi);
        }
    }

    if match_positions.is_empty() {
        return;
    }

    let current_cursor = app.track_states.get(&track_id).map_or(0, |s| s.cursor);

    if let Some((idx, wrapped)) =
        find_next_match_position(&match_positions, current_cursor, direction)
    {
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
pub(super) fn auto_expand_for_task(app: &mut App, track_id: &str, target_task_id: &str) {
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
pub(super) fn find_task_path(tasks: &[crate::model::Task], target_id: &str) -> Option<Vec<usize>> {
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

pub(super) fn search_in_tracks_view(app: &mut App, re: &Regex, direction: i32) {
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
    if let Some((idx, wrapped)) =
        find_next_match_position(&match_positions, current_cursor, direction)
    {
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

pub(super) fn search_in_inbox(app: &mut App, re: &Regex, direction: i32) {
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
    if let Some((idx, wrapped)) =
        find_next_match_position(&match_positions, current_cursor, direction)
    {
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

pub(super) fn search_in_recent(app: &mut App, re: &Regex, direction: i32) {
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
    if let Some((idx, wrapped)) =
        find_next_match_position(&match_positions, current_cursor, direction)
    {
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

/// Search within the detail view. Cycles the region cursor (and subtask cursor)
/// through fields/subtasks that match the regex.
pub(super) fn search_in_detail(
    app: &mut App,
    track_id: &str,
    task_id: &str,
    re: &Regex,
    direction: i32,
) {
    let track = match App::find_track_in_project(&app.project, track_id) {
        Some(t) => t,
        None => return,
    };
    let task = match task_ops::find_task_in_track(track, task_id) {
        Some(t) => t,
        None => return,
    };

    // Build ordered list of match positions: (region, subtask_cursor_index)
    // Region order follows the detail view layout.
    let mut positions: Vec<(DetailRegion, Option<usize>)> = Vec::new();

    // Title: check ID and title text
    let title_matches =
        task.id.as_ref().is_some_and(|id| re.is_match(id)) || re.is_match(&task.title);
    if title_matches {
        positions.push((DetailRegion::Title, None));
    }

    // Tags
    if task.tags.iter().any(|tag| re.is_match(tag)) {
        positions.push((DetailRegion::Tags, None));
    }

    // Deps
    let has_dep_match = task.metadata.iter().any(|m| {
        if let Metadata::Dep(deps) = m {
            deps.iter().any(|d| re.is_match(d))
        } else {
            false
        }
    });
    if has_dep_match {
        positions.push((DetailRegion::Deps, None));
    }

    // Spec
    let has_spec_match = task
        .metadata
        .iter()
        .any(|m| matches!(m, Metadata::Spec(s) if re.is_match(s)));
    if has_spec_match {
        positions.push((DetailRegion::Spec, None));
    }

    // Refs
    let has_ref_match = task.metadata.iter().any(|m| {
        if let Metadata::Ref(refs) = m {
            refs.iter().any(|r| re.is_match(r))
        } else {
            false
        }
    });
    if has_ref_match {
        positions.push((DetailRegion::Refs, None));
    }

    // Note
    let has_note_match = task
        .metadata
        .iter()
        .any(|m| matches!(m, Metadata::Note(n) if re.is_match(n)));
    if has_note_match {
        positions.push((DetailRegion::Note, None));
    }

    // Subtasks: each matching subtask is a separate position
    let ds = match &app.detail_state {
        Some(ds) => ds,
        None => return,
    };
    for (si, sub_id) in ds.flat_subtask_ids.iter().enumerate() {
        // Find the subtask by ID and check if it matches
        if let Some(sub_task) = task_ops::find_task_in_track(track, sub_id) {
            let sub_matches = sub_task.id.as_ref().is_some_and(|id| re.is_match(id))
                || re.is_match(&sub_task.title)
                || sub_task.tags.iter().any(|tag| re.is_match(tag));
            if sub_matches {
                positions.push((DetailRegion::Subtasks, Some(si)));
            }
        }
    }

    if positions.is_empty() {
        return;
    }

    // Find current position index
    let current_region = ds.region;
    let current_subtask = ds.subtask_cursor;
    let current_pos = positions
        .iter()
        .position(|(r, si)| {
            *r == current_region && (*r != DetailRegion::Subtasks || *si == Some(current_subtask))
        })
        .unwrap_or(0);

    // Advance in direction
    let len = positions.len();
    let (new_idx, wrapped) = match direction {
        0 => {
            // First match at or after current
            let idx = positions
                .iter()
                .enumerate()
                .position(|(i, _)| i >= current_pos)
                .unwrap_or(0);
            (idx, false)
        }
        1 => {
            let next = (current_pos + 1) % len;
            (next, next <= current_pos)
        }
        -1 => {
            let prev = if current_pos == 0 {
                len - 1
            } else {
                current_pos - 1
            };
            (prev, prev >= current_pos)
        }
        _ => return,
    };

    if wrapped {
        app.search_wrap_message = Some(if direction == 1 {
            "Search wrapped to top".to_string()
        } else {
            "Search wrapped to bottom".to_string()
        });
    }

    let (target_region, target_subtask) = positions[new_idx];
    if let Some(ds) = &mut app.detail_state {
        // Reset note_view_line when leaving Note
        if ds.region == DetailRegion::Note && target_region != DetailRegion::Note {
            ds.note_view_line = None;
        }
        ds.region = target_region;
        if target_region == DetailRegion::Subtasks
            && let Some(si) = target_subtask
        {
            ds.subtask_cursor = si;
        }
        // When landing on Note, position the view cursor at the first matching line
        if target_region == DetailRegion::Note
            && let Some(note_header) = ds.note_header_line
        {
            let note_line_offset = find_first_matching_note_line(task, re);
            // note_header + 1 is the first content line in body coordinates
            ds.note_view_line = Some(note_header + 1 + note_line_offset);
        }
    }
}

/// Find the 0-indexed text line within a task's note that contains the first regex match.
/// Returns 0 if no note or no match found.
pub(super) fn find_first_matching_note_line(task: &Task, re: &Regex) -> usize {
    for meta in &task.metadata {
        if let Metadata::Note(text) = meta {
            for (i, line) in text.lines().enumerate() {
                if re.is_match(line) {
                    return i;
                }
            }
        }
    }
    0
}

/// Count unique matches for a regex pattern in the current view.
/// Only counts tasks that are actually visible (respects filters, excludes Done section).
pub(super) fn count_matches_for_pattern(app: &App, re: &Regex) -> usize {
    match &app.view {
        View::Detail { track_id, task_id } => {
            // Count matches within this single task's fields
            let track = match App::find_track_in_project(&app.project, track_id) {
                Some(t) => t,
                None => return 0,
            };
            let task = match task_ops::find_task_in_track(track, task_id) {
                Some(t) => t,
                None => return 0,
            };
            count_matches_in_task(task, re)
        }
        View::Track(idx) => {
            let track_id = match app.active_track_ids.get(*idx) {
                Some(id) => id.as_str(),
                None => return 0,
            };
            // Build flat items (excludes Done, respects filters)
            let flat_items = app.build_flat_items(track_id);
            let track = match App::find_track_in_project(&app.project, track_id) {
                Some(t) => t,
                None => return 0,
            };
            // Collect visible (non-context) task IDs
            let mut visible_ids: Vec<String> = Vec::new();
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
                        visible_ids.push(id.clone());
                    }
                }
            }
            // Search and filter to visible tasks only
            let hits = search_tasks(&app.project, re, Some(track_id));
            let mut seen: Vec<&str> = Vec::new();
            for hit in &hits {
                if visible_ids.iter().any(|id| id == &hit.task_id)
                    && !seen.contains(&hit.task_id.as_str())
                {
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
        View::Search => 0,
    }
}

/// Count matches across all searchable fields of a single task.
/// Returns 1 if any field matches, 0 otherwise.
pub(super) fn count_matches_in_task(task: &Task, re: &Regex) -> usize {
    // Check ID
    if let Some(id) = &task.id
        && re.is_match(id)
    {
        return 1;
    }
    // Check title
    if re.is_match(&task.title) {
        return 1;
    }
    // Check tags
    for tag in &task.tags {
        if re.is_match(tag) {
            return 1;
        }
    }
    // Check metadata fields
    for meta in &task.metadata {
        match meta {
            Metadata::Note(text) => {
                if re.is_match(text) {
                    return 1;
                }
            }
            Metadata::Dep(deps) => {
                for dep in deps {
                    if re.is_match(dep) {
                        return 1;
                    }
                }
            }
            Metadata::Ref(refs) => {
                for r in refs {
                    if re.is_match(r) {
                        return 1;
                    }
                }
            }
            Metadata::Spec(spec) => {
                if re.is_match(spec) {
                    return 1;
                }
            }
            _ => {}
        }
    }
    0
}

/// Update search_match_count based on current search input (for real-time display in Search mode).
pub(super) fn update_match_count(app: &mut App) {
    if let Some(re) = app.active_search_re() {
        app.search_match_count = Some(count_matches_for_pattern(app, &re));
    } else {
        app.search_match_count = None;
    }
}
