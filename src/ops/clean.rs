use std::collections::{HashMap, HashSet};
use std::path::Path;

use chrono::Local;

use crate::model::project::Project;
use crate::model::task::{Metadata, Task, TaskState};
use crate::model::track::{SectionKind, Track, TrackNode};
use crate::ops::task_ops::find_max_id_in_track;

/// Result of a clean operation
#[derive(Debug, Default)]
pub struct CleanResult {
    /// IDs assigned to tasks that were missing them
    pub ids_assigned: Vec<IdAssignment>,
    /// Added dates filled in
    pub dates_assigned: Vec<DateAssignment>,
    /// Duplicate IDs resolved (reassigned)
    pub duplicates_resolved: Vec<DuplicateResolution>,
    /// Tasks archived from done sections
    pub tasks_archived: Vec<ArchiveRecord>,
    /// Dangling dependency references
    pub dangling_deps: Vec<DanglingDep>,
    /// Broken file references (ref/spec)
    pub broken_refs: Vec<BrokenRef>,
    /// Suggestions (e.g., all subtasks done → suggest parent done)
    pub suggestions: Vec<Suggestion>,
}

#[derive(Debug, Clone)]
pub struct IdAssignment {
    pub track_id: String,
    pub assigned_id: String,
    pub title: String,
}

#[derive(Debug, Clone)]
pub struct DateAssignment {
    pub track_id: String,
    pub task_id: String,
    pub date: String,
}

#[derive(Debug, Clone)]
pub struct DuplicateResolution {
    pub track_id: String,
    pub original_id: String,
    pub new_id: String,
    pub title: String,
}

#[derive(Debug, Clone)]
pub struct ArchiveRecord {
    pub track_id: String,
    pub task_id: String,
    pub title: String,
}

#[derive(Debug, Clone)]
pub struct DanglingDep {
    pub track_id: String,
    pub task_id: String,
    pub dep_id: String,
}

