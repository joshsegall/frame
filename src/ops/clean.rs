use std::collections::{HashMap, HashSet};
use std::path::Path;

use chrono::Local;
use serde::Serialize;

use crate::io::actors::IdScope;
use crate::model::archive::Archive;
use crate::model::project::Project;
use crate::model::task::{Metadata, Task, TaskState};
use crate::model::task_id::{TaskId, Token};
use crate::model::track::{SectionKind, Track, TrackNode};
use crate::ops::ids::Mint;
use crate::ops::refs as refs_ops;
use crate::ops::task_ops::renumber_subtasks;

/// One task whose fields are out of canonical order.
#[derive(Debug, Clone, Serialize)]
pub struct Normalized {
    pub track_id: String,
    /// The task's ID, or its title when it has none.
    pub task: String,
    /// Field keys in the order the file holds them.
    pub was: Vec<&'static str>,
    /// Field keys in canonical order — what the task was rewritten to, or for a
    /// task in [`NormalizeResult::skipped`] what it would have become.
    pub now: Vec<&'static str>,
}

/// Result of a normalize pass.
#[derive(Debug, Default, Serialize)]
pub struct NormalizeResult {
    pub reordered: Vec<Normalized>,
    /// Tasks left alone because a note moved last would have swallowed their
    /// stranded lines. Reported rather than skipped silently: the file keeps a
    /// field order the rest of the project no longer uses, and the reason is
    /// damage the reader may want to fix by hand.
    pub skipped: Vec<Normalized>,
}

/// Rewrite every task whose fields are out of canonical order.
///
/// The serializer already writes a task in canonical order the first time frame
/// edits it, so a project converges on its own — task by task, over as long as
/// it takes to touch every task. This is the same convergence asked for at once.
///
/// **Deliberately not part of [`clean_project`], and not reachable from the
/// TUI's auto-clean.** Clean runs unattended after every file reload when
/// `auto_clean` is on, so it may only do what is correct with nobody watching
/// (`doc/cli.md`). Reordering every task in a project is a large, boring diff,
/// and this codebase has already paid for one of those: a `fr clean` run that
/// rewrote a whole track to fill one `resolved:` date, with a one-line deletion
/// hidden inside it that got committed unread. A separate function is what makes
/// "auto-clean cannot reach this" a fact about the call graph rather than a
/// promise.
///
/// Only tasks that are actually out of order are marked dirty. The rest stay
/// clean and serialize verbatim from `source_text`, so the diff is exactly the
/// tasks named in the result and nothing else. That also makes the pass
/// idempotent: a second run finds nothing.
///
/// Marking a task dirty re-canonicalizes **all** of that task's own lines, not
/// only the order — checkbox spacing, tag placement, the note block form, and
/// the `", "` join on `dep:`/`ref:`/`spec:`. That is the same canonical form the
/// task would have reached anyway the next time anyone edited it.
pub fn normalize_project(project: &mut Project) -> NormalizeResult {
    let mut result = NormalizeResult::default();
    for (track_id, track) in &mut project.tracks {
        for node in &mut track.nodes {
            if let TrackNode::Section { tasks, .. } = node {
                normalize_tasks(tasks, track_id, 0, true, &mut result);
            }
        }
    }
    result
}

/// The same survey, without touching anything.
///
/// What a plain `fr clean` reports. Field order is not damage and not a `fr
/// check` finding, so without this a user has nowhere to learn that their
/// project predates the canonical order or that [`normalize_project`] exists —
/// clean would say "project is clean" about a project with 599 tasks it would
/// rewrite the moment anyone asked. It sits with clean's other report-only
/// findings (dangling deps, broken refs, suggestions): named, not acted on.
pub fn scan_field_order(project: &Project) -> NormalizeResult {
    let mut result = NormalizeResult::default();
    for (track_id, track) in &project.tracks {
        for node in &track.nodes {
            if let TrackNode::Section { tasks, .. } = node {
                // Surveying cannot mutate, and the walk below takes `&mut` so
                // the applying caller can. Cloning the tasks is what lets one
                // rule serve both rather than two walks drifting apart; a survey
                // is not on any hot path.
                let mut copy = tasks.clone();
                normalize_tasks(&mut copy, track_id, 0, false, &mut result);
            }
        }
    }
    result
}

fn normalize_tasks(
    tasks: &mut [Task],
    track_id: &str,
    indent: usize,
    apply: bool,
    result: &mut NormalizeResult,
) {
    for task in tasks.iter_mut() {
        if !crate::model::task::metadata_is_ordered(task) {
            let record = Normalized {
                track_id: track_id.to_string(),
                task: task
                    .id
                    .as_ref()
                    .map(|i| i.to_string())
                    .unwrap_or_else(|| task.title.clone()),
                was: task.metadata.iter().map(|m| m.key()).collect(),
                now: crate::model::task::ordered_metadata(task)
                    .iter()
                    .map(|m| m.key())
                    .collect(),
            };
            // Ask the writer, rather than re-deriving its rule here: a task it
            // would leave alone must not be marked dirty, or the pass would
            // report a change the file never receives.
            if crate::parse::stranded_would_be_absorbed(task, indent) {
                result.skipped.push(record);
            } else {
                if apply {
                    // Order the model too, not only the file the serializer is
                    // about to write. Leaving the two disagreeing would keep this
                    // task reported as out of order for the rest of the process,
                    // and a second pass would rewrite what the first already
                    // fixed.
                    crate::model::task::sort_metadata(task);
                    task.mark_dirty();
                }
                result.reordered.push(record);
            }
        }
        normalize_tasks(&mut task.subtasks, track_id, indent + 2, apply, result);
    }
}

/// Result of a clean operation
#[derive(Debug, Default, Serialize)]
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
    /// Top-level tasks moved to the correct section based on state
    pub sections_reconciled: Vec<SectionReconcile>,
    /// Suggestions (e.g., all subtasks done → suggest parent done)
    pub suggestions: Vec<Suggestion>,
}

#[derive(Debug, Clone, Serialize)]
pub struct IdAssignment {
    pub track_id: String,
    pub assigned_id: String,
    pub title: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct DateAssignment {
    pub track_id: String,
    pub task_id: String,
    pub date: String,
    pub kind: DateKind,
}

/// Which date a [`DateAssignment`] filled in. Both are reported the same way, but
/// naming the field keeps `T-001 → 2026-07-31` from being ambiguous now that
/// clean fills two kinds of date.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DateKind {
    Added,
    Resolved,
}