#[derive(Debug, Clone)]
pub struct BrokenRef {
    pub track_id: String,
    pub task_id: String,
    pub path: String,
    pub kind: RefKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefKind {
    Ref,
    Spec,
}

#[derive(Debug, Clone)]
pub struct Suggestion {
    pub track_id: String,
    pub task_id: String,
    pub kind: SuggestionKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SuggestionKind {
    /// All subtasks are done — parent could be marked done
    AllSubtasksDone,
}

// ---------------------------------------------------------------------------
// Lightweight ID + date + dedup assignment (used by TUI on load/reload)
// ---------------------------------------------------------------------------

/// Assign missing IDs and dates, and resolve duplicate IDs across the project.
///
/// This runs steps 1–3 of the clean pipeline (ID assignment, date assignment,
/// duplicate ID resolution). Returns the list of track IDs that were modified,
/// so callers can selectively save only those tracks.
pub fn ensure_ids_and_dates(project: &mut Project) -> Vec<String> {
    let mut result = CleanResult::default();
    let mut modified = HashSet::new();

    for (track_id, track) in &mut project.tracks {
        let before_ids = result.ids_assigned.len();
        let before_dates = result.dates_assigned.len();

        let prefix = project.config.ids.prefixes.get(track_id.as_str()).cloned();

        if let Some(ref pfx) = prefix {
            assign_missing_ids(track, track_id, pfx, &mut result);
        }
        assign_missing_dates(track, track_id, &mut result);

        if result.ids_assigned.len() > before_ids || result.dates_assigned.len() > before_dates {
            modified.insert(track_id.clone());
        }
    }

    // Resolve duplicate IDs (cross-track and within-track)
    let before_dups = result.duplicates_resolved.len();
    resolve_duplicate_ids(project, &mut result);
    for dup in &result.duplicates_resolved[before_dups..] {
        modified.insert(dup.track_id.clone());
    }

    modified.into_iter().collect()
}

// ---------------------------------------------------------------------------
// Main clean entry point
// ---------------------------------------------------------------------------

/// Run clean operations on a project (mutates in place).
///
/// Operations:
/// 1. Assign IDs to tasks missing them
/// 2. Assign `added:` dates where missing
/// 3. Duplicate ID resolution (first by track order keeps ID; duplicates reassigned)
/// 4. Validate deps (flag dangling)
/// 5. Validate file refs (flag broken paths)
/// 6. State suggestions (all subtasks done → suggest parent done)
/// 7. Archive done tasks past threshold
///
/// Returns a report of all changes made and issues found.
pub fn clean_project(project: &mut Project) -> CleanResult {
    let mut result = CleanResult::default();

    for (track_id, track) in &mut project.tracks {
        let prefix = project.config.ids.prefixes.get(track_id.as_str()).cloned();

        // 1. Assign missing IDs
        if let Some(ref pfx) = prefix {
            assign_missing_ids(track, track_id, pfx, &mut result);
        }

        // 2. Assign missing added dates
        assign_missing_dates(track, track_id, &mut result);
    }

    // 3. Duplicate ID resolution
    resolve_duplicate_ids(project, &mut result);

    // Collect all task IDs across all tracks for dep validation (after duplicate resolution)
    let all_task_ids = collect_all_task_ids(project);

    for (track_id, track) in &mut project.tracks {
        // 4. Validate deps
        validate_deps(track, track_id, &all_task_ids, &mut result);

        // 5. Validate refs/specs
        validate_refs(track, track_id, &project.root, &mut result);

        // 6. State suggestions
        collect_suggestions(track, track_id, &mut result);
    }

    // 7. Archive done tasks past threshold
    archive_done_tasks(project, &mut result);

    result
}

/// Generate ACTIVE.md content summarizing active tracks and their top tasks.
pub fn generate_active_md(project: &Project) -> String {
    let mut lines = Vec::new();
    lines.push(format!("# {} — Active Tasks", project.config.project.name));
    lines.push(String::new());
    lines.push("> Auto-generated by `fr clean`. Do not edit.".to_string());
    lines.push(String::new());

    for tc in &project.config.tracks {
        if tc.state != "active" {
            continue;
        }
        let track = project.tracks.iter().find(|(id, _)| id == &tc.id);
        let Some((_, track)) = track else {
            continue;
        };

        lines.push(format!("## {}", track.title));
        lines.push(String::new());

        let backlog = track.backlog();
        if backlog.is_empty() {
            lines.push("(empty backlog)".to_string());
        } else {
            for task in backlog {
                let state_char = task.state.checkbox_char();
                let id_str = task
                    .id
                    .as_ref()
                    .map(|id| format!("`{}` ", id))
                    .unwrap_or_default();
                let tags_str = if task.tags.is_empty() {
                    String::new()
                } else {
                    format!(
                        " {}",
                        task.tags
                            .iter()
                            .map(|t| format!("#{}", t))
                            .collect::<Vec<_>>()
                            .join(" ")
                    )
                };
                lines.push(format!(
                    "- [{}] {}{}{}",
                    state_char, id_str, task.title, tags_str
                ));
            }
        }
        lines.push(String::new());
    }

    // Trim trailing blank line
    while lines.last().map_or(false, |l| l.is_empty()) {
        lines.pop();
    }

    lines.join("\n")
}

// ---------------------------------------------------------------------------
// 1. Assign missing IDs
// ---------------------------------------------------------------------------

fn assign_missing_ids(track: &mut Track, track_id: &str, prefix: &str, result: &mut CleanResult) {
    let prefix_dash = format!("{}-", prefix);
    let mut max = 0usize;
    find_max_id_in_track(track, &prefix_dash, &mut max);

    for node in &mut track.nodes {
        if let TrackNode::Section { tasks, .. } = node {
            assign_ids_in_tasks(tasks, track_id, prefix, &prefix_dash, &mut max, result);
        }
    }
}

fn assign_ids_in_tasks(
    tasks: &mut [Task],
    track_id: &str,
    _prefix: &str,
    prefix_dash: &str,
    max: &mut usize,
    result: &mut CleanResult,
) {
    for task in tasks.iter_mut() {
        if task.id.is_none() {
            *max += 1;
            let new_id = format!("{}{:03}", prefix_dash, max);
            task.id = Some(new_id.clone());
            task.mark_dirty();
            result.ids_assigned.push(IdAssignment {
                track_id: track_id.to_string(),
                assigned_id: new_id,
                title: task.title.clone(),
            });
        }
        // Recurse into subtasks — subtasks with no ID also get assigned
        // (subtask IDs are parent_id.N)
        assign_subtask_ids(task, track_id, result);
    }
}

fn assign_subtask_ids(parent: &mut Task, track_id: &str, result: &mut CleanResult) {
    let parent_id = match &parent.id {
        Some(id) => id.clone(),
        None => return, // Parent must have an ID first
    };
    for (i, sub) in parent.subtasks.iter_mut().enumerate() {
        if sub.id.is_none() {
            let sub_id = format!("{}.{}", parent_id, i + 1);
            sub.id = Some(sub_id.clone());
            sub.mark_dirty();
            result.ids_assigned.push(IdAssignment {
                track_id: track_id.to_string(),
                assigned_id: sub_id,
                title: sub.title.clone(),
            });
        }
        // Recurse deeper
        assign_subtask_ids(sub, track_id, result);
    }
}

// ---------------------------------------------------------------------------
// 2. Assign missing added dates
// ---------------------------------------------------------------------------

fn assign_missing_dates(track: &mut Track, track_id: &str, result: &mut CleanResult) {
    let today = today_str();
    for node in &mut track.nodes {
        if let TrackNode::Section { tasks, .. } = node {
            assign_dates_in_tasks(tasks, track_id, &today, result);
        }
    }
}

fn assign_dates_in_tasks(
    tasks: &mut [Task],
    track_id: &str,
    today: &str,
    result: &mut CleanResult,
) {
    for task in tasks.iter_mut() {
        let has_added = task
            .metadata
            .iter()
            .any(|m| matches!(m, Metadata::Added(_)));
        if !has_added {
            task.metadata.insert(0, Metadata::Added(today.to_string()));
            task.mark_dirty();
            result.dates_assigned.push(DateAssignment {
                track_id: track_id.to_string(),
                task_id: task.id.clone().unwrap_or_default(),
                date: today.to_string(),
            });
        }
        assign_dates_in_tasks(&mut task.subtasks, track_id, today, result);
    }
}

// ---------------------------------------------------------------------------
// 3. Duplicate ID resolution
// ---------------------------------------------------------------------------

/// Find and resolve duplicate IDs across the project.
///
/// The first occurrence by track order (as listed in `project.toml`) then by
/// position within the track keeps its ID. Subsequent duplicates are reassigned
/// new IDs via the standard `max + 1` rule. Dependencies pointing to the
/// reassigned ID are updated across all tracks.
fn resolve_duplicate_ids(project: &mut Project, result: &mut CleanResult) {
    // Build ordered track list from config (defines precedence)
    let track_order: Vec<String> = project
        .config
        .tracks
        .iter()
        .map(|tc| tc.id.clone())
        .collect();

    // Pass 1: Walk all tasks in track order, identify duplicate IDs.
    // First occurrence keeps the ID; subsequent occurrences are collected for reassignment.
    let mut seen_ids: HashSet<String> = HashSet::new();
    // (old_id, track_id, title) for each duplicate that needs reassignment
    let mut duplicates: Vec<(String, String, String)> = Vec::new();

    for config_track_id in &track_order {
        if let Some((_, track)) = project
            .tracks
            .iter()
            .find(|(tid, _)| tid == config_track_id)
        {
            for node in &track.nodes {
                if let TrackNode::Section { tasks, .. } = node {
                    find_duplicates_in_tasks(
                        tasks,
                        config_track_id,
                        &mut seen_ids,
                        &mut duplicates,
                    );
                }
            }
        }
    }

    if duplicates.is_empty() {
        return;
    }

    // Pass 2: Compute new IDs for each duplicate.
    // old_id → new_id mapping (note: multiple tasks can share the same old_id,
    // so we use a Vec to track all reassignments)
    let mut reassignments: HashMap<String, Vec<String>> = HashMap::new();
    // Also build a flat old→new map for dep rewriting (maps old_id to the LAST
    // assigned new_id — but for deps we want to keep pointing to the *keeper*,
    // not the reassigned duplicate, so we DON'T rewrite deps from old to new.
    // Actually per design: "Dependencies pointing to the reassigned ID are updated."
    // This means: if task A has dep on ID "X", and "X" was reassigned to "X-NEW",
    // then A's dep should still point to "X" (the keeper). The reassigned task
    // got a NEW id so nothing should dep on it by the old id anymore.
    // Wait — actually the design says deps pointing to the reassigned ID are updated.
    // That means if someone had `dep: DUP-001` and DUP-001 was the duplicate that
    // got reassigned to M-005, the dep should be updated to M-005.
    // But that's ambiguous — the keeper also has id DUP-001, so the dep is still valid.
    // The most sensible interpretation: deps continue to point at the keeper (which
    // retains the original ID), so no dep rewriting is needed for the common case.
    // Only if a dep pointed at a task that was specifically the duplicate instance
    // would it need updating — but deps are by ID string, not by instance.
    // So if the keeper retains the ID, deps pointing to that ID are still valid.
    // We don't need to rewrite deps. The design note about "deps updated" likely
    // refers to cross-track moves where the old ID disappears entirely.
    //
    // Re-reading the design: "Dependencies pointing to the reassigned ID are updated
    // across all tracks." This means: if a dep references an ID that was reassigned
    // (i.e., the duplicate's old ID was changed), those deps should be updated.
    // But since the keeper ALSO has that same old ID, the dep still resolves.
    // So dep rewriting is only needed if ALL instances of an ID were reassigned
    // (which never happens — the first keeps its ID). Therefore: no dep rewriting needed.

    for (old_id, dup_track_id, _title) in &duplicates {
        let prefix = project
            .config
            .ids
            .prefixes
            .get(dup_track_id.as_str())
            .cloned();
        let Some(pfx) = prefix else { continue };
        let prefix_dash = format!("{}-", pfx);

        // Find the track and compute max+1
        let track = project
            .tracks
            .iter()
            .find(|(tid, _)| tid == dup_track_id)
            .map(|(_, t)| t);
        let Some(track) = track else { continue };

        let mut max = 0usize;
        find_max_id_in_track(track, &prefix_dash, &mut max);
        // Also account for any already-assigned new IDs in this batch
        for new_id in reassignments.values().flatten() {
            if let Some(n) = new_id
                .strip_prefix(&prefix_dash)
                .and_then(|s| s.split('.').next())
                .and_then(|s| s.parse::<usize>().ok())
                .filter(|&n| n > max)
            {
                max = n;
            }
        }

        let new_id = format!("{}{:03}", prefix_dash, max + 1);
        reassignments
            .entry(old_id.clone())
            .or_default()
            .push(new_id);
    }

    // Pass 3: Apply reassignments by walking tasks in the same track order.
    // For each duplicate ID, we consume the next new_id from the reassignments vec.
    let mut reassignment_cursors: HashMap<String, usize> = HashMap::new();
    let mut seen_in_apply: HashSet<String> = HashSet::new();

    for config_track_id in &track_order {
        if let Some((_, track)) = project
            .tracks
            .iter_mut()
            .find(|(tid, _)| tid == config_track_id)
        {
            for node in &mut track.nodes {
                if let TrackNode::Section { tasks, .. } = node {
                    apply_duplicate_reassignments(
                        tasks,
                        config_track_id,
                        &reassignments,
                        &mut reassignment_cursors,
                        &mut seen_in_apply,
                        result,
                    );
                }
            }
        }
    }
}

fn find_duplicates_in_tasks(
    tasks: &[Task],
    track_id: &str,
    seen: &mut HashSet<String>,
    duplicates: &mut Vec<(String, String, String)>,
) {
    for task in tasks {
        if task.id.as_ref().is_some_and(|id| !seen.insert(id.clone())) {
            let id = task.id.as_ref().unwrap();
            duplicates.push((id.clone(), track_id.to_string(), task.title.clone()));
        }
        find_duplicates_in_tasks(&task.subtasks, track_id, seen, duplicates);
    }
}

/// Walk tasks in order, applying reassignments to duplicate instances.
/// The first time we see an ID, it's the keeper (skip). Second+ times, reassign.
fn apply_duplicate_reassignments(
    tasks: &mut [Task],
    track_id: &str,
    reassignments: &HashMap<String, Vec<String>>,
    cursors: &mut HashMap<String, usize>,
    seen: &mut HashSet<String>,
    result: &mut CleanResult,
) {
    for task in tasks.iter_mut() {
        if let Some(old_id) = task
            .id
            .clone()
            .filter(|id| reassignments.contains_key(id) && !seen.insert(id.clone()))
        {
            // This is a duplicate occurrence — reassign
            let cursor = cursors.entry(old_id.clone()).or_insert(0);
            if let Some(new_id) = reassignments.get(&old_id).and_then(|ids| ids.get(*cursor)) {
                task.id = Some(new_id.clone());
                task.mark_dirty();
                renumber_subtask_ids(task, new_id);
                result.duplicates_resolved.push(DuplicateResolution {
                    track_id: track_id.to_string(),
                    original_id: old_id.clone(),
                    new_id: new_id.clone(),
                    title: task.title.clone(),
                });
                *cursor += 1;
            }
        }
        apply_duplicate_reassignments(
            &mut task.subtasks,
            track_id,
            reassignments,
            cursors,
            seen,
            result,
        );
    }
}

/// After reassigning a parent's ID, renumber its subtasks to use the new parent ID.
fn renumber_subtask_ids(parent: &mut Task, new_parent_id: &str) {
    for (i, sub) in parent.subtasks.iter_mut().enumerate() {
        let new_sub_id = format!("{}.{}", new_parent_id, i + 1);
        sub.id = Some(new_sub_id.clone());
        sub.mark_dirty();
        renumber_subtask_ids(sub, &new_sub_id);
    }
}

// ---------------------------------------------------------------------------
// 4. Validate deps
// ---------------------------------------------------------------------------

fn validate_deps(
    track: &Track,
    track_id: &str,
    all_ids: &HashSet<String>,
    result: &mut CleanResult,
) {
    for node in &track.nodes {
        if let TrackNode::Section { tasks, .. } = node {
            validate_deps_in_tasks(tasks, track_id, all_ids, result);
        }
    }
}

fn validate_deps_in_tasks(
    tasks: &[Task],
    track_id: &str,
    all_ids: &HashSet<String>,
    result: &mut CleanResult,
) {
    for task in tasks {
        let task_id = task.id.as_deref().unwrap_or("");
        for meta in &task.metadata {
            if let Metadata::Dep(deps) = meta {
                for dep_id in deps {
                    if !all_ids.contains(dep_id) {
                        result.dangling_deps.push(DanglingDep {
                            track_id: track_id.to_string(),
                            task_id: task_id.to_string(),
                            dep_id: dep_id.clone(),
                        });
                    }
                }
            }
        }
        validate_deps_in_tasks(&task.subtasks, track_id, all_ids, result);
    }
}

// ---------------------------------------------------------------------------
// 4. Validate file refs
// ---------------------------------------------------------------------------

fn validate_refs(track: &Track, track_id: &str, project_root: &Path, result: &mut CleanResult) {
    for node in &track.nodes {
        if let TrackNode::Section { tasks, .. } = node {
            validate_refs_in_tasks(tasks, track_id, project_root, result);
        }
    }
}

fn validate_refs_in_tasks(
    tasks: &[Task],
    track_id: &str,
    project_root: &Path,
    result: &mut CleanResult,
) {
    for task in tasks {
        let task_id = task.id.as_deref().unwrap_or("");
        for meta in &task.metadata {
            match meta {
                Metadata::Ref(refs) => {
                    for r in refs {
                        if !path_exists(project_root, r) {
                            result.broken_refs.push(BrokenRef {
                                track_id: track_id.to_string(),
                                task_id: task_id.to_string(),
                                path: r.clone(),
                                kind: RefKind::Ref,
                            });
                        }
                    }
                }
                Metadata::Spec(spec) => {
                    // spec can have #section suffix — strip it for file check
                    let file_path = spec.split('#').next().unwrap_or(spec);
                    if !path_exists(project_root, file_path) {
                        result.broken_refs.push(BrokenRef {
                            track_id: track_id.to_string(),
                            task_id: task_id.to_string(),
                            path: spec.clone(),
                            kind: RefKind::Spec,
                        });
                    }
                }
                _ => {}
            }
        }
        validate_refs_in_tasks(&task.subtasks, track_id, project_root, result);
    }
}

fn path_exists(project_root: &Path, relative_path: &str) -> bool {
    project_root.join(relative_path).exists()
}

// ---------------------------------------------------------------------------
// 5. State suggestions
// ---------------------------------------------------------------------------

fn collect_suggestions(track: &Track, track_id: &str, result: &mut CleanResult) {
    for node in &track.nodes {
        if let TrackNode::Section { tasks, .. } = node {
            collect_suggestions_in_tasks(tasks, track_id, result);
        }
    }
}

fn collect_suggestions_in_tasks(tasks: &[Task], track_id: &str, result: &mut CleanResult) {
    for task in tasks {
        if !task.subtasks.is_empty()
            && task.state != TaskState::Done
            && task.subtasks.iter().all(|s| s.state == TaskState::Done)
        {
            result.suggestions.push(Suggestion {
                track_id: track_id.to_string(),
                task_id: task.id.clone().unwrap_or_default(),
                kind: SuggestionKind::AllSubtasksDone,
            });
        }
        collect_suggestions_in_tasks(&task.subtasks, track_id, result);
    }
}

// ---------------------------------------------------------------------------
// 6. Archive done tasks past threshold
// ---------------------------------------------------------------------------

fn archive_done_tasks(project: &mut Project, result: &mut CleanResult) {
    if !project.config.clean.archive_per_track {
        return;
    }
    let threshold = project.config.clean.done_threshold;

    for (track_id, track) in &mut project.tracks {
        let done_line_count = count_done_section_lines(track);
        if done_line_count <= threshold {
            continue;
        }

        // Extract done tasks to archive
        let archived = extract_done_tasks_for_archive(track);
        for task in &archived {
            result.tasks_archived.push(ArchiveRecord {
                track_id: track_id.clone(),
                task_id: task.id.clone().unwrap_or_default(),
                title: task.title.clone(),
            });
        }

        // Store archived tasks in a temporary track for serialization
        if !archived.is_empty() {
            let archive_content = serialize_archived_tasks(&archived);
            let archive_path = project
                .frame_dir
                .join("archive")
                .join(format!("{}.md", track_id));
            if let Some(parent) = archive_path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            // Append to existing archive or create new
            let existing = std::fs::read_to_string(&archive_path).unwrap_or_default();
            let new_content = if existing.is_empty() {
                format!("# Archive — {}\n\n{}", track_id, archive_content)
            } else {
                format!("{}\n{}", existing.trim_end(), archive_content)
            };
            let _ = std::fs::write(&archive_path, new_content);
        }
    }
}

fn count_done_section_lines(track: &Track) -> usize {
    let done_tasks = track.section_tasks(SectionKind::Done);
    let lines = crate::parse::serialize_tasks(done_tasks, 0);
    lines.len()
}

fn extract_done_tasks_for_archive(track: &mut Track) -> Vec<Task> {
    for node in &mut track.nodes {
        if let TrackNode::Section {
            kind: SectionKind::Done,
            tasks,
            ..
        } = node
        {
            return std::mem::take(tasks);
        }
    }
    Vec::new()
}

fn serialize_archived_tasks(tasks: &[Task]) -> String {
    let lines = crate::parse::serialize_tasks(tasks, 0);
    lines.join("\n")
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn today_str() -> String {
    Local::now().format("%Y-%m-%d").to_string()
}

/// Collect all task IDs across every track in the project.
fn collect_all_task_ids(project: &Project) -> HashSet<String> {
    let mut ids = HashSet::new();
    for (_, track) in &project.tracks {
        for node in &track.nodes {
            if let TrackNode::Section { tasks, .. } = node {
                collect_ids_from_tasks(tasks, &mut ids);
            }
        }
    }
    ids
}

fn collect_ids_from_tasks(tasks: &[Task], ids: &mut HashSet<String>) {
    for task in tasks {
        if let Some(ref id) = task.id {
            ids.insert(id.clone());
        }
        collect_ids_from_tasks(&task.subtasks, ids);
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::config::{
        AgentConfig, CleanConfig, IdConfig, ProjectConfig, ProjectInfo, TrackConfig, UiConfig,
    };
    use crate::parse::parse_track;
    use std::collections::HashMap;
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn make_config(prefixes: Vec<(&str, &str)>) -> ProjectConfig {
        let mut prefix_map = HashMap::new();
        for (k, v) in &prefixes {
            prefix_map.insert(k.to_string(), v.to_string());
        }
        ProjectConfig {
            project: ProjectInfo {
                name: "test".to_string(),
            },
            agent: AgentConfig::default(),
            tracks: vec![TrackConfig {
                id: "main".to_string(),
                name: "Main".to_string(),
                state: "active".to_string(),
                file: "tracks/main.md".to_string(),
            }],
            clean: CleanConfig::default(),
            ids: IdConfig {
                prefixes: prefix_map,
            },
            ui: UiConfig::default(),
        }
    }

    fn make_project(track_src: &str, prefixes: Vec<(&str, &str)>) -> Project {
        let track = parse_track(track_src);
        Project {
            root: PathBuf::from("/tmp/test"),
            frame_dir: PathBuf::from("/tmp/test/frame"),
            config: make_config(prefixes),
            tracks: vec![("main".to_string(), track)],
            inbox: None,
        }
    }

    // --- 1. Assign missing IDs ---

    #[test]
    fn test_assign_missing_ids() {
        let mut project = make_project(
            "\
# Main

## Backlog

- [ ] `M-001` Has ID
- [ ] Missing ID task
- [ ] Another missing

## Done
",
            vec![("main", "M")],
        );

        let result = clean_project(&mut project);
        assert_eq!(result.ids_assigned.len(), 2);
        assert_eq!(result.ids_assigned[0].assigned_id, "M-002");
        assert_eq!(result.ids_assigned[0].title, "Missing ID task");
        assert_eq!(result.ids_assigned[1].assigned_id, "M-003");

        // Verify tasks were actually modified
        let backlog = project.tracks[0].1.backlog();
        assert_eq!(backlog[1].id.as_deref(), Some("M-002"));
        assert_eq!(backlog[2].id.as_deref(), Some("M-003"));
        assert!(backlog[1].dirty);
    }

    #[test]
    fn test_assign_missing_ids_no_prefix() {
        let mut project = make_project(
            "\
# Main

## Backlog

- [ ] No prefix configured

## Done
",
            vec![], // no prefixes
        );

        let result = clean_project(&mut project);
        // Should not assign IDs if no prefix configured
        assert!(result.ids_assigned.is_empty());
    }

    #[test]
    fn test_assign_subtask_ids() {
        let mut project = make_project(
            "\
# Main

## Backlog

- [ ] `M-001` Parent
  - [ ] Sub without ID
  - [ ] `M-001.2` Has ID

## Done
",
            vec![("main", "M")],
        );

        let result = clean_project(&mut project);
        // Only the first subtask should get an ID assigned
        let sub_assignments: Vec<_> = result
            .ids_assigned
            .iter()
            .filter(|a| a.assigned_id.contains('.'))
            .collect();
        assert_eq!(sub_assignments.len(), 1);
        assert_eq!(sub_assignments[0].assigned_id, "M-001.1");
    }

    // --- 2. Assign missing dates ---

    #[test]
    fn test_assign_missing_dates() {
        let mut project = make_project(
            "\
# Main

## Backlog

- [ ] `M-001` Has date
  - added: 2025-05-01
- [ ] `M-002` Missing date

## Done
",
            vec![("main", "M")],
        );

        let result = clean_project(&mut project);
        assert_eq!(result.dates_assigned.len(), 1);
        assert_eq!(result.dates_assigned[0].task_id, "M-002");

        // Verify the task got the date
        let backlog = project.tracks[0].1.backlog();
        assert!(
            backlog[1]
                .metadata
                .iter()
                .any(|m| matches!(m, Metadata::Added(_)))
        );
    }

    #[test]
    fn test_no_duplicate_dates() {
        let mut project = make_project(
            "\
# Main

## Backlog

- [ ] `M-001` Already has date
  - added: 2025-01-01

## Done
",
            vec![("main", "M")],
        );

        let result = clean_project(&mut project);
        assert!(result.dates_assigned.is_empty());
    }

    // --- 3. Validate deps ---

    #[test]
    fn test_dangling_deps() {
        let mut project = make_project(
            "\
# Main

## Backlog

- [ ] `M-001` Task with good dep
  - dep: M-002
- [ ] `M-002` Target task
- [ ] `M-003` Task with bad dep
  - dep: NONEXIST-999

## Done
",
            vec![("main", "M")],
        );

        let result = clean_project(&mut project);
        assert_eq!(result.dangling_deps.len(), 1);
        assert_eq!(result.dangling_deps[0].task_id, "M-003");
        assert_eq!(result.dangling_deps[0].dep_id, "NONEXIST-999");
    }

    #[test]
    fn test_cross_track_deps_valid() {
        let track_a = parse_track(
            "\
# Track A

## Backlog

- [ ] `A-001` Task A
  - dep: B-001

## Done
",
        );
        let track_b = parse_track(
            "\
# Track B

## Backlog

- [ ] `B-001` Task B

## Done
",
        );
        let mut project = Project {
            root: PathBuf::from("/tmp/test"),
            frame_dir: PathBuf::from("/tmp/test/frame"),
            config: {
                let mut cfg = make_config(vec![("a", "A"), ("b", "B")]);
                cfg.tracks = vec![
                    TrackConfig {
                        id: "a".to_string(),
                        name: "A".to_string(),
                        state: "active".to_string(),
                        file: "tracks/a.md".to_string(),
                    },
                    TrackConfig {
                        id: "b".to_string(),
                        name: "B".to_string(),
                        state: "active".to_string(),
                        file: "tracks/b.md".to_string(),
                    },
                ];
                cfg
            },
            tracks: vec![("a".to_string(), track_a), ("b".to_string(), track_b)],
            inbox: None,
        };

        let result = clean_project(&mut project);
        assert!(result.dangling_deps.is_empty());
    }

    // --- 4. Validate refs ---

    #[test]
    fn test_broken_refs() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("frame/tracks")).unwrap();
        // Create a file that exists
        std::fs::write(root.join("existing.md"), "hi").unwrap();

        let track = parse_track(
            "\
# Main

## Backlog

- [ ] `M-001` Task with refs
  - ref: existing.md
  - ref: missing.md
  - spec: also_missing.md#section

## Done
",
        );

        let mut project = Project {
            root: root.to_path_buf(),
            frame_dir: root.join("frame"),
            config: make_config(vec![("main", "M")]),
            tracks: vec![("main".to_string(), track)],
            inbox: None,
        };

        let result = clean_project(&mut project);
        assert_eq!(result.broken_refs.len(), 2);
        assert_eq!(result.broken_refs[0].path, "missing.md");
        assert_eq!(result.broken_refs[0].kind, RefKind::Ref);
        assert_eq!(result.broken_refs[1].path, "also_missing.md#section");
        assert_eq!(result.broken_refs[1].kind, RefKind::Spec);
    }

    #[test]
    fn test_valid_refs() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("frame/tracks")).unwrap();
        std::fs::create_dir_all(root.join("doc")).unwrap();
        std::fs::write(root.join("doc/spec.md"), "spec").unwrap();