impl DateKind {
    pub fn key(self) -> &'static str {
        match self {
            DateKind::Added => "added",
            DateKind::Resolved => "resolved",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct DuplicateResolution {
    pub track_id: String,
    pub original_id: String,
    pub new_id: String,
    pub title: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ArchiveRecord {
    pub track_id: String,
    pub task_id: String,
    pub title: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct DanglingDep {
    pub track_id: String,
    pub task_id: String,
    pub dep_id: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct BrokenRef {
    pub track_id: String,
    pub task_id: String,
    pub path: String,
    pub kind: RefKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum RefKind {
    Ref,
    Spec,
}

#[derive(Debug, Clone, Serialize)]
pub struct SectionReconcile {
    pub track_id: String,
    pub task_id: String,
    pub from: SectionKind,
    pub to: SectionKind,
}

#[derive(Debug, Clone, Serialize)]
pub struct Suggestion {
    pub track_id: String,
    pub task_id: String,
    pub kind: SuggestionKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
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
///
/// `scope` honors the strict null policy: with [`IdScope::Mint`] any IDs minted
/// are scoped to that namespace (`None` = null); with [`IdScope::Unclaimed`] the
/// minting steps (ID assignment and duplicate reassignment) are **skipped** —
/// tasks are left ID-less — while date filling and section reconciliation still
/// run, since those mint nothing.
pub fn ensure_ids_and_dates(project: &mut Project, scope: IdScope) -> Vec<String> {
    let mut result = CleanResult::default();
    let mut modified = HashSet::new();

    for (track_id, track) in &mut project.tracks {
        let before_ids = result.ids_assigned.len();
        let before_dates = result.dates_assigned.len();

        let prefix = project.config.ids.prefixes.get(track_id.as_str()).cloned();

        if let (Some(pfx), IdScope::Mint(ns)) = (&prefix, &scope) {
            let mint = Mint::new(&project.frame_dir, track_id, pfx, ns.as_ref());
            assign_missing_ids(track, track_id, mint, &mut result);
        }
        assign_missing_dates(track, track_id, &mut result);

        if result.ids_assigned.len() > before_ids || result.dates_assigned.len() > before_dates {
            modified.insert(track_id.clone());
        }
    }

    // Resolve duplicate IDs (cross-track and within-track) — minting, so only
    // when this clone owns a namespace.
    if let IdScope::Mint(ns) = &scope {
        let before_dups = result.duplicates_resolved.len();
        resolve_duplicate_ids(project, ns.as_ref(), &mut result);
        for dup in &result.duplicates_resolved[before_dups..] {
            modified.insert(dup.track_id.clone());
        }
    }

    // Reconcile misplaced tasks (e.g., parked task in Backlog section) — no
    // minting, so it runs regardless of claim state.
    for (track_id, track) in &mut project.tracks {
        if reconcile_sections_for_track(track, track_id, &mut result) {
            modified.insert(track_id.clone());
        }
    }

    modified.into_iter().collect()
}

// ---------------------------------------------------------------------------
// Section reconciliation — move misplaced top-level tasks to correct section
// ---------------------------------------------------------------------------

use crate::ops::task_ops::canonical_section;

/// Move top-level tasks that are in the wrong section to the correct one.
/// For example, a `[~]` parked task sitting in `## Backlog` gets moved to `## Parked`.
/// Returns true if any tasks were moved (i.e., the track was modified).
fn reconcile_sections_for_track(
    track: &mut Track,
    track_id: &str,
    result: &mut CleanResult,
) -> bool {
    // Collect (task_id, current_section, target_section) for misplaced tasks.
    // We iterate sections in order, checking only top-level tasks.
    let mut moves: Vec<(String, SectionKind, SectionKind)> = Vec::new();

    for node in &track.nodes {
        if let TrackNode::Section { kind, tasks, .. } = node {
            for task in tasks {
                let target = canonical_section(task.state);
                if target != *kind
                    && let Some(ref id) = task.id
                {
                    moves.push((id.to_string(), *kind, target));
                }
            }
        }
    }

    if moves.is_empty() {
        return false;
    }

    for (task_id, from, to) in &moves {
        crate::ops::task_ops::move_task_between_sections(track, task_id, *from, *to);
        result.sections_reconciled.push(SectionReconcile {
            track_id: track_id.to_string(),
            task_id: task_id.clone(),
            from: *from,
            to: *to,
        });
    }

    true
}

/// Reconcile sections across all tracks in a project.
/// Returns the list of track IDs that were modified.
pub fn reconcile_sections(project: &mut Project) -> Vec<String> {
    let mut result = CleanResult::default();
    let mut modified = Vec::new();

    for (track_id, track) in &mut project.tracks {
        if reconcile_sections_for_track(track, track_id, &mut result) {
            modified.push(track_id.clone());
        }
    }

    modified
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
///    3b. Reconcile sections (move misplaced tasks to correct section by state)
/// 4. Validate deps (flag dangling)
/// 5. Validate file refs (flag broken paths)
/// 6. State suggestions (all subtasks done → suggest parent done)
/// 7. Archive done tasks past threshold
///
/// Returns a report of all changes made and issues found.
///
/// `scope` honors the strict null policy: with [`IdScope::Mint`] any IDs minted
/// (newly assigned or reassigned duplicates) are scoped to that namespace
/// (`None` = null); with [`IdScope::Unclaimed`] those minting steps are skipped
/// so the clone never mints null IDs it doesn't own. Archival and thresholds
/// key on task state and `resolved:` dates, not ID structure, so they run
/// identically regardless of `scope`.
pub fn clean_project(project: &mut Project, scope: IdScope) -> CleanResult {
    let mut result = CleanResult::default();

    for (track_id, track) in &mut project.tracks {
        let prefix = project.config.ids.prefixes.get(track_id.as_str()).cloned();

        // 1. Assign missing IDs (minting — only when this clone owns a namespace)
        if let (Some(pfx), IdScope::Mint(ns)) = (&prefix, &scope) {
            let mint = Mint::new(&project.frame_dir, track_id, pfx, ns.as_ref());
            assign_missing_ids(track, track_id, mint, &mut result);
        }

        // 2. Assign missing added dates
        assign_missing_dates(track, track_id, &mut result);
    }

    // 3. Duplicate ID resolution (minting — only when this clone owns a namespace)
    if let IdScope::Mint(ns) = &scope {
        resolve_duplicate_ids(project, ns.as_ref(), &mut result);
    }

    // 3b. Reconcile misplaced tasks (e.g., parked task in Backlog section)
    for (track_id, track) in &mut project.tracks {
        reconcile_sections_for_track(track, track_id, &mut result);
    }

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

    // 8. Fill missing resolved dates. After archival by design — see
    //    `assign_missing_resolved_dates`.
    for (track_id, track) in &mut project.tracks {
        assign_missing_resolved_dates(track, track_id, &mut result);
    }

    result
}

// ---------------------------------------------------------------------------
// 1. Assign missing IDs
// ---------------------------------------------------------------------------

fn assign_missing_ids(track: &mut Track, track_id: &str, mint: Mint<'_>, result: &mut CleanResult) {
    // Reserve exactly as many numbers as there are ID-less top-level tasks, in
    // one reservation, then walk them out in order. Subtasks are numbered
    // relative to their parent, so they need nothing reserved — but they are
    // still assigned when no top-level task is missing an ID.
    let needed = count_missing_top_level_ids(track);
    let mut max = if needed > 0 {
        (mint.next_n(track, needed) - 1) as usize
    } else {
        0
    };

    let (prefix, token) = (mint.prefix(), mint.token());
    for node in &mut track.nodes {
        if let TrackNode::Section { tasks, .. } = node {
            assign_ids_in_tasks(tasks, track_id, prefix, token, &mut max, result);
        }
    }
}

/// How many top-level tasks are missing an ID — the size of the block
/// [`assign_ids_in_tasks`] is about to consume. Subtasks are numbered relative to
/// their parent, so they need nothing from the track's frontier.
fn count_missing_top_level_ids(track: &Track) -> u32 {
    let mut n = 0;
    for node in &track.nodes {
        if let TrackNode::Section { tasks, .. } = node {
            n += tasks.iter().filter(|t| t.id.is_none()).count() as u32;
        }
    }
    n
}

fn assign_ids_in_tasks(
    tasks: &mut [Task],
    track_id: &str,
    prefix: &str,
    token: Option<&Token>,
    max: &mut usize,
    result: &mut CleanResult,
) {
    for task in tasks.iter_mut() {
        if task.id.is_none() {
            *max += 1;
            let new_id = TaskId::with_number(prefix, *max as u32, token);
            task.id = Some(new_id.clone());
            task.mark_dirty();
            result.ids_assigned.push(IdAssignment {
                track_id: track_id.to_string(),
                assigned_id: new_id.to_string(),
                title: task.title.clone(),
            });
        }
        // Recurse into subtasks — subtasks with no ID also get assigned
        // (subtask IDs are parent_id.N)
        assign_subtask_ids(task, track_id, token, result);
    }
}

fn assign_subtask_ids(
    parent: &mut Task,
    track_id: &str,
    token: Option<&Token>,
    result: &mut CleanResult,
) {
    let parent_id = match &parent.id {
        Some(id) => id.clone(),
        None => return, // Parent must have an ID first
    };

    // Find the max existing child number in this namespace to avoid collisions
    // after deletions. E.g., if subtasks are [.1, .2, .4] (after deleting .3),
    // next should be .5 not .4.
    let mut max_num: u32 = 0;
    for sub in parent.subtasks.iter() {
        if let Some(n) = sub
            .id
            .as_ref()
            .and_then(|id| id.child_number_of(&parent_id, token))
        {
            max_num = max_num.max(n);
        }
    }

    for sub in parent.subtasks.iter_mut() {
        if sub.id.is_none() {
            max_num += 1;
            let sub_id = TaskId::child_of(&parent_id, max_num, token);
            sub.id = Some(sub_id.clone());
            sub.mark_dirty();
            result.ids_assigned.push(IdAssignment {
                track_id: track_id.to_string(),
                assigned_id: sub_id.to_string(),
                title: sub.title.clone(),
            });
        }
        // Recurse deeper
        assign_subtask_ids(sub, track_id, token, result);
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
                task_id: task.id.as_ref().map(|i| i.to_string()).unwrap_or_default(),
                date: today.to_string(),
                kind: DateKind::Added,
            });
        }

        assign_dates_in_tasks(&mut task.subtasks, track_id, today, result);
    }
}

// ---------------------------------------------------------------------------
// 2b. Assign missing resolved dates
// ---------------------------------------------------------------------------

/// Fill `resolved:` on done tasks that lack it, matching the condition
/// `fr check` warns on and the position `fr state <id> done` writes it in (last).
///
/// Reachable by ticking a checkbox in the editor, which is a supported workflow,
/// so it belongs in clean rather than behind `fr check --fix`: a user wants it
/// filled silently, exactly as `added:` already is.
///
/// **Runs after archival, and the order is load-bearing.** Archive retention
/// ranks done tasks by `resolved:`, treating a missing date as oldest — so a task
/// with no date is archived first. Filling the date earlier in the run would
/// stamp it with today, making the oldest task look like the newest completion:
/// it would be retained over genuinely recent work and surface at the top of
/// `fr recent`. Filling afterwards leaves that ranking exactly as it was.
fn assign_missing_resolved_dates(track: &mut Track, track_id: &str, result: &mut CleanResult) {
    let today = today_str();
    for node in &mut track.nodes {
        if let TrackNode::Section { tasks, .. } = node {
            assign_resolved_in_tasks(tasks, track_id, &today, result);
        }
    }
}

fn assign_resolved_in_tasks(
    tasks: &mut [Task],
    track_id: &str,
    today: &str,
    result: &mut CleanResult,
) {
    for task in tasks.iter_mut() {
        if task.state == TaskState::Done
            && !task
                .metadata
                .iter()
                .any(|m| matches!(m, Metadata::Resolved(_)))
        {
            task.metadata.push(Metadata::Resolved(today.to_string()));
            task.mark_dirty();
            result.dates_assigned.push(DateAssignment {
                track_id: track_id.to_string(),
                task_id: task.id.as_ref().map(|i| i.to_string()).unwrap_or_default(),
                date: today.to_string(),
                kind: DateKind::Resolved,
            });
        }
        assign_resolved_in_tasks(&mut task.subtasks, track_id, today, result);
    }
}

// ---------------------------------------------------------------------------
// 3. Duplicate ID resolution
// ---------------------------------------------------------------------------

/// A duplicate occurrence awaiting a fresh ID.
struct Duplicate {
    old_id: String,
    track_id: String,
    /// The ID of the task this one is nested under, or `None` at top level.
    /// A subtask's replacement ID has to extend its parent's, so this decides
    /// which allocator the replacement comes from.
    parent_id: Option<String>,
}

/// Find and resolve duplicate IDs across the project.
///
/// The first occurrence by track order (as listed in `project.toml`) then by
/// position within the track keeps its ID. Subsequent duplicates are reassigned
/// new IDs via the standard `max + 1` rule. Dependencies pointing to the
/// reassigned ID are updated across all tracks.
///
/// A duplicate that is a **subtask** is renumbered under its own parent, not
/// given a top-level number. Both allocators are `max + 1`, but they number
/// different things: minting `BAC-207` for a task nested under `BAC-153` resolves
/// the collision while breaking the rule that a subtask's ID extends its
/// parent's, leaving damage `fr check` reports as `ChildIdNotUnderParent`.
fn resolve_duplicate_ids(project: &mut Project, token: Option<&Token>, result: &mut CleanResult) {
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
    let mut duplicates: Vec<Duplicate> = Vec::new();

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
                        None,
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

    // Child numbers already handed out in this batch, keyed by parent ID. Like
    // `staged` below, these aren't in the track yet, so two duplicates under one
    // parent would otherwise both be offered the same number.
    let mut staged_children: HashMap<String, u32> = HashMap::new();

    for dup in &duplicates {
        let Duplicate {
            old_id,
            track_id: dup_track_id,
            parent_id,
        } = dup;
        let prefix = project
            .config
            .ids
            .prefixes
            .get(dup_track_id.as_str())
            .cloned();
        let Some(pfx) = prefix else { continue };

        // Find the track the duplicate lives in
        let track = project
            .tracks
            .iter()
            .find(|(tid, _)| tid == dup_track_id)
            .map(|(_, t)| t);
        let Some(track) = track else { continue };

        // A nested duplicate is renumbered under its own parent. Falls through to
        // the top-level mint if the parent has gone missing or its ID doesn't
        // match the grammar, where there is no child number to hand out.
        let new_id = match parent_id
            .as_deref()
            .and_then(|pid| next_child_id_under(track, pid, token, &mut staged_children))
        {
            Some(child_id) => child_id,
            None => {
                // Reassignments already computed in this batch aren't in the
                // track yet, so they have to be floored in explicitly.
                let staged = reassignments
                    .values()
                    .flatten()
                    .filter_map(|new_id| TaskId::parse(new_id).top_level_number(&pfx, token))
                    .max()
                    .unwrap_or(0);

                let mint = Mint::new(&project.frame_dir, dup_track_id, &pfx, token);
                TaskId::with_number(&pfx, mint.next_above(track, staged), token).to_string()
            }
        };
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
                        token,
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

/// The next free child number under `parent_id`, rendered as a full child ID.
///
/// `None` when the parent is gone or its own ID doesn't match the grammar —
/// there is no child number to extend in either case, and the caller falls back
/// to a top-level mint rather than inventing one.
///
/// `staged` carries the numbers already handed out under each parent in this
/// batch, which the track does not show yet.
fn next_child_id_under(
    track: &Track,
    parent_id: &str,
    token: Option<&Token>,
    staged: &mut HashMap<String, u32>,
) -> Option<String> {
    let parent = crate::ops::task_ops::find_task_in_track(track, parent_id)?;
    let parent_task_id = parent.id.as_ref().filter(|id| id.is_structured())?;

    let scanned = crate::ops::task_ops::next_child_number(parent, token) as u32;
    let slot = staged.entry(parent_id.to_string()).or_insert(0);
    let number = scanned.max(*slot + 1);
    *slot = number;
    Some(TaskId::child_of(parent_task_id, number, token).to_string())
}

fn find_duplicates_in_tasks(
    tasks: &[Task],
    track_id: &str,
    parent_id: Option<&str>,
    seen: &mut HashSet<String>,
    duplicates: &mut Vec<Duplicate>,
) {
    for task in tasks {
        if task
            .id
            .as_ref()
            .is_some_and(|id| !seen.insert(id.to_string()))
        {
            let id = task.id.as_ref().unwrap();
            duplicates.push(Duplicate {
                old_id: id.to_string(),
                track_id: track_id.to_string(),
                parent_id: parent_id.map(str::to_string),
            });
        }
        find_duplicates_in_tasks(
            &task.subtasks,
            track_id,
            task.id.as_deref(),
            seen,
            duplicates,
        );
    }
}

/// Walk tasks in order, applying reassignments to duplicate instances.
/// The first time we see an ID, it's the keeper (skip). Second+ times, reassign.
#[allow(clippy::too_many_arguments)]
fn apply_duplicate_reassignments(
    tasks: &mut [Task],
    track_id: &str,
    token: Option<&Token>,
    reassignments: &HashMap<String, Vec<String>>,
    cursors: &mut HashMap<String, usize>,
    seen: &mut HashSet<String>,
    result: &mut CleanResult,
) {
    for task in tasks.iter_mut() {
        let dup_old: Option<String> = task
            .id
            .as_ref()
            .map(|id| id.to_string())
            .filter(|id| reassignments.contains_key(id) && !seen.insert(id.clone()));
        if let Some(old_id) = dup_old {
            // This is a duplicate occurrence — reassign
            let cursor = cursors.entry(old_id.clone()).or_insert(0);
            if let Some(new_id) = reassignments.get(&old_id).and_then(|ids| ids.get(*cursor)) {
                task.id = Some(TaskId::parse(new_id));
                task.mark_dirty();
                renumber_subtasks(task, new_id, token);
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
            token,
            reassignments,
            cursors,
            seen,
            result,
        );
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
                        if !refs_ops::exists(project_root, r) {
                            result.broken_refs.push(BrokenRef {
                                track_id: track_id.to_string(),
                                task_id: task_id.to_string(),
                                path: r.clone(),
                                kind: RefKind::Ref,
                            });
                        }
                    }
                }
                Metadata::Spec(specs) => {
                    for spec in specs {
                        if !refs_ops::exists(project_root, spec) {
                            result.broken_refs.push(BrokenRef {
                                track_id: track_id.to_string(),
                                task_id: task_id.to_string(),
                                path: spec.clone(),
                                kind: RefKind::Spec,
                            });
                        }
                    }
                }
                _ => {}
            }
        }
        validate_refs_in_tasks(&task.subtasks, track_id, project_root, result);
    }
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
                task_id: task.id.as_ref().map(|i| i.to_string()).unwrap_or_default(),
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
    let retain = project.config.clean.done_retain;

    for (track_id, track) in &mut project.tracks {
        let done_tasks = track.section_tasks(SectionKind::Done);
        let done_task_count = done_tasks.len();
        if done_task_count <= threshold {
            continue;
        }

        // If we'd retain everything, skip archiving entirely
        if retain >= done_task_count {
            continue;
        }

        // Build (index, resolved_date) pairs, sort by resolved date descending.
        // Tasks without a resolved date get "" so they sort as oldest.
        let mut indexed: Vec<(usize, String)> = done_tasks
            .iter()
            .enumerate()
            .map(|(i, task)| {
                let resolved = task
                    .metadata
                    .iter()
                    .find_map(|m| {
                        if let Metadata::Resolved(d) = m {
                            Some(d.clone())
                        } else {
                            None
                        }
                    })
                    .unwrap_or_default();
                (i, resolved)
            })
            .collect();
        indexed.sort_by(|a, b| b.1.cmp(&a.1)); // most recent first

        // The top `retain` entries stay; the rest get archived
        let retain_indices: HashSet<usize> = indexed.iter().take(retain).map(|(i, _)| *i).collect();

        let tasks_to_archive: Vec<&Task> = done_tasks
            .iter()
            .enumerate()
            .filter(|(i, _)| !retain_indices.contains(i))
            .map(|(_, t)| t)
            .collect();
        if tasks_to_archive.is_empty() {
            continue;
        }

        let archive_path = project
            .frame_dir
            .join("archive")
            .join(format!("{}.md", track_id));
        if let Some(parent) = archive_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let existing = std::fs::read_to_string(&archive_path).unwrap_or_default();

        // Appending is not idempotent, so a task the archive already holds must
        // not be written a second time. That state is reachable: the archive is
        // written *before* the track is updated (below), deliberately, so a task
        // is never lost if the second write doesn't land — but if it doesn't (a
        // crash, or a `git checkout`/`reset` reverting the track file), the task
        // stays in Done and the next clean would archive it again.
        let already_archived = archived_task_ids(&existing);
        let (fresh, duplicates): (Vec<&Task>, Vec<&Task>) =
            tasks_to_archive.iter().partition(|task| {
                task.id
                    .as_ref()
                    .is_none_or(|id| !already_archived.contains(id.as_str()))
            });

        // The live copy of an already-archived task is about to be dropped from
        // the track. It should be identical to the archived one, but if it was
        // edited after that first write those edits would vanish silently — so
        // preserve it where anything lost goes.
        for task in &duplicates {
            let id = task.id.as_ref().map(|i| i.to_string()).unwrap_or_default();
            crate::io::recovery::log_recovery(
                &project.frame_dir,
                crate::io::recovery::RecoveryEntry {
                    timestamp: chrono::Utc::now(),
                    category: crate::io::recovery::RecoveryCategory::Conflict,
                    description: format!(
                        "{} was already in archive/{}.md — live copy removed from the track, not appended again",
                        id, track_id
                    ),
                    fields: vec![
                        ("track".to_string(), track_id.clone()),
                        ("task".to_string(), id),
                    ],
                    body: crate::parse::serialize_tasks(&[(*task).clone()], 0).join("\n"),
                },
            );
        }

        // Nothing new to append (every task was already archived): skip the
        // write, but still extract below — leaving them in Done would make every
        // future clean retry the same no-op.
        if !fresh.is_empty() {
            // Appending used to be string concatenation onto the raw existing
            // text, which is how a CRLF archive ended up with LF blocks glued
            // under CRLF ones — a file with both, that no later reader could put
            // right. Going through the pair means the file is rebuilt with its
            // own line ending, and the new tasks land after the last archived
            // task rather than after anything written below them.
            let mut archive = if existing.is_empty() {
                Archive::new(track_id)
            } else {
                crate::parse::parse_archive(&existing)
            };
            archive
                .tasks
                .extend(fresh.iter().map(|task| (*task).clone()));
            let new_content = crate::parse::serialize_archive(&archive);

            // Write archive — if this fails, leave tasks in place
            if crate::io::recovery::atomic_write(&archive_path, new_content.as_bytes()).is_err() {
                eprintln!(
                    "warning: could not write archive for {}, skipping",
                    track_id
                );
                continue;
            }
        }

        // Only NOW extract non-retained tasks from the Done section
        let archived = extract_done_tasks_except(track, &retain_indices);
        for task in &archived {
            result.tasks_archived.push(ArchiveRecord {
                track_id: track_id.clone(),
                task_id: task.id.as_ref().map(|i| i.to_string()).unwrap_or_default(),
                title: task.title.clone(),
            });
        }
    }
}

/// The task IDs an archive file already holds, read straight from its task lines
/// (`- [x] \`ID\` …`) rather than parsed into tasks — this only needs to know
/// which IDs are present, and a raw scan can't be thrown off by note bodies or
/// hand-editing.
fn archived_task_ids(existing: &str) -> HashSet<String> {
    existing
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim_start();
            if !trimmed.starts_with("- [") {
                return None;
            }
            let (_, after) = trimmed.split_once('`')?;
            let (id, _) = after.split_once('`')?;
            (!id.is_empty()).then(|| id.to_string())
        })
        .collect()
}

/// Remove done tasks from the track EXCEPT those at the given indices.
/// Returns the removed tasks.
fn extract_done_tasks_except(track: &mut Track, retain_indices: &HashSet<usize>) -> Vec<Task> {
    for node in &mut track.nodes {
        if let TrackNode::Section {
            kind: SectionKind::Done,
            tasks,
            ..
        } = node
        {
            let mut archived = Vec::new();
            let mut retained = Vec::new();
            for (i, task) in std::mem::take(tasks).into_iter().enumerate() {
                if retain_indices.contains(&i) {
                    retained.push(task);
                } else {
                    archived.push(task);
                }
            }
            *tasks = retained;
            return archived;
        }
    }
    Vec::new()
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
            ids.insert(id.to_string());
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
    use indexmap::IndexMap;
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn make_config(prefixes: Vec<(&str, &str)>) -> ProjectConfig {
        let mut prefix_map = IndexMap::new();
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
            recovery: Default::default(),
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

        let result = clean_project(&mut project, IdScope::Mint(None));
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

        let result = clean_project(&mut project, IdScope::Mint(None));
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

        let result = clean_project(&mut project, IdScope::Mint(None));
        // Only the first subtask should get an ID assigned
        let sub_assignments: Vec<_> = result
            .ids_assigned
            .iter()
            .filter(|a| a.assigned_id.contains('.'))
            .collect();
        assert_eq!(sub_assignments.len(), 1);
        // Max existing child number is 2 (from M-001.2), so next is .3
        assert_eq!(sub_assignments[0].assigned_id, "M-001.3");
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

        let result = clean_project(&mut project, IdScope::Mint(None));
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
    fn test_assigns_missing_resolved_date() {
        let mut project = make_project(
            "\
# Main

## Done

- [x] `M-001` Has a resolved date
  - added: 2025-05-01
  - resolved: 2025-05-02
- [x] `M-002` Ticked done by hand, no resolved date
  - added: 2025-05-01
",
            vec![("main", "M")],
        );

        let result = clean_project(&mut project, IdScope::Mint(None));

        let resolved: Vec<_> = result
            .dates_assigned
            .iter()
            .filter(|d| d.kind == DateKind::Resolved)
            .collect();
        assert_eq!(resolved.len(), 1, "only the dateless done task");
        assert_eq!(resolved[0].task_id, "M-002");

        let done = project.tracks[0].1.done();
        assert!(
            done[1]
                .metadata
                .iter()
                .any(|m| matches!(m, Metadata::Resolved(_)))
        );
    }

    #[test]
    fn test_resolved_date_is_not_assigned_to_unfinished_tasks() {
        let mut project = make_project(
            "\
# Main

## Backlog

- [ ] `M-001` Todo
  - added: 2025-05-01
- [~] `M-002` Parked
  - added: 2025-05-01
",
            vec![("main", "M")],
        );

        let result = clean_project(&mut project, IdScope::Mint(None));

        assert!(
            !result
                .dates_assigned
                .iter()
                .any(|d| d.kind == DateKind::Resolved),
            "only done tasks get a resolved date"
        );
    }

    /// Filling `resolved:` must not disturb archive retention, which ranks done
    /// tasks by that date and treats a missing one as oldest. Stamping the date
    /// before archival would make the oldest task look like the newest
    /// completion — it would be retained over genuinely recent work. The fill
    /// therefore runs after archival; this pins the ordering.
    #[test]
    fn test_missing_resolved_date_still_archives_first() {
        let root = PathBuf::from("/tmp/test-resolved-order");
        let track = parse_track(
            "\
# Main

## Done

- [x] `M-001` Dateless — must archive first
  - added: 2025-01-01
- [x] `M-002` Older
  - added: 2025-01-02
  - resolved: 2025-05-01
- [x] `M-003` Newest
  - added: 2025-01-03
  - resolved: 2025-05-20
",
        );

        let mut config = make_config(vec![("main", "M")]);
        config.clean.done_threshold = 1;
        config.clean.done_retain = 2;

        let mut project = Project {
            root: root.clone(),
            frame_dir: root.join("frame"),
            config,
            tracks: vec![("main".to_string(), track)],
            inbox: None,
        };

        let result = clean_project(&mut project, IdScope::Mint(None));

        let archived: Vec<&str> = result
            .tasks_archived
            .iter()
            .map(|a| a.task_id.as_str())
            .collect();
        assert_eq!(
            archived,
            vec!["M-001"],
            "the dateless task ranks oldest and is archived, not stamped with today"
        );

        let retained: Vec<&str> = project.tracks[0]
            .1
            .done()
            .iter()
            .filter_map(|t| t.id.as_deref())
            .collect();
        assert_eq!(retained, vec!["M-002", "M-003"]);
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

        let result = clean_project(&mut project, IdScope::Mint(None));
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

        let result = clean_project(&mut project, IdScope::Mint(None));
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

        let result = clean_project(&mut project, IdScope::Mint(None));
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

        let result = clean_project(&mut project, IdScope::Mint(None));
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

        let result = clean_project(&mut project, IdScope::Mint(None));
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

        let result = clean_project(&mut project, IdScope::Mint(None));
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

        let result = clean_project(&mut project, IdScope::Mint(None));
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

        let result = clean_project(&mut project, IdScope::Mint(None));
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
        config.clean.done_retain = 0; // retain none so all 100 are archived

        let mut project = Project {
            root: root.to_path_buf(),
            frame_dir: root.join("frame"),
            config,
            tracks: vec![("main".to_string(), track)],
            inbox: None,
        };

        let result = clean_project(&mut project, IdScope::Mint(None));
        assert_eq!(result.tasks_archived.len(), 100);

        // Done section should now be empty
        let done = project.tracks[0].1.done();
        assert!(done.is_empty());

        // Archive file should exist
        let archive_path = root.join("frame/archive/main.md");
        assert!(archive_path.exists());
    }

    /// A task the archive already holds must not be appended twice.
    ///
    /// Reachable because the archive is written before the track is updated: if
    /// that second write is lost (crash, or a git revert of the track file), the
    /// task is still in Done and the next clean would archive it again. This is
    /// what produced a doubled archive in a real project.
    #[test]
    fn test_archive_does_not_duplicate_an_already_archived_task() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("frame/archive")).unwrap();

        // The state left behind by an interrupted clean: already in the archive,
        // still in Done.
        std::fs::write(
            root.join("frame/archive/main.md"),
            "# Archive \u{2014} main\n\n- [x] `M-001` First\n  - resolved: 2025-05-01\n",
        )
        .unwrap();

        let track = parse_track(
            "\
# Main

## Backlog

## Done

- [x] `M-001` First
  - added: 2025-01-01
  - resolved: 2025-05-01
- [x] `M-002` Second
  - added: 2025-01-02
  - resolved: 2025-05-02
",
        );

        let mut config = make_config(vec![("main", "M")]);
        config.clean.done_threshold = 1;
        config.clean.done_retain = 0;

        let mut project = Project {
            root: root.to_path_buf(),
            frame_dir: root.join("frame"),
            config,
            tracks: vec![("main".to_string(), track)],
            inbox: None,
        };

        clean_project(&mut project, IdScope::Mint(None));

        let archive = std::fs::read_to_string(root.join("frame/archive/main.md")).unwrap();
        assert_eq!(
            archive.matches("`M-001`").count(),
            1,
            "M-001 was appended twice:\n{archive}"
        );
        assert_eq!(
            archive.matches("`M-002`").count(),
            1,
            "M-002 should be archived once:\n{archive}"
        );
        // Both leave the track either way — leaving the duplicate in Done would
        // make every future clean retry it.
        assert!(project.tracks[0].1.done().is_empty());

        // The live copy of the skipped task is preserved where lost data goes.
        let log = std::fs::read_to_string(root.join("frame/.recovery.log")).unwrap();
        assert!(log.contains("M-001"), "recovery log should hold it:\n{log}");
        assert!(log.contains("already in archive/main.md"), "{log}");
    }

    /// Appending must not change the file's line ending, and must not write past
    /// content already at the bottom of it.
    ///
    /// The append used to be string concatenation onto the raw existing text, so
    /// a CRLF archive got LF blocks glued under CRLF ones — one file with both
    /// endings, which no reader can put right afterwards because `LineEnding` is
    /// per file. The same concatenation put new tasks after *everything*,
    /// including a note somebody left at the end, and the next rewrite of the
    /// file then dropped that note as unparseable content.
    #[test]
    fn test_archive_append_keeps_the_line_ending_and_the_tail() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("frame/archive")).unwrap();

        std::fs::write(
            root.join("frame/archive/main.md"),
            "# Archive \u{2014} main\r\n\r\n- [x] `M-001` First\r\n  - resolved: 2025-05-01\r\n\r\n<!-- 2025 notes -->\r\n",
        )
        .unwrap();

        let track = parse_track(
            "\
# Main

## Backlog

## Done

- [x] `M-002` Second
  - added: 2025-01-02
  - resolved: 2025-05-02
- [x] `M-003` Third
  - added: 2025-01-03
  - resolved: 2025-05-03
",
        );

        let mut config = make_config(vec![("main", "M")]);
        config.clean.done_threshold = 1;
        config.clean.done_retain = 0;

        let mut project = Project {
            root: root.to_path_buf(),
            frame_dir: root.join("frame"),
            config,
            tracks: vec![("main".to_string(), track)],
            inbox: None,
        };

        clean_project(&mut project, IdScope::Mint(None));

        let bytes = std::fs::read(root.join("frame/archive/main.md")).unwrap();
        let text = String::from_utf8(bytes.clone()).unwrap();
        assert!(text.contains("`M-002`"), "the append happened:\n{text}");
        assert_eq!(
            bytes.iter().filter(|b| **b == b'\n').count(),
            text.matches("\r\n").count(),
            "the file gained a bare LF — it now mixes endings:\n{text:?}"
        );
        assert!(
            text.contains("<!-- 2025 notes -->"),
            "content at the bottom was dropped:\n{text}"
        );
        assert!(
            text.find("`M-002`") < text.find("<!-- 2025 notes -->"),
            "the new task should land with the other tasks, above the tail:\n{text}"
        );
    }

    /// Every task already archived: nothing to append, but Done still drains.
    #[test]
    fn test_archive_all_duplicates_still_clears_done() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("frame/archive")).unwrap();
        let original = "# Archive \u{2014} main\n\n- [x] `M-001` First\n  - resolved: 2025-05-01\n";
        std::fs::write(root.join("frame/archive/main.md"), original).unwrap();

        let track = parse_track(
            "\
# Main

## Backlog

## Done

- [x] `M-001` First
  - added: 2025-01-01
  - resolved: 2025-05-01
",
        );

        let mut config = make_config(vec![("main", "M")]);
        config.clean.done_threshold = 0;
        config.clean.done_retain = 0;

        let mut project = Project {
            root: root.to_path_buf(),
            frame_dir: root.join("frame"),
            config,
            tracks: vec![("main".to_string(), track)],
            inbox: None,
        };

        clean_project(&mut project, IdScope::Mint(None));

        assert_eq!(
            std::fs::read_to_string(root.join("frame/archive/main.md")).unwrap(),
            original,
            "archive should be untouched when there is nothing new to append"
        );
        assert!(project.tracks[0].1.done().is_empty());
    }

    #[test]
    fn test_archived_task_ids_reads_task_lines_only() {
        let ids = archived_task_ids(
            "\
# Archive \u{2014} main

- [x] `M-001` First
  - note:
    A note mentioning `M-999` in prose, and a fake `- [x] `M-998`` line.
  - [x] `M-001.1` Subtask
- [x] `M-a7` Another namespace
",
        );
        assert!(ids.contains("M-001"));
        assert!(ids.contains("M-001.1"), "subtask lines count too");
        assert!(ids.contains("M-a7"));
        assert!(!ids.contains("M-999"), "prose is not a task line");
        assert_eq!(ids.len(), 3, "{ids:?}");
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

        let result = clean_project(&mut project, IdScope::Mint(None));
        assert!(result.tasks_archived.is_empty());
    }

    #[test]
    fn test_archive_threshold_counts_tasks_not_lines() {
        // 5 tasks with verbose metadata = many lines but only 5 tasks.
        // With threshold of 5, should NOT archive (5 <= 5).
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("frame/tracks")).unwrap();

        let src = "\
# Main