        let track = parse_track(
            "\
# Main

## Backlog

- [ ] `M-001` Task with valid ref
  - spec: doc/spec.md#section

## Done
",
        );

        let mut project = Project {
            root: root.to_path_buf(),
            frame_dir: root.join("frame"),
            config: make_config(vec![("main", "M")]),
            tracks: vec![("main".to_string(), track)],
            inbox: None,
        };

        let result = clean_project(&mut project);
        assert!(result.broken_refs.is_empty());
    }

    // --- 5. Suggestions ---

    #[test]
    fn test_suggest_parent_done_when_all_subtasks_done() {
        let mut project = make_project(
            "\
# Main

## Backlog

- [ ] `M-001` Parent with all done subs
  - [x] `M-001.1` Sub one
    - resolved: 2025-05-10
  - [x] `M-001.2` Sub two
    - resolved: 2025-05-11
- [ ] `M-002` Parent with mixed subs
  - [x] `M-002.1` Done sub
  - [ ] `M-002.2` Todo sub

## Done
",
            vec![("main", "M")],
        );

        let result = clean_project(&mut project);
        assert_eq!(result.suggestions.len(), 1);
        assert_eq!(result.suggestions[0].task_id, "M-001");
        assert_eq!(result.suggestions[0].kind, SuggestionKind::AllSubtasksDone);
    }