## Backlog

- [ ] `M-100` Active task

## Done

- [x] `M-001` Task one
  - added: 2025-01-01
  - resolved: 2025-05-01
  - note:
    A long multi-line note that spans
    several lines to inflate the line count
    well beyond what a simple task would use.
- [x] `M-002` Task two
  - added: 2025-01-02
  - resolved: 2025-05-02
  - note:
    Another verbose note here
    with multiple lines
- [x] `M-003` Task three
  - added: 2025-01-03
  - resolved: 2025-05-03
  - spec: doc/spec.md
  - ref: doc/ref1.md, doc/ref2.md
  - note: Short note
- [x] `M-004` Task four
  - added: 2025-01-04
  - resolved: 2025-05-04
- [x] `M-005` Task five
  - added: 2025-01-05
  - resolved: 2025-05-05
";

        let track = parse_track(src);

        let mut config = make_config(vec![("main", "M")]);
        config.clean.done_threshold = 5; // exactly 5 tasks

        let mut project = Project {
            root: root.to_path_buf(),
            frame_dir: root.join("frame"),
            config,
            tracks: vec![("main".to_string(), track)],
            inbox: None,
        };

        let result = clean_project(&mut project, IdScope::Mint(None));
        // 5 tasks <= threshold of 5, so nothing should be archived
        assert!(result.tasks_archived.is_empty());
        assert_eq!(project.tracks[0].1.done().len(), 5);
    }

    #[test]
    fn test_archive_triggers_above_task_threshold() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("frame/tracks")).unwrap();

        let src = "\
# Main

## Backlog

- [ ] `M-100` Active task

## Done

- [x] `M-001` Task one
  - added: 2025-01-01
  - resolved: 2025-05-01
- [x] `M-002` Task two
  - added: 2025-01-02
  - resolved: 2025-05-02
- [x] `M-003` Task three
  - added: 2025-01-03
  - resolved: 2025-05-03
";

        let track = parse_track(src);

        let mut config = make_config(vec![("main", "M")]);
        config.clean.done_threshold = 2; // 3 tasks > 2
        config.clean.done_retain = 0; // retain none so all 3 are archived

        let mut project = Project {
            root: root.to_path_buf(),
            frame_dir: root.join("frame"),
            config,
            tracks: vec![("main".to_string(), track)],
            inbox: None,
        };

        let result = clean_project(&mut project, IdScope::Mint(None));
        assert_eq!(result.tasks_archived.len(), 3);
        assert!(project.tracks[0].1.done().is_empty());
    }

    #[test]
    fn test_archive_retains_most_recent() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("frame/tracks")).unwrap();

        let src = "\
# Main

## Backlog

- [ ] `M-100` Active task

## Done

- [x] `M-001` Oldest task
  - added: 2025-01-01
  - resolved: 2025-05-01
- [x] `M-002` No resolved date
  - added: 2025-01-02