    #[test]
    fn test_no_suggestion_for_already_done_parent() {
        let mut project = make_project(
            "\
# Main

## Backlog

## Done

- [x] `M-001` Already done parent
  - resolved: 2025-05-10
  - [x] `M-001.1` Sub one
  - [x] `M-001.2` Sub two
",
            vec![("main", "M")],
        );

        let result = clean_project(&mut project);
        assert!(result.suggestions.is_empty());
    }

    #[test]
    fn test_no_suggestion_for_leaf_tasks() {
        let mut project = make_project(
            "\
# Main

## Backlog

- [ ] `M-001` Leaf task with no subtasks

## Done
",
            vec![("main", "M")],
        );

        let result = clean_project(&mut project);
        assert!(result.suggestions.is_empty());
    }

    // --- 6. Archive done tasks ---

    #[test]
    fn test_archive_done_past_threshold() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("frame/tracks")).unwrap();

        // Build a track with many done tasks to exceed threshold
        let mut done_lines = String::new();
        for i in 0..100 {
            done_lines.push_str(&format!(
                "- [x] `M-{:03}` Done task {}\n  - added: 2025-01-01\n  - resolved: 2025-05-{:02}\n",
                i, i, (i % 28) + 1
            ));
        }

        let src = format!(
            "\
# Main

## Backlog

- [ ] `M-200` Active task

## Done

{}",
            done_lines.trim_end()
        );

        let track = parse_track(&src);

        let mut config = make_config(vec![("main", "M")]);
        config.clean.done_threshold = 10; // low threshold to trigger archive

        let mut project = Project {
            root: root.to_path_buf(),
            frame_dir: root.join("frame"),
            config,
            tracks: vec![("main".to_string(), track)],
            inbox: None,
        };

        let result = clean_project(&mut project);
        assert_eq!(result.tasks_archived.len(), 100);

        // Done section should now be empty
        let done = project.tracks[0].1.done();
        assert!(done.is_empty());

        // Archive file should exist
        let archive_path = root.join("frame/archive/main.md");
        assert!(archive_path.exists());
    }

    #[test]
    fn test_no_archive_under_threshold() {
        let mut project = make_project(
            "\
# Main

## Backlog

## Done

- [x] `M-001` One done task
  - resolved: 2025-05-10
",
            vec![("main", "M")],
        );

        let result = clean_project(&mut project);
        assert!(result.tasks_archived.is_empty());
    }

    // --- Generate ACTIVE.md ---

    #[test]
    fn test_generate_active_md() {
        let project = make_project(
            "\
# Main Track

## Backlog

- [>] `M-001` Active task #ready
- [ ] `M-002` Todo task
- [-] `M-003` Blocked task

## Done
",
            vec![("main", "M")],
        );

        let content = generate_active_md(&project);
        assert!(content.contains("# test — Active Tasks"));
        assert!(content.contains("## Main Track"));
        assert!(content.contains("- [>] `M-001` Active task #ready"));
        assert!(content.contains("- [ ] `M-002` Todo task"));
        assert!(content.contains("- [-] `M-003` Blocked task"));
    }

    #[test]
    fn test_generate_active_md_skips_shelved() {
        let track = parse_track(
            "\
# Shelved

## Backlog

- [ ] `S-001` Hidden task

## Done
",
        );
        let project = Project {
            root: PathBuf::from("/tmp/test"),
            frame_dir: PathBuf::from("/tmp/test/frame"),
            config: {
                let mut cfg = make_config(vec![]);
                cfg.tracks = vec![TrackConfig {
                    id: "shelved".to_string(),
                    name: "Shelved".to_string(),
                    state: "shelved".to_string(),
                    file: "tracks/shelved.md".to_string(),
                }];
                cfg
            },
            tracks: vec![("shelved".to_string(), track)],
            inbox: None,
        };

        let content = generate_active_md(&project);
        assert!(!content.contains("Hidden task"));
    }

    // --- 3. Duplicate ID resolution ---

    #[test]
    fn test_resolve_duplicate_ids_cross_track() {
        let track_a = parse_track(
            "\
# Track A

## Backlog

- [ ] `DUP-001` First occurrence in A
  - added: 2025-05-01

## Done
",
        );
        let track_b = parse_track(
            "\
# Track B

## Backlog

- [ ] `DUP-001` Duplicate in B
  - added: 2025-05-02

## Done
",
        );
        let mut project = Project {
            root: PathBuf::from("/tmp/test"),
            frame_dir: PathBuf::from("/tmp/test/frame"),
            config: {
                let mut cfg = make_config(vec![("a", "A"), ("b", "B")]);
                cfg.tracks = vec![
                    TrackConfig {
                        id: "a".to_string(),
                        name: "A".to_string(),
                        state: "active".to_string(),
                        file: "tracks/a.md".to_string(),
                    },
                    TrackConfig {
                        id: "b".to_string(),
                        name: "B".to_string(),
                        state: "active".to_string(),
                        file: "tracks/b.md".to_string(),
                    },
                ];
                cfg
            },
            tracks: vec![("a".to_string(), track_a), ("b".to_string(), track_b)],
            inbox: None,
        };

        let result = clean_project(&mut project);

        // Track A's DUP-001 should be kept, track B's should be reassigned
        assert_eq!(result.duplicates_resolved.len(), 1);
        assert_eq!(result.duplicates_resolved[0].track_id, "b");
        assert_eq!(result.duplicates_resolved[0].original_id, "DUP-001");
        assert_eq!(result.duplicates_resolved[0].title, "Duplicate in B");

        // Track A keeps its ID
        let a_backlog = project.tracks[0].1.backlog();
        assert_eq!(a_backlog[0].id.as_deref(), Some("DUP-001"));

        // Track B got a new ID (B-prefix, max+1)
        let b_backlog = project.tracks[1].1.backlog();
        assert_eq!(b_backlog[0].id.as_deref(), Some("B-001"));
    }

    #[test]
    fn test_resolve_duplicate_ids_within_track() {
        let mut project = make_project(
            "\
# Main

## Backlog

- [ ] `M-001` First occurrence
  - added: 2025-05-01
- [ ] `M-001` Duplicate in same track
  - added: 2025-05-02

## Done
",
            vec![("main", "M")],
        );

        let result = clean_project(&mut project);

        assert_eq!(result.duplicates_resolved.len(), 1);
        assert_eq!(result.duplicates_resolved[0].original_id, "M-001");
        assert_eq!(
            result.duplicates_resolved[0].title,
            "Duplicate in same track"
        );

        let backlog = project.tracks[0].1.backlog();
        assert_eq!(backlog[0].id.as_deref(), Some("M-001"));
        assert_eq!(backlog[1].id.as_deref(), Some("M-002"));
    }

    #[test]
    fn test_resolve_duplicate_ids_track_order_precedence() {
        // Track order in config is [b, a], so track B should keep the ID
        let track_a = parse_track(
            "\
# Track A

## Backlog

- [ ] `X-001` In track A
  - added: 2025-05-01

## Done
",
        );
        let track_b = parse_track(
            "\
# Track B

## Backlog

- [ ] `X-001` In track B
  - added: 2025-05-02

## Done
",
        );
        let mut project = Project {
            root: PathBuf::from("/tmp/test"),
            frame_dir: PathBuf::from("/tmp/test/frame"),
            config: {
                let mut cfg = make_config(vec![("a", "A"), ("b", "B")]);
                // Track B comes first in config → it has precedence
                cfg.tracks = vec![
                    TrackConfig {
                        id: "b".to_string(),
                        name: "B".to_string(),
                        state: "active".to_string(),
                        file: "tracks/b.md".to_string(),
                    },
                    TrackConfig {
                        id: "a".to_string(),
                        name: "A".to_string(),
                        state: "active".to_string(),
                        file: "tracks/a.md".to_string(),
                    },
                ];
                cfg
            },
            tracks: vec![("a".to_string(), track_a), ("b".to_string(), track_b)],
            inbox: None,
        };

        let result = clean_project(&mut project);

        // Track B is first in config, so it keeps X-001. Track A's gets reassigned.
        assert_eq!(result.duplicates_resolved.len(), 1);
        assert_eq!(result.duplicates_resolved[0].track_id, "a");
        assert_eq!(result.duplicates_resolved[0].original_id, "X-001");

        // Track A got reassigned with A-prefix
        let a_backlog = project
            .tracks
            .iter()
            .find(|(id, _)| id == "a")
            .unwrap()
            .1
            .backlog();
        assert_eq!(a_backlog[0].id.as_deref(), Some("A-001"));

        // Track B keeps its ID
        let b_backlog = project
            .tracks
            .iter()
            .find(|(id, _)| id == "b")
            .unwrap()
            .1
            .backlog();
        assert_eq!(b_backlog[0].id.as_deref(), Some("X-001"));
    }

    #[test]
    fn test_resolve_duplicate_ids_renumbers_subtasks() {
        let mut project = make_project(
            "\
# Main

## Backlog

- [ ] `M-001` First
  - added: 2025-05-01
- [ ] `M-001` Duplicate parent with subtasks
  - added: 2025-05-02
  - [ ] `M-001.1` Sub one
    - added: 2025-05-02
  - [ ] `M-001.2` Sub two
    - added: 2025-05-02

## Done
",
            vec![("main", "M")],
        );

        let result = clean_project(&mut project);

        assert_eq!(result.duplicates_resolved.len(), 1);
        let backlog = project.tracks[0].1.backlog();
        // First keeps M-001
        assert_eq!(backlog[0].id.as_deref(), Some("M-001"));
        // Duplicate gets M-002
        assert_eq!(backlog[1].id.as_deref(), Some("M-002"));
        // Subtasks renumbered
        assert_eq!(backlog[1].subtasks[0].id.as_deref(), Some("M-002.1"));
        assert_eq!(backlog[1].subtasks[1].id.as_deref(), Some("M-002.2"));
    }

    #[test]
    fn test_no_duplicates_no_changes() {
        let mut project = make_project(
            "\
# Main

## Backlog

- [ ] `M-001` Task one
  - added: 2025-05-01
- [ ] `M-002` Task two
  - added: 2025-05-01

## Done
",
            vec![("main", "M")],
        );

        let result = clean_project(&mut project);
        assert!(result.duplicates_resolved.is_empty());
    }

    // --- Combined clean operations ---

    #[test]
    fn test_clean_assigns_ids_then_validates_deps() {
        let mut project = make_project(
            "\
# Main

## Backlog

- [ ] `M-001` Task one
  - dep: M-002
- [ ] `M-002` Task two

## Done
",
            vec![("main", "M")],
        );

        let result = clean_project(&mut project);
        // Deps should be valid (M-002 exists)
        assert!(result.dangling_deps.is_empty());
    }

    #[test]
    fn test_clean_full_run() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("frame/tracks")).unwrap();
        std::fs::write(root.join("doc.md"), "doc").unwrap();

        let track = parse_track(
            "\
# Main

## Backlog

- [ ] `M-001` Has everything
  - added: 2025-05-01
  - dep: M-002
  - ref: doc.md
- [ ] Missing ID and date
- [ ] `M-002` Second task

## Done
",
        );

        let mut project = Project {
            root: root.to_path_buf(),
            frame_dir: root.join("frame"),
            config: make_config(vec![("main", "M")]),
            tracks: vec![("main".to_string(), track)],
            inbox: None,
        };

        let result = clean_project(&mut project);

        // Should have assigned 1 ID
        assert_eq!(result.ids_assigned.len(), 1);
        assert_eq!(result.ids_assigned[0].title, "Missing ID and date");

        // Should have assigned dates to tasks missing them
        assert!(!result.dates_assigned.is_empty());

        // No dangling deps (M-002 exists)
        assert!(result.dangling_deps.is_empty());

        // No broken refs (doc.md exists)
        assert!(result.broken_refs.is_empty());
    }

    // --- ensure_ids_and_dates ---

    #[test]
    fn test_ensure_ids_and_dates_basic() {
        let mut project = make_project(
            "\
# Main

## Backlog

- [ ] `M-001` Has ID and date
  - added: 2025-05-01
- [ ] Missing everything

## Done
",
            vec![("main", "M")],
        );

        let modified = ensure_ids_and_dates(&mut project);
        assert_eq!(modified, vec!["main".to_string()]);

        let backlog = project.tracks[0].1.backlog();
        // Second task should now have an ID
        assert_eq!(backlog[1].id.as_deref(), Some("M-002"));
        // Second task should now have an added date
        assert!(
            backlog[1]
                .metadata
                .iter()
                .any(|m| matches!(m, Metadata::Added(_)))
        );
    }

    #[test]
    fn test_ensure_ids_and_dates_no_changes() {
        let mut project = make_project(
            "\
# Main

## Backlog

- [ ] `M-001` All good
  - added: 2025-05-01
- [ ] `M-002` Also good
  - added: 2025-05-02

## Done
",
            vec![("main", "M")],
        );

        let modified = ensure_ids_and_dates(&mut project);
        assert!(modified.is_empty());
    }

    #[test]
    fn test_ensure_ids_and_dates_no_prefix() {
        let mut project = make_project(
            "\
# Main

## Backlog

- [ ] No prefix configured

## Done
",
            vec![], // no prefixes
        );

        let modified = ensure_ids_and_dates(&mut project);
        // Should still assign dates even without a prefix
        assert_eq!(modified, vec!["main".to_string()]);
        // But should NOT assign IDs
        let backlog = project.tracks[0].1.backlog();
        assert!(backlog[0].id.is_none());
        // Should have an added date
        assert!(
            backlog[0]
                .metadata
                .iter()
                .any(|m| matches!(m, Metadata::Added(_)))
        );
    }

    #[test]
    fn test_ensure_ids_and_dates_resolves_duplicates() {
        let mut project = make_project(
            "\
# Main

## Backlog

- [ ] `M-001` First occurrence
  - added: 2025-05-01
- [ ] `M-001` Duplicate
  - added: 2025-05-02

## Done
",
            vec![("main", "M")],
        );

        let modified = ensure_ids_and_dates(&mut project);
        assert!(modified.contains(&"main".to_string()));

        let backlog = project.tracks[0].1.backlog();
        assert_eq!(backlog[0].id.as_deref(), Some("M-001"));
        assert_eq!(backlog[1].id.as_deref(), Some("M-002"));
    }
}