- [x] `M-003` Middle task
  - added: 2025-01-03
  - resolved: 2025-05-10
- [x] `M-004` Most recent
  - added: 2025-01-04
  - resolved: 2025-05-20
- [x] `M-005` Second most recent
  - added: 2025-01-05
  - resolved: 2025-05-15
";

        let track = parse_track(src);

        let mut config = make_config(vec![("main", "M")]);
        config.clean.done_threshold = 2; // 5 tasks > 2, triggers archive
        config.clean.done_retain = 2; // keep the 2 most recent

        let mut project = Project {
            root: root.to_path_buf(),
            frame_dir: root.join("frame"),
            config,
            tracks: vec![("main".to_string(), track)],
            inbox: None,
        };

        let result = clean_project(&mut project, IdScope::Mint(None));

        // 5 tasks - 2 retained = 3 archived
        assert_eq!(result.tasks_archived.len(), 3);

        // The 2 most recent (by resolved date) should remain
        let done = project.tracks[0].1.done();
        assert_eq!(done.len(), 2);
        let retained_ids: Vec<&str> = done.iter().filter_map(|t| t.id.as_deref()).collect();
        // M-004 (2025-05-20) and M-005 (2025-05-15) are most recent
        assert!(retained_ids.contains(&"M-004"));
        assert!(retained_ids.contains(&"M-005"));

        // The archived tasks should include M-001, M-002 (no date), and M-003
        let archived_ids: Vec<&str> = result
            .tasks_archived
            .iter()
            .map(|a| a.task_id.as_str())
            .collect();
        assert!(archived_ids.contains(&"M-001"));
        assert!(archived_ids.contains(&"M-002"));
        assert!(archived_ids.contains(&"M-003"));
    }

    #[test]
    fn test_archive_retain_exceeds_count() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("frame/tracks")).unwrap();

        let src = "\
# Main

## Backlog

- [ ] `M-100` Active task

## Done

- [x] `M-001` Task one
  - added: 2025-01-01
  - resolved: 2025-05-01
- [x] `M-002` Task two
  - added: 2025-01-02
  - resolved: 2025-05-02
- [x] `M-003` Task three
  - added: 2025-01-03
  - resolved: 2025-05-03
";

        let track = parse_track(src);

        let mut config = make_config(vec![("main", "M")]);
        config.clean.done_threshold = 2; // 3 > 2, would normally trigger
        config.clean.done_retain = 5; // but retain 5 > count of 3

        let mut project = Project {
            root: root.to_path_buf(),
            frame_dir: root.join("frame"),
            config,
            tracks: vec![("main".to_string(), track)],
            inbox: None,
        };

        let result = clean_project(&mut project, IdScope::Mint(None));

        // Nothing should be archived since retain >= count
        assert!(result.tasks_archived.is_empty());
        assert_eq!(project.tracks[0].1.done().len(), 3);

        // No archive file should have been created
        let archive_path = root.join("frame/archive/main.md");
        assert!(!archive_path.exists());
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

        let result = clean_project(&mut project, IdScope::Mint(None));

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

        let result = clean_project(&mut project, IdScope::Mint(None));

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

        let result = clean_project(&mut project, IdScope::Mint(None));

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

        let result = clean_project(&mut project, IdScope::Mint(None));

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

    /// The collision two worktrees of one clone can still produce: both add a
    /// subtask to the same parent, both mint `.4`, the merge keeps both.
    ///
    /// Resolution has to come from the *parent's* child numbering. Minting a
    /// top-level `M-002` here would make the ID unique while breaking the rule
    /// that a subtask's ID extends its parent's.
    #[test]
    fn test_resolve_duplicate_subtask_renumbers_under_its_parent() {
        let mut project = make_project(
            "\
# Main

## Backlog

- [ ] `M-001` Parent
  - added: 2025-05-01
  - [ ] `M-001.4` Mine
    - added: 2025-05-01
  - [ ] `M-001.4` Theirs
    - added: 2025-05-01

## Done
",
            vec![("main", "M")],
        );

        let result = clean_project(&mut project, IdScope::Mint(None));

        assert_eq!(result.duplicates_resolved.len(), 1);
        assert_eq!(result.duplicates_resolved[0].original_id, "M-001.4");
        assert_eq!(result.duplicates_resolved[0].new_id, "M-001.5");

        let backlog = project.tracks[0].1.backlog();
        assert_eq!(backlog.len(), 1, "no task was promoted to top level");
        assert_eq!(backlog[0].subtasks[0].id.as_deref(), Some("M-001.4"));
        assert_eq!(backlog[0].subtasks[1].id.as_deref(), Some("M-001.5"));
    }

    /// Two collisions under one parent in a single pass: the second cannot be
    /// offered the number the first just took, which the track does not show yet.
    #[test]
    fn test_resolve_duplicate_subtasks_stage_within_one_parent() {
        let mut project = make_project(
            "\
# Main

## Backlog

- [ ] `M-001` Parent
  - added: 2025-05-01
  - [ ] `M-001.1` Original
    - added: 2025-05-01
  - [ ] `M-001.1` Copy one
    - added: 2025-05-01
  - [ ] `M-001.1` Copy two
    - added: 2025-05-01

## Done
",
            vec![("main", "M")],
        );

        clean_project(&mut project, IdScope::Mint(None));

        let subs = &project.tracks[0].1.backlog()[0].subtasks;
        let ids: Vec<_> = subs.iter().filter_map(|s| s.id.as_deref()).collect();
        assert_eq!(ids, vec!["M-001.1", "M-001.2", "M-001.3"]);
    }

    /// A duplicate nested two deep is renumbered under *its* parent, not the
    /// top-level task at the root of the branch.
    #[test]
    fn test_resolve_duplicate_grandchild_renumbers_under_its_own_parent() {
        let mut project = make_project(
            "\
# Main

## Backlog

- [ ] `M-001` Parent
  - added: 2025-05-01
  - [ ] `M-001.1` Child
    - added: 2025-05-01
    - [ ] `M-001.1.2` Grandchild
      - added: 2025-05-01
    - [ ] `M-001.1.2` Grandchild twin
      - added: 2025-05-01

## Done
",
            vec![("main", "M")],
        );

        clean_project(&mut project, IdScope::Mint(None));

        let grandkids = &project.tracks[0].1.backlog()[0].subtasks[0].subtasks;
        assert_eq!(grandkids[0].id.as_deref(), Some("M-001.1.2"));
        assert_eq!(grandkids[1].id.as_deref(), Some("M-001.1.3"));
    }

    /// A renumbered subtask carries its own descendants with it.
    #[test]
    fn test_resolve_duplicate_subtask_rekeys_descendants() {
        let mut project = make_project(
            "\
# Main

## Backlog

- [ ] `M-001` Parent
  - added: 2025-05-01
  - [ ] `M-001.1` Original
    - added: 2025-05-01
  - [ ] `M-001.1` Twin
    - added: 2025-05-01
    - [ ] `M-001.1.1` Twin's child
      - added: 2025-05-01

## Done
",
            vec![("main", "M")],
        );

        clean_project(&mut project, IdScope::Mint(None));

        let twin = &project.tracks[0].1.backlog()[0].subtasks[1];
        assert_eq!(twin.id.as_deref(), Some("M-001.2"));
        assert_eq!(twin.subtasks[0].id.as_deref(), Some("M-001.2.1"));
    }

    /// A duplicated subtask under a token-namespace clean is renumbered in that
    /// namespace, still under its parent.
    #[test]
    fn test_resolve_duplicate_subtask_in_token_namespace() {
        let mut project = make_project(
            "\
# Main

## Backlog

- [ ] `M-001` Parent
  - added: 2025-05-01
  - [ ] `M-001.1` Original
    - added: 2025-05-01
  - [ ] `M-001.1` Twin
    - added: 2025-05-01

## Done
",
            vec![("main", "M")],
        );

        let token = Token::new("b").unwrap();
        clean_project(&mut project, IdScope::Mint(Some(token)));

        let subs = &project.tracks[0].1.backlog()[0].subtasks;
        assert_eq!(subs[0].id.as_deref(), Some("M-001.1"));
        assert_eq!(subs[1].id.as_deref(), Some("M-001.b1"));
    }

    /// The resolved project is clean by `fr check`'s reckoning — no leftover
    /// duplicate, and no subtask whose id escaped its parent.
    #[test]
    fn test_resolve_duplicate_subtask_leaves_no_check_finding() {
        let mut project = make_project(
            "\
# Main

## Backlog

- [ ] `M-001` Parent
  - added: 2025-05-01
  - [ ] `M-001.4` Mine
    - added: 2025-05-01
  - [ ] `M-001.4` Theirs
    - added: 2025-05-01

## Done
",
            vec![("main", "M")],
        );

        clean_project(&mut project, IdScope::Mint(None));

        let check = crate::ops::check::check_project(&project);
        assert!(
            !check
                .errors
                .iter()
                .any(|e| matches!(e, crate::ops::check::CheckError::DuplicateId { .. })),
            "duplicate survived: {:?}",
            check.errors
        );
        assert!(
            !check.warnings.iter().any(|w| matches!(
                w,
                crate::ops::check::CheckWarning::ChildIdNotUnderParent { .. }
            )),
            "resolution misparented a subtask: {:?}",
            check.warnings
        );
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

        let result = clean_project(&mut project, IdScope::Mint(None));
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

        let result = clean_project(&mut project, IdScope::Mint(None));
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

        let result = clean_project(&mut project, IdScope::Mint(None));

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

        let modified = ensure_ids_and_dates(&mut project, IdScope::Mint(None));
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

        let modified = ensure_ids_and_dates(&mut project, IdScope::Mint(None));
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

        let modified = ensure_ids_and_dates(&mut project, IdScope::Mint(None));
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

        let modified = ensure_ids_and_dates(&mut project, IdScope::Mint(None));
        assert!(modified.contains(&"main".to_string()));

        let backlog = project.tracks[0].1.backlog();
        assert_eq!(backlog[0].id.as_deref(), Some("M-001"));
        assert_eq!(backlog[1].id.as_deref(), Some("M-002"));
    }

    // --- Section reconciliation ---

    #[test]
    fn test_reconcile_parked_task_in_backlog() {
        let mut project = make_project(
            "\
# Main

## Backlog

- [ ] `M-001` Normal task
  - added: 2025-05-01
- [~] `M-002` Should be in Parked
  - added: 2025-05-02

## Done
",
            vec![("main", "M")],
        );

        let result = clean_project(&mut project, IdScope::Mint(None));
        assert_eq!(result.sections_reconciled.len(), 1);
        assert_eq!(result.sections_reconciled[0].task_id, "M-002");
        assert_eq!(result.sections_reconciled[0].from, SectionKind::Backlog);
        assert_eq!(result.sections_reconciled[0].to, SectionKind::Parked);

        // Task should now be in Parked section
        assert_eq!(project.tracks[0].1.parked().len(), 1);
        assert_eq!(project.tracks[0].1.parked()[0].id.as_deref(), Some("M-002"));
        // And removed from Backlog
        assert_eq!(project.tracks[0].1.backlog().len(), 1);
    }

    #[test]
    fn test_reconcile_done_task_in_backlog() {
        let mut project = make_project(
            "\
# Main

## Backlog

- [x] `M-001` Done but stuck in Backlog
  - added: 2025-05-01
  - resolved: 2025-05-10

## Done
",
            vec![("main", "M")],
        );

        let result = clean_project(&mut project, IdScope::Mint(None));
        assert_eq!(result.sections_reconciled.len(), 1);
        assert_eq!(result.sections_reconciled[0].to, SectionKind::Done);

        assert_eq!(project.tracks[0].1.done().len(), 1);
        assert!(project.tracks[0].1.backlog().is_empty());
    }

    #[test]
    fn test_reconcile_unparked_task_in_parked() {
        let mut project = make_project(
            "\
# Main

## Backlog

## Parked

- [ ] `M-001` Unparked but stuck in Parked section
  - added: 2025-05-01

## Done
",
            vec![("main", "M")],
        );

        let result = clean_project(&mut project, IdScope::Mint(None));
        assert_eq!(result.sections_reconciled.len(), 1);
        assert_eq!(result.sections_reconciled[0].from, SectionKind::Parked);
        assert_eq!(result.sections_reconciled[0].to, SectionKind::Backlog);

        assert_eq!(project.tracks[0].1.backlog().len(), 1);
        assert!(project.tracks[0].1.parked().is_empty());
    }

    #[test]
    fn test_reconcile_no_changes_when_correct() {
        let mut project = make_project(
            "\
# Main

## Backlog

- [ ] `M-001` Normal task
  - added: 2025-05-01

## Parked

- [~] `M-002` Correctly parked
  - added: 2025-05-02

## Done

- [x] `M-003` Correctly done
  - added: 2025-05-03
  - resolved: 2025-05-10
",
            vec![("main", "M")],
        );

        let result = clean_project(&mut project, IdScope::Mint(None));
        assert!(result.sections_reconciled.is_empty());
    }

    #[test]
    fn test_reconcile_via_ensure_ids_and_dates() {
        let mut project = make_project(
            "\
# Main

## Backlog

- [~] `M-001` Parked in wrong section
  - added: 2025-05-01

## Done
",
            vec![("main", "M")],
        );

        let modified = ensure_ids_and_dates(&mut project, IdScope::Mint(None));
        assert!(modified.contains(&"main".to_string()));
        assert_eq!(project.tracks[0].1.parked().len(), 1);
        assert!(project.tracks[0].1.backlog().is_empty());
    }

    #[test]
    fn test_assign_subtask_ids_after_deletion() {
        // If subtask .3 was deleted from [.1, .2, .3, .4], and a new subtask
        // without an ID is added, it should get .5, not .4 (which already exists).
        let track = parse_track(
            "\
# Test

## Backlog

- [ ] `T-001` Parent
  - [ ] `T-001.1` Sub 1
  - [ ] `T-001.2` Sub 2
  - [ ] `T-001.4` Sub 4
  - [ ] New subtask without ID

## Done",
        );

        let config = make_config(vec![("main", "T")]);
        let root = TempDir::new().unwrap();
        let mut project = Project {
            config,
            root: root.path().to_path_buf(),
            frame_dir: root.path().join("frame"),
            tracks: vec![("main".to_string(), track)],
            inbox: None,
        };

        let modified = ensure_ids_and_dates(&mut project, IdScope::Mint(None));
        assert!(modified.contains(&"main".to_string()));

        // The new subtask should get .5 (not .4 which already exists)
        let parent =
            crate::ops::task_ops::find_task_in_track(&project.tracks[0].1, "T-001").unwrap();
        let new_sub = &parent.subtasks[3];
        assert_eq!(new_sub.id.as_deref(), Some("T-001.5"));
    }

    // --- Namespace-scoped minting (Phase 3) ---

    #[test]
    fn test_clean_assigns_missing_ids_in_token_namespace() {
        let token = Token::new("a").unwrap();
        let mut project = make_project(
            "\
# Main

## Backlog

- [ ] `M-001` Has null ID
- [ ] Missing ID task
  - [ ] Missing subtask ID

## Done
",
            vec![("main", "M")],
        );

        // Cleaning in actor `a`'s clone mints into the empty `a` namespace.
        let result = clean_project(&mut project, IdScope::Mint(Some(token.clone())));
        let assigned: Vec<&str> = result
            .ids_assigned
            .iter()
            .map(|a| a.assigned_id.as_str())
            .collect();
        assert_eq!(assigned, vec!["M-a1", "M-a1.a1"]);
        // The pre-existing null ID is untouched.
        assert_eq!(
            project.tracks[0].1.backlog()[0].id.as_deref(),
            Some("M-001")
        );
    }

    #[test]
    fn test_clean_resolves_duplicates_in_token_namespace() {
        let token = Token::new("a").unwrap();
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

        clean_project(&mut project, IdScope::Mint(Some(token.clone())));
        let backlog = project.tracks[0].1.backlog();
        // Keeper retains its null ID; the duplicate is reassigned in `a`'s namespace.
        assert_eq!(backlog[0].id.as_deref(), Some("M-001"));
        assert_eq!(backlog[1].id.as_deref(), Some("M-a1"));
    }

    #[test]
    fn test_clean_archival_unchanged_by_token() {
        // Archival keys on state + resolved date, not ID structure, so a tokened
        // clean archives exactly what a null clean does.
        let src = "\
# Main

## Backlog

- [ ] `M-100` Active task

## Done

- [x] `M-001` Task one
  - added: 2025-01-01
  - resolved: 2025-05-01
- [x] `M-002` Task two
  - added: 2025-01-02
  - resolved: 2025-05-02
- [x] `M-003` Task three
  - added: 2025-01-03
  - resolved: 2025-05-03
";
        let archived_count = |scope: IdScope| {
            let tmp = TempDir::new().unwrap();
            let root = tmp.path();
            std::fs::create_dir_all(root.join("frame/tracks")).unwrap();
            let mut config = make_config(vec![("main", "M")]);
            config.clean.done_threshold = 2;
            config.clean.done_retain = 0;
            let mut project = Project {
                root: root.to_path_buf(),
                frame_dir: root.join("frame"),
                config,
                tracks: vec![("main".to_string(), parse_track(src))],
                inbox: None,
            };
            clean_project(&mut project, scope).tasks_archived.len()
        };
        // Null creator, a tokened clone, and an unclaimed clone all archive the
        // same set — archival is independent of ID minting.
        assert_eq!(archived_count(IdScope::Mint(None)), 3);
        assert_eq!(archived_count(IdScope::Mint(Token::new("a"))), 3);
        assert_eq!(archived_count(IdScope::Unclaimed), 3);
    }

    // --- Strict null policy: passive paths on an unclaimed clone (Phase 3.x) ---

    fn project_with_idless_task() -> Project {
        make_project(
            "\
# Main

## Backlog

- [ ] `M-001` Has an ID
- [ ] Missing an ID

## Done
",
            vec![("main", "M")],
        )
    }

    #[test]
    fn test_unclaimed_passive_skips_id_assignment() {
        // An unclaimed clone must NOT mint null on a passive path: the ID-less
        // task stays ID-less.
        let mut project = project_with_idless_task();
        let modified = ensure_ids_and_dates(&mut project, IdScope::Unclaimed);
        let backlog = project.tracks[0].1.backlog();
        assert!(
            backlog[1].id.is_none(),
            "unclaimed clone must not mint an ID"
        );
        // The date-only normalization still ran (it mints nothing).
        assert!(
            backlog[1]
                .metadata
                .iter()
                .any(|m| matches!(m, Metadata::Added(_)))
        );
        assert!(modified.contains(&"main".to_string()));
    }

    #[test]
    fn test_null_creator_passive_mints_null() {
        // The `fr init` creator deliberately owns null, so it still mints null.
        let mut project = project_with_idless_task();
        ensure_ids_and_dates(&mut project, IdScope::Mint(None));
        assert_eq!(
            project.tracks[0].1.backlog()[1].id.as_deref(),
            Some("M-002")
        );
    }

    #[test]
    fn test_tokened_passive_mints_in_namespace() {
        let mut project = project_with_idless_task();
        ensure_ids_and_dates(&mut project, IdScope::Mint(Token::new("a")));
        assert_eq!(project.tracks[0].1.backlog()[1].id.as_deref(), Some("M-a1"));
    }

    #[test]
    fn test_unclaimed_clean_skips_minting_but_archives() {
        // `clean_project` on an unclaimed clone skips ID assignment and duplicate
        // resolution, but still archives done tasks.
        let mut project = project_with_idless_task();
        let result = clean_project(&mut project, IdScope::Unclaimed);
        assert!(result.ids_assigned.is_empty());
        assert!(project.tracks[0].1.backlog()[1].id.is_none());
    }

    /// The `fr clean` incident, at the level it was reported: a track the user
    /// never touched, one task missing a `resolved:` date, and a mis-indented
    /// prose line on a *different*, already-done task.
    ///
    /// Filling the date makes that one task dirty, which rewrites the file —
    /// and the rewrite used to drop the prose line, because the parser had
    /// consumed it without recording it. The damage arrived inside a large,
    /// boring clean diff, on a task and a track unrelated to the work in hand.
    #[test]
    fn test_clean_keeps_a_stray_line_on_an_untouched_task() {
        let source = "\
# Main

## Done

- [x] `M-001` Sharded map lowering
  - added: 2026-07-01
  - resolved: 2026-07-20
    **Shape.** A sharded map whose callback produces a per-row output.
- [x] `M-002` Needs a resolved date
  - added: 2026-07-02
";
        let mut project = make_project(source, vec![("main", "M")]);
        let result = clean_project(&mut project, IdScope::Mint(None));

        // The date fill is what triggered the rewrite.
        assert!(
            result
                .dates_assigned
                .iter()
                .any(|d| d.task_id == "M-002" && d.kind == DateKind::Resolved),
            "expected clean to fill M-002's resolved date: {:?}",
            result.dates_assigned
        );

        let written = crate::parse::serialize_track(&project.tracks[0].1);
        assert!(
            written.contains("**Shape.** A sharded map whose callback produces a per-row output."),
            "clean deleted a line from an untouched task: {written}"
        );
    }

    /// Archiving must carry a stranded line with it rather than leave it behind
    /// — the incident report checked the archive too, and the line was in
    /// neither place. It travels with the task that holds it, which is the task
    /// *below* it; when a whole Done section is archived together, as here, that
    /// keeps its position exactly.
    #[test]
    fn test_archive_carries_a_stranded_line() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("frame/tracks")).unwrap();

        let src = "\
# Main

## Backlog

- [ ] `M-200` Active task

## Done

- [x] `M-001` Sharded map lowering
  - added: 2026-07-01
  - resolved: 2026-07-20
    **Shape.** A sharded map whose callback produces a per-row output.
- [x] `M-002` Unrelated finished work
  - added: 2026-07-02
  - resolved: 2026-07-21
";

        let mut config = make_config(vec![("main", "M")]);
        config.clean.done_threshold = 1;
        config.clean.done_retain = 0;

        let mut project = Project {
            root: root.to_path_buf(),
            frame_dir: root.join("frame"),
            config,
            tracks: vec![("main".to_string(), parse_track(src))],
            inbox: None,
        };

        let result = clean_project(&mut project, IdScope::Mint(None));
        assert_eq!(result.tasks_archived.len(), 2);

        let archive = std::fs::read_to_string(root.join("frame/archive/main.md")).unwrap();
        assert!(
            archive.contains("**Shape.** A sharded map whose callback produces a per-row output."),
            "archiving dropped the stranded line: {archive}"
        );
        let track = crate::parse::serialize_track(&project.tracks[0].1);
        assert!(
            !track.contains("**Shape."),
            "the line was left behind in the track as well: {track}"
        );
    }

    // -----------------------------------------------------------------------
    // normalize_project
    // -----------------------------------------------------------------------

    /// The shape the whole thing exists for: `resolved:` appended after a note,
    /// where it reads as missing.
    #[test]
    fn normalize_reorders_a_task_whose_fields_are_out_of_order() {
        let mut project = make_project(
            "\
# Main

## Backlog

## Done

- [x] `M-001` Finished
  - added: 2026-01-01
  - note: some body
  - resolved: 2026-01-02
",
            vec![("main", "M")],
        );

        let result = normalize_project(&mut project);
        assert_eq!(result.reordered.len(), 1);
        assert_eq!(result.reordered[0].task, "M-001");
        assert_eq!(result.reordered[0].was, ["added", "note", "resolved"]);
        assert!(result.skipped.is_empty());

        let text = crate::parse::serialize_track(&project.tracks[0].1);
        let added = text.find("added:").unwrap();
        let resolved = text.find("resolved:").unwrap();
        let note = text.find("note:").unwrap();
        assert!(added < resolved && resolved < note, "{text}");
    }

    /// A task already in order is left **clean**, so it serializes verbatim and
    /// the pass produces no diff for it. This is what keeps the rewrite to the
    /// tasks the report names rather than every task in the project.
    #[test]
    fn normalize_leaves_an_ordered_task_byte_for_byte() {
        let src = "\
# Main

## Backlog

- [ ]  `M-001`   Odd   spacing kept
  - added: 2026-01-01
  - note: body

## Done
";
        let mut project = make_project(src, vec![("main", "M")]);

        let result = normalize_project(&mut project);
        assert!(result.reordered.is_empty(), "{:?}", result.reordered);
        assert_eq!(crate::parse::serialize_track(&project.tracks[0].1), src);
    }

    /// Running it twice changes nothing the second time.
    #[test]
    fn normalize_is_idempotent() {
        let mut project = make_project(
            "\
# Main

## Backlog

- [ ] `M-001` A task
  - added: 2026-01-01
  - note: body
  - ref: src/a.rs

## Done
",
            vec![("main", "M")],
        );

        assert_eq!(normalize_project(&mut project).reordered.len(), 1);
        let once = crate::parse::serialize_track(&project.tracks[0].1);

        let second = normalize_project(&mut project);
        assert!(second.reordered.is_empty(), "{:?}", second.reordered);
        assert_eq!(crate::parse::serialize_track(&project.tracks[0].1), once);
    }

    /// A task whose stranded lines a note would swallow is reported, not
    /// rewritten — and the report is what tells the reader the file has damage
    /// worth a look, rather than the pass skipping it in silence.
    #[test]
    fn normalize_reports_the_task_it_must_not_touch() {
        let mut project = make_project(
            "\
# Main

## Backlog

- [ ] `M-001` Has stranded lines
  - note:
  - added: 2026-01-01
    ```rust
    stranded
- [ ] `M-002` Next

## Done
",
            vec![("main", "M")],
        );

        let before = crate::parse::serialize_track(&project.tracks[0].1);
        let result = normalize_project(&mut project);

        assert_eq!(result.skipped.len(), 1, "{result:?}");
        assert_eq!(result.skipped[0].task, "M-001");
        assert!(result.reordered.is_empty(), "{:?}", result.reordered);
        assert_eq!(
            crate::parse::serialize_track(&project.tracks[0].1),
            before,
            "the task must be left exactly as it was"
        );
    }

    /// Subtasks are reached too, and reported under their own id.
    #[test]
    fn normalize_reaches_subtasks() {
        let mut project = make_project(
            "\
# Main

## Backlog

- [ ] `M-001` Parent
  - added: 2026-01-01
  - [x] `M-001.1` Child
    - added: 2026-01-01
    - note: body
    - resolved: 2026-01-02

## Done
",
            vec![("main", "M")],
        );

        let result = normalize_project(&mut project);
        assert_eq!(result.reordered.len(), 1);
        assert_eq!(result.reordered[0].task, "M-001.1");
    }
}
