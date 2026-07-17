use chrono::Local;

use crate::model::task::{Metadata, Task, TaskState};
use crate::model::task_id::{TaskId, Token};
use crate::model::track::{SectionKind, Track, TrackNode};
use crate::parse::parse_title_and_tags;

/// Error type for task operations
#[derive(Debug, thiserror::Error)]
pub enum TaskError {
    #[error("task not found: {0}")]
    NotFound(String),
    #[error("no ID prefix configured for track {0}")]
    NoPrefixForTrack(String),
    #[error("cannot add subtask: maximum nesting depth (3) reached")]
    MaxDepthReached,
    #[error("invalid position: {0}")]
    InvalidPosition(String),
    #[error("reparenting would create a cycle")]
    CycleDetected,
    #[error("task is already top-level")]
    AlreadyTopLevel,
    #[error("reparenting would exceed maximum nesting depth (3)")]
    DepthExceeded,
}

/// Location of a task in the track tree
#[derive(Debug, Clone)]
pub struct TaskLocation {
    pub section: SectionKind,
    pub parent_id: Option<String>,
    pub sibling_index: usize,
}

/// Result of a reparent operation (for undo)
#[derive(Debug, Clone)]
pub struct ReparentResult {
    pub new_root_id: String,
    pub id_mappings: Vec<(String, String)>, // old_id -> new_id
    pub old_location: TaskLocation,
}

// ---------------------------------------------------------------------------
// 2.1 — State transitions
// ---------------------------------------------------------------------------

/// Cycle state: todo → active → done → todo
pub fn cycle_state(task: &mut Task) {
    let new_state = match task.state {
        TaskState::Todo => TaskState::Active,
        TaskState::Active => TaskState::Done,
        TaskState::Done => TaskState::Todo,
        // Blocked/Parked cycle back to todo
        TaskState::Blocked => TaskState::Todo,
        TaskState::Parked => TaskState::Todo,
    };
    set_state(task, new_state);
}

/// Set blocked: any → blocked, blocked → todo
pub fn set_blocked(task: &mut Task) {
    if task.state == TaskState::Blocked {
        set_state(task, TaskState::Todo);
    } else {
        set_state(task, TaskState::Blocked);
    }
}

/// Set parked: any → parked, parked → todo
pub fn set_parked(task: &mut Task) {
    if task.state == TaskState::Parked {
        set_state(task, TaskState::Todo);
    } else {
        set_state(task, TaskState::Parked);
    }
}

/// Set done: any → done (adds resolved date)
pub fn set_done(task: &mut Task) {
    set_state(task, TaskState::Done);
}

/// Direct state set — handles resolved/added date bookkeeping
pub fn set_state(task: &mut Task, new_state: TaskState) {
    if task.state == new_state {
        return;
    }
    let was_done = task.state == TaskState::Done;
    task.state = new_state;
    task.mark_dirty();

    if new_state == TaskState::Done {
        let today = today_str();
        // Add resolved date (replace existing if present)
        remove_metadata(task, "resolved");
        task.metadata.push(Metadata::Resolved(today));
    } else if was_done {
        // Leaving done state — remove resolved date
        remove_metadata(task, "resolved");
    }
}

// ---------------------------------------------------------------------------
// 2.2 — Task CRUD
// ---------------------------------------------------------------------------

/// Where to insert a new task in a section
#[derive(Debug, Clone)]
pub enum InsertPosition {
    /// Append to end of section (lowest priority)
    Bottom,
    /// Prepend to start of section (highest priority)
    Top,
    /// Insert after the task with this ID
    After(String),
}

/// Add a task to a track's backlog section.
/// Returns the assigned ID.
pub fn add_task(
    track: &mut Track,
    title: String,
    position: InsertPosition,
    prefix: &str,
    token: Option<&Token>,
) -> Result<String, TaskError> {
    let next_num = next_id_number(track, prefix, token);
    let id = TaskId::with_number(prefix, next_num as u32, token);

    let (parsed_title, tags) = parse_title_and_tags(&title);
    let mut task = Task::new(TaskState::Todo, Some(id.clone()), parsed_title);
    task.tags = tags;
    task.metadata.push(Metadata::Added(today_str()));

    let tasks = track
        .section_tasks_mut(SectionKind::Backlog)
        .ok_or_else(|| TaskError::InvalidPosition("no backlog section".into()))?;

    insert_at(tasks, task, &position)?;
    Ok(id.to_string())
}

/// Add a subtask to an existing task identified by `parent_id`.
/// Returns the assigned subtask ID.
pub fn add_subtask(
    track: &mut Track,
    parent_id: &str,
    title: String,
    token: Option<&Token>,
) -> Result<String, TaskError> {
    let parent = find_task_mut_in_track(track, parent_id)
        .ok_or_else(|| TaskError::NotFound(parent_id.to_string()))?;

    if parent.depth >= 2 {
        return Err(TaskError::MaxDepthReached);
    }

    let sub_id = next_child_id(parent, parent_id, token);
    let (parsed_title, tags) = parse_title_and_tags(&title);
    let mut subtask = Task::new(TaskState::Todo, Some(sub_id.clone()), parsed_title);
    subtask.tags = tags;
    subtask.depth = parent.depth + 1;
    subtask.metadata.push(Metadata::Added(today_str()));
    parent.subtasks.push(subtask);
    parent.mark_dirty();

    Ok(sub_id.to_string())
}

/// Add a subtask to an existing task, inserted after a specific sibling.
/// Returns the assigned subtask ID.
pub fn add_subtask_after(
    track: &mut Track,
    parent_id: &str,
    after_sibling_id: &str,
    title: String,
    token: Option<&Token>,
) -> Result<String, TaskError> {
    let parent = find_task_mut_in_track(track, parent_id)
        .ok_or_else(|| TaskError::NotFound(parent_id.to_string()))?;

    if parent.depth >= 2 {
        return Err(TaskError::MaxDepthReached);
    }

    let sub_id = next_child_id(parent, parent_id, token);
    let (parsed_title, tags) = parse_title_and_tags(&title);
    let mut subtask = Task::new(TaskState::Todo, Some(sub_id.clone()), parsed_title);
    subtask.tags = tags;
    subtask.depth = parent.depth + 1;
    subtask.metadata.push(Metadata::Added(today_str()));

    let insert_idx = parent
        .subtasks
        .iter()
        .position(|t| t.id.as_deref() == Some(after_sibling_id))
        .map(|i| i + 1)
        .unwrap_or(parent.subtasks.len());
    parent.subtasks.insert(insert_idx, subtask);
    parent.mark_dirty();

    Ok(sub_id.to_string())
}

/// Edit a task's title.
pub fn edit_title(track: &mut Track, task_id: &str, new_title: String) -> Result<(), TaskError> {
    let (parsed_title, new_tags) = parse_title_and_tags(&new_title);
    let task = find_task_mut_in_track(track, task_id)
        .ok_or_else(|| TaskError::NotFound(task_id.to_string()))?;
    task.title = parsed_title;
    for tag in new_tags {
        if !task.tags.contains(&tag) {
            task.tags.push(tag);
        }
    }
    task.mark_dirty();
    Ok(())
}

/// "Delete" a task by marking it done and adding #wontdo tag.
pub fn delete_task(track: &mut Track, task_id: &str) -> Result<(), TaskError> {
    let task = find_task_mut_in_track(track, task_id)
        .ok_or_else(|| TaskError::NotFound(task_id.to_string()))?;
    set_done(task);
    if !task.tags.contains(&"wontdo".to_string()) {
        task.tags.push("wontdo".to_string());
    }
    task.mark_dirty();
    Ok(())
}

// ---------------------------------------------------------------------------
// 2.3 — Metadata operations
// ---------------------------------------------------------------------------

pub fn add_tag(track: &mut Track, task_id: &str, tag: &str) -> Result<(), TaskError> {
    let task = find_task_mut_in_track(track, task_id)
        .ok_or_else(|| TaskError::NotFound(task_id.to_string()))?;
    let tag = tag.trim_start_matches('#').to_string();
    if !task.tags.contains(&tag) {
        task.tags.push(tag);
        task.mark_dirty();
    }
    Ok(())
}

pub fn remove_tag(track: &mut Track, task_id: &str, tag: &str) -> Result<(), TaskError> {
    let task = find_task_mut_in_track(track, task_id)
        .ok_or_else(|| TaskError::NotFound(task_id.to_string()))?;
    let tag = tag.trim_start_matches('#');
    let before_len = task.tags.len();
    task.tags.retain(|t| t != tag);
    if task.tags.len() != before_len {
        task.mark_dirty();
    }
    Ok(())
}

/// Add a dependency. `dep_id` is validated to exist somewhere in the provided tracks.
pub fn add_dep(
    track: &mut Track,
    task_id: &str,
    dep_id: &str,
    all_tracks: &[(String, Track)],
) -> Result<(), TaskError> {
    // Validate the dep target exists
    if !task_id_exists_in_tracks(dep_id, all_tracks) {
        return Err(TaskError::NotFound(format!("dep target {}", dep_id)));
    }

    let task = find_task_mut_in_track(track, task_id)
        .ok_or_else(|| TaskError::NotFound(task_id.to_string()))?;

    // Find existing Dep metadata or create one
    if let Some(Metadata::Dep(deps)) = task.metadata.iter_mut().find(|m| m.key() == "dep") {
        if !deps.contains(&dep_id.to_string()) {
            deps.push(dep_id.to_string());
            task.mark_dirty();
        }
    } else {
        task.metadata.push(Metadata::Dep(vec![dep_id.to_string()]));
        task.mark_dirty();
    }
    Ok(())
}

pub fn remove_dep(track: &mut Track, task_id: &str, dep_id: &str) -> Result<(), TaskError> {
    let task = find_task_mut_in_track(track, task_id)
        .ok_or_else(|| TaskError::NotFound(task_id.to_string()))?;

    let mut changed = false;
    for m in &mut task.metadata {
        if let Metadata::Dep(deps) = m {
            let before = deps.len();
            deps.retain(|d| d != dep_id);
            if deps.len() != before {
                changed = true;
            }
        }
    }
    // Remove empty Dep entries
    task.metadata
        .retain(|m| !matches!(m, Metadata::Dep(d) if d.is_empty()));

    if changed {
        task.mark_dirty();
    }
    Ok(())
}

pub fn set_note(track: &mut Track, task_id: &str, note_text: String) -> Result<(), TaskError> {
    let task = find_task_mut_in_track(track, task_id)
        .ok_or_else(|| TaskError::NotFound(task_id.to_string()))?;
    remove_metadata(task, "note");
    if !note_text.is_empty() {
        task.metadata.push(Metadata::Note(note_text));
    }
    task.mark_dirty();
    Ok(())
}

pub fn append_note(track: &mut Track, task_id: &str, note_text: String) -> Result<(), TaskError> {
    let task = find_task_mut_in_track(track, task_id)
        .ok_or_else(|| TaskError::NotFound(task_id.to_string()))?;
    let existing = task.metadata.iter().find_map(|m| match m {
        Metadata::Note(n) => Some(n.clone()),
        _ => None,
    });
    let new_note = match existing {
        Some(old) if !old.is_empty() => format!("{}\n\n{}", old, note_text),
        _ => note_text,
    };
    remove_metadata(task, "note");
    if !new_note.is_empty() {
        task.metadata.push(Metadata::Note(new_note));
    }
    task.mark_dirty();
    Ok(())
}

pub fn add_ref(track: &mut Track, task_id: &str, path: &str) -> Result<(), TaskError> {
    let task = find_task_mut_in_track(track, task_id)
        .ok_or_else(|| TaskError::NotFound(task_id.to_string()))?;

    if let Some(Metadata::Ref(refs)) = task.metadata.iter_mut().find(|m| m.key() == "ref") {
        if !refs.contains(&path.to_string()) {
            refs.push(path.to_string());
            task.mark_dirty();
        }
    } else {
        task.metadata.push(Metadata::Ref(vec![path.to_string()]));
        task.mark_dirty();
    }
    Ok(())
}

pub fn set_spec(track: &mut Track, task_id: &str, spec: String) -> Result<(), TaskError> {
    let task = find_task_mut_in_track(track, task_id)
        .ok_or_else(|| TaskError::NotFound(task_id.to_string()))?;
    remove_metadata(task, "spec");
    task.metadata.push(Metadata::Spec(spec));
    task.mark_dirty();
    Ok(())
}

// ---------------------------------------------------------------------------
// 2.4 — Move operations
// ---------------------------------------------------------------------------

/// Move a task within the same track's backlog (reorder).
pub fn move_task(
    track: &mut Track,
    task_id: &str,
    position: InsertPosition,
) -> Result<(), TaskError> {
    // Reorder within whatever section holds the task at top level, not just the
    // Backlog — a Parked or Done task can be repositioned in place too.
    let section = top_level_section(track, task_id)
        .ok_or_else(|| TaskError::NotFound(task_id.to_string()))?;
    let tasks = track
        .section_tasks_mut(section)
        .ok_or_else(|| TaskError::InvalidPosition("no such section".into()))?;

    let idx = tasks
        .iter()
        .position(|t| t.id.as_deref() == Some(task_id))
        .ok_or_else(|| TaskError::NotFound(task_id.to_string()))?;

    let task = tasks.remove(idx);
    insert_at(tasks, task, &position)?;
    Ok(())
}

/// The section that holds `task_id` as a top-level task, if any. Cross-track and
/// reorder moves operate on whole top-level subtrees, so a completed task in the
/// Done section (or a Parked one) is located here — not just the Backlog.
fn top_level_section(track: &Track, task_id: &str) -> Option<SectionKind> {
    [SectionKind::Backlog, SectionKind::Parked, SectionKind::Done]
        .into_iter()
        .find(|&s| is_top_level_in_section(track, task_id, s))
}

/// Move a task to a different track. Reassigns the task ID using the target
/// track's prefix. Updates dependency references across all provided tracks.
///
/// The task is located in whichever section holds it at top level (Backlog,
/// Parked, or Done) and is inserted into the *same* section in the target, so a
/// Done task keeps its completed state instead of silently reopening.
pub fn move_task_to_track(
    source_track: &mut Track,
    target_track: &mut Track,
    task_id: &str,
    position: InsertPosition,
    target_prefix: &str,
    all_tracks_for_dep_update: &mut [(String, Track)],
    token: Option<&Token>,
) -> Result<String, TaskError> {
    // Remove from whichever section holds the task at top level.
    let section = top_level_section(source_track, task_id)
        .ok_or_else(|| TaskError::NotFound(task_id.to_string()))?;
    let source_tasks = source_track
        .section_tasks_mut(section)
        .ok_or_else(|| TaskError::InvalidPosition("no such section in source".into()))?;

    let idx = source_tasks
        .iter()
        .position(|t| t.id.as_deref() == Some(task_id))
        .ok_or_else(|| TaskError::NotFound(task_id.to_string()))?;

    let mut task = source_tasks.remove(idx);

    // Assign a new ID by max-scanning the target in the *mover's* namespace
    // (`token`): only the mover writes its own namespace, so this re-mint can't
    // collide with the target track's own actors, who are invisible to the scan.
    let next_num = next_id_number(target_track, target_prefix, token);
    let new_id = TaskId::with_number(target_prefix, next_num as u32, token);
    let old_id = task.id.clone();
    task.id = Some(new_id.clone());
    task.mark_dirty();

    // Re-key the subtree into the mover's namespace too.
    renumber_subtasks(&mut task, &new_id, token);

    // Insert into the same section in the target, creating it if the target
    // track doesn't have it yet (e.g. moving a Done task into a track that has
    // never had one).
    target_track.ensure_section(section);
    let target_tasks = target_track
        .section_tasks_mut(section)
        .ok_or_else(|| TaskError::InvalidPosition("no such section in target".into()))?;
    insert_at(target_tasks, task, &position)?;

    // Update dep references across all tracks
    if let Some(old) = &old_id {
        update_dep_references(all_tracks_for_dep_update, old, &new_id);
    }

    Ok(new_id.to_string())
}

// ---------------------------------------------------------------------------
// Section moves
// ---------------------------------------------------------------------------

/// Move a top-level task (with its entire subtree) from one section to another.
/// Returns the original index in the source section, or None if the task is not
/// found as a top-level task in the source section (subtasks are not moved independently).
pub fn move_task_between_sections(
    track: &mut Track,
    task_id: &str,
    from: SectionKind,
    to: SectionKind,
) -> Option<usize> {
    // Remove from source section
    let task = {
        let source = track.section_tasks_mut(from)?;
        let idx = source
            .iter()
            .position(|t| t.id.as_deref() == Some(task_id))?;
        let task = source.remove(idx);
        // Store idx for caller before we drop the borrow
        (idx, task)
    };
    let (source_index, task) = task;

    // Ensure the destination section exists, then insert at top
    track.ensure_section(to);
    if let Some(dest) = track.section_tasks_mut(to) {
        dest.insert(0, task);
    }

    Some(source_index)
}

/// Check if a task ID is a top-level task in the given section.
pub fn is_top_level_in_section(track: &Track, task_id: &str, section: SectionKind) -> bool {
    track
        .section_tasks(section)
        .iter()
        .any(|t| t.id.as_deref() == Some(task_id))
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn today_str() -> String {
    Local::now().format("%Y-%m-%d").to_string()
}

fn remove_metadata(task: &mut Task, key: &str) {
    task.metadata.retain(|m| m.key() != key);
}

/// Find the next available ID number for a given prefix in a track, scoped to
/// the `token` namespace (`None` = null). Two clones minting in different
/// namespaces never collide because each scans only its own.
pub fn next_id_number(track: &Track, prefix: &str, token: Option<&Token>) -> usize {
    let mut max = 0usize;
    let prefix_dash = format!("{}-", prefix);
    find_max_id_in_track(track, &prefix_dash, token, &mut max);
    max + 1
}

/// Scan a track for the highest ID number with the given prefix (e.g. "T-") in
/// the `token` namespace (`None` = null). Updates `max` if a higher number is
/// found.
///
/// Only structured IDs under the prefix whose first segment is in `token`'s
/// namespace are considered; `Raw` IDs and segments in other namespaces are
/// invisible and never perturb `max`.
pub fn find_max_id_in_track(
    track: &Track,
    prefix_dash: &str,
    token: Option<&Token>,
    max: &mut usize,
) {
    let prefix = prefix_dash.strip_suffix('-').unwrap_or(prefix_dash);
    for_each_task_in_track(track, &mut |task: &Task| {
        if let Some(id) = &task.id
            && let Some(n) = id.top_level_number(prefix, token)
            && (n as usize) > *max
        {
            *max = n as usize;
        }
    });
}

/// Insert a task at the given position in a task list.
fn insert_at(
    tasks: &mut Vec<Task>,
    task: Task,
    position: &InsertPosition,
) -> Result<(), TaskError> {
    match position {
        InsertPosition::Bottom => tasks.push(task),
        InsertPosition::Top => tasks.insert(0, task),
        InsertPosition::After(after_id) => {
            let idx = tasks
                .iter()
                .position(|t| t.id.as_deref() == Some(after_id.as_str()))
                .ok_or_else(|| TaskError::NotFound(format!("after target {}", after_id)))?;
            tasks.insert(idx + 1, task);
        }
    }
    Ok(())
}

/// Find a task by ID anywhere in a track (including subtasks), return mutable ref.
pub fn find_task_mut_in_track<'a>(track: &'a mut Track, task_id: &str) -> Option<&'a mut Task> {
    for node in &mut track.nodes {
        if let TrackNode::Section { tasks, .. } = node
            && let Some(t) = find_task_mut_in_list(tasks, task_id)
        {
            return Some(t);
        }
    }
    None
}

fn find_task_mut_in_list<'a>(tasks: &'a mut [Task], task_id: &str) -> Option<&'a mut Task> {
    for task in tasks.iter_mut() {
        if task.id.as_deref() == Some(task_id) {
            return Some(task);
        }
        if let Some(t) = find_task_mut_in_list(&mut task.subtasks, task_id) {
            return Some(t);
        }
    }
    None
}

/// Find a task by ID anywhere in a track (immutable).
pub fn find_task_in_track<'a>(track: &'a Track, task_id: &str) -> Option<&'a Task> {
    for node in &track.nodes {
        if let TrackNode::Section { tasks, .. } = node
            && let Some(t) = find_task_in_list(tasks, task_id)
        {
            return Some(t);
        }
    }
    None
}

fn find_task_in_list<'a>(tasks: &'a [Task], task_id: &str) -> Option<&'a Task> {
    for task in tasks {
        if task.id.as_deref() == Some(task_id) {
            return Some(task);
        }
        if let Some(t) = find_task_in_list(&task.subtasks, task_id) {
            return Some(t);
        }
    }
    None
}

/// Check if a task ID exists anywhere across the provided tracks.
fn task_id_exists_in_tracks(task_id: &str, all_tracks: &[(String, Track)]) -> bool {
    all_tracks
        .iter()
        .any(|(_, track)| find_task_in_track(track, task_id).is_some())
}

/// Iterate over all tasks in a track (all sections, including subtasks).
fn for_each_task_in_track(track: &Track, f: &mut dyn FnMut(&Task)) {
    for node in &track.nodes {
        if let TrackNode::Section { tasks, .. } = node {
            for_each_task(tasks, f);
        }
    }
}

fn for_each_task(tasks: &[Task], f: &mut dyn FnMut(&Task)) {
    for task in tasks {
        f(task);
        for_each_task(&task.subtasks, f);
    }
}

/// Recursively renumber subtask IDs based on the new parent ID. New child
/// segments carry `token` (`None` = null namespace).
pub fn renumber_subtasks(task: &mut Task, parent_id: &str, token: Option<&Token>) {
    let parent = TaskId::parse(parent_id);
    for (i, sub) in task.subtasks.iter_mut().enumerate() {
        let new_sub_id = TaskId::child_of(&parent, (i + 1) as u32, token);
        renumber_subtasks(sub, &new_sub_id, token);
        sub.id = Some(new_sub_id);
        sub.mark_dirty();
    }
}

/// Update all dep references across tracks from old_id to new_id.
pub fn update_dep_references(tracks: &mut [(String, Track)], old_id: &str, new_id: &str) {
    for (_, track) in tracks.iter_mut() {
        for node in &mut track.nodes {
            if let TrackNode::Section { tasks, .. } = node {
                update_deps_in_tasks(tasks, old_id, new_id);
            }
        }
    }
}

/// True if any task (or subtask) in the track is marked dirty. Lets a caller
/// persist exactly the tracks an in-memory mutation touched (e.g. the tracks
/// whose dep references a cross-track move rewrote).
pub fn track_has_dirty_task(track: &Track) -> bool {
    fn any_dirty(tasks: &[Task]) -> bool {
        tasks.iter().any(|t| t.dirty || any_dirty(&t.subtasks))
    }
    track
        .nodes
        .iter()
        .any(|node| matches!(node, TrackNode::Section { tasks, .. } if any_dirty(tasks)))
}

/// Update all dep references within a single track from old_id to new_id.
pub fn update_dep_references_in_track(track: &mut Track, old_id: &str, new_id: &str) {
    for node in &mut track.nodes {
        if let TrackNode::Section { tasks, .. } = node {
            update_deps_in_tasks(tasks, old_id, new_id);
        }
    }
}

fn update_deps_in_tasks(tasks: &mut [Task], old_id: &str, new_id: &str) {
    for task in tasks.iter_mut() {
        let mut changed = false;
        for m in &mut task.metadata {
            if let Metadata::Dep(deps) = m {
                for dep in deps.iter_mut() {
                    if dep == old_id {
                        *dep = new_id.to_string();
                        changed = true;
                    }
                }
            }
        }
        if changed {
            task.mark_dirty();
        }
        update_deps_in_tasks(&mut task.subtasks, old_id, new_id);
    }
}

// ---------------------------------------------------------------------------
// Reparent helpers
// ---------------------------------------------------------------------------

/// Find a task's location (parent and sibling index) within a track.
pub fn find_task_location(
    track: &Track,
    task_id: &str,
    section: SectionKind,
) -> Option<TaskLocation> {
    let tasks = track.section_tasks(section);
    // Check top-level
    for (i, task) in tasks.iter().enumerate() {
        if task.id.as_deref() == Some(task_id) {
            return Some(TaskLocation {
                section,
                parent_id: None,
                sibling_index: i,
            });
        }
        if let Some(loc) = find_in_subtasks(task, task_id, section) {
            return Some(loc);
        }
    }
    None
}

fn find_in_subtasks(parent: &Task, task_id: &str, section: SectionKind) -> Option<TaskLocation> {
    let parent_id = parent.id.as_ref()?;
    for (i, sub) in parent.subtasks.iter().enumerate() {
        if sub.id.as_deref() == Some(task_id) {
            return Some(TaskLocation {
                section,
                parent_id: Some(parent_id.to_string()),
                sibling_index: i,
            });
        }
        if let Some(loc) = find_in_subtasks(sub, task_id, section) {
            return Some(loc);
        }
    }
    None
}

/// Find a task's location across all sections of a track.
pub fn find_task_location_any_section(track: &Track, task_id: &str) -> Option<TaskLocation> {
    for kind in &[SectionKind::Backlog, SectionKind::Parked, SectionKind::Done] {
        if let Some(loc) = find_task_location(track, task_id, *kind) {
            return Some(loc);
        }
    }
    None
}

/// Remove a task (with its entire subtree) from its current position.
/// Returns the task and its original location.
pub fn remove_task_subtree(track: &mut Track, task_id: &str) -> Option<(Task, TaskLocation)> {
    for node in &mut track.nodes {
        if let TrackNode::Section { kind, tasks, .. } = node
            && let Some(result) = remove_from_list(tasks, task_id, *kind, None)
        {
            return Some(result);
        }
    }
    None
}

fn remove_from_list(
    tasks: &mut Vec<Task>,
    task_id: &str,
    section: SectionKind,
    parent_id: Option<&str>,
) -> Option<(Task, TaskLocation)> {
    for i in 0..tasks.len() {
        if tasks[i].id.as_deref() == Some(task_id) {
            let task = tasks.remove(i);
            return Some((
                task,
                TaskLocation {
                    section,
                    parent_id: parent_id.map(|s| s.to_string()),
                    sibling_index: i,
                },
            ));
        }
        let pid = tasks[i].id.clone();
        if let Some(pid) = &pid
            && let Some(result) =
                remove_from_list(&mut tasks[i].subtasks, task_id, section, Some(&**pid))
        {
            tasks[i].mark_dirty();
            return Some(result);
        }
    }
    None
}

/// Insert a task subtree at a specific location in a track.
pub fn insert_task_subtree(
    track: &mut Track,
    mut task: Task,
    parent_id: Option<&str>,
    section: SectionKind,
    index: usize,
) -> Result<(), TaskError> {
    match parent_id {
        None => {
            // Insert as top-level task
            let tasks = track
                .section_tasks_mut(section)
                .ok_or_else(|| TaskError::InvalidPosition("no such section".into()))?;
            let idx = index.min(tasks.len());
            task.mark_dirty();
            tasks.insert(idx, task);
            Ok(())
        }
        Some(pid) => {
            let parent = find_task_mut_in_track(track, pid)
                .ok_or_else(|| TaskError::NotFound(pid.to_string()))?;
            let idx = index.min(parent.subtasks.len());
            task.mark_dirty();
            parent.subtasks.insert(idx, task);
            parent.mark_dirty();
            Ok(())
        }
    }
}

/// Recursively set the depth of a task and all its subtasks.
pub fn set_subtree_depth(task: &mut Task, depth: usize) {
    task.depth = depth;
    task.mark_dirty();
    for sub in &mut task.subtasks {
        set_subtree_depth(sub, depth + 1);
    }
}

/// Get the maximum relative depth of any descendant in a task's subtree.
/// A task with no children returns 0. A task with children returns 1 + max of children.
pub fn max_subtree_depth(task: &Task) -> usize {
    if task.subtasks.is_empty() {
        0
    } else {
        1 + task
            .subtasks
            .iter()
            .map(max_subtree_depth)
            .max()
            .unwrap_or(0)
    }
}

/// Assign new IDs to a task and all its descendants. Re-minted child segments
/// carry `token` (the mover's namespace; `None` = null), so a re-keyed subtree
/// lands entirely in the mover's namespace and can't collide with the target's
/// own actors. Returns the list of (old_id, new_id) mappings.
pub fn rekey_subtree(
    task: &mut Task,
    new_id: &str,
    token: Option<&Token>,
) -> Vec<(String, String)> {
    let mut mappings = Vec::new();
    if let Some(ref old_id) = task.id {
        mappings.push((old_id.to_string(), new_id.to_string()));
    }
    let parsed = TaskId::parse(new_id);
    for (i, sub) in task.subtasks.iter_mut().enumerate() {
        let sub_new_id = TaskId::child_of(&parsed, (i + 1) as u32, token);
        let sub_mappings = rekey_subtree(sub, &sub_new_id, token);
        mappings.extend(sub_mappings);
    }
    task.id = Some(parsed);
    task.mark_dirty();
    mappings
}

/// Check if `candidate_id` is a descendant of `ancestor_id` in the given track.
pub fn is_descendant_of(track: &Track, ancestor_id: &str, candidate_id: &str) -> bool {
    if let Some(ancestor) = find_task_in_track(track, ancestor_id) {
        return find_task_in_list(&ancestor.subtasks, candidate_id).is_some();
    }
    false
}

/// Compute the next available child number for a parent task.
/// Scans existing subtask IDs to find the max suffix number, avoiding collisions
/// when subtasks have been deleted (e.g., deleting .3 from [.1, .2, .3, .4]
/// should produce .5, not .4).
pub fn next_child_number(parent: &Task, token: Option<&Token>) -> usize {
    let parent_id = match &parent.id {
        Some(id) => id,
        None => return parent.subtasks.len() + 1,
    };
    let max_num = parent
        .subtasks
        .iter()
        .filter_map(|sub| {
            sub.id
                .as_ref()
                .and_then(|cid| cid.child_number_of(parent_id, token))
        })
        .max()
        .unwrap_or(0);
    (max_num as usize) + 1
}

/// Compute the next child ID for a parent (gap-safe), in the `token` namespace
/// (`None` = null). `parent_id` is the parent task's ID as a string, used as a
/// fallback when the parent has no parsed ID.
fn next_child_id(parent: &Task, parent_id: &str, token: Option<&Token>) -> TaskId {
    let num = next_child_number(parent, token) as u32;
    match &parent.id {
        Some(pid) => TaskId::child_of(pid, num, token),
        None => TaskId::child_of(&TaskId::parse(parent_id), num, token),
    }
}

/// Main reparent operation: move a task to a new parent (or promote to top-level).
///
/// - `new_parent_id`: None = promote to top-level, Some(id) = reparent under that task.
/// - `sibling_index`: position among the new parent's children (or top-level tasks).
///   Use `usize::MAX` to append at the end.
/// - `prefix`: the track's ID prefix (e.g., "EFF") for generating new IDs.
/// - `all_tracks`: all tracks in the project for updating dep references.
pub fn reparent_task(
    track: &mut Track,
    task_id: &str,
    new_parent_id: Option<&str>,
    sibling_index: usize,
    prefix: &str,
    all_tracks: &mut [(String, Track)],
    token: Option<&Token>,
) -> Result<ReparentResult, TaskError> {
    // 1. Validate task exists
    let _old_location = find_task_location_any_section(track, task_id)
        .ok_or_else(|| TaskError::NotFound(task_id.to_string()))?;

    // 2. Cycle check: new_parent must not be a descendant of the task
    if let Some(new_pid) = new_parent_id {
        if is_descendant_of(track, task_id, new_pid) {
            return Err(TaskError::CycleDetected);
        }
        if new_pid == task_id {
            return Err(TaskError::CycleDetected);
        }
    }

    // 3. Depth check: determine the new depth and verify max depth constraint
    let new_depth = match &new_parent_id {
        None => 0,
        Some(pid) => {
            let parent = find_task_in_track(track, pid)
                .ok_or_else(|| TaskError::NotFound(pid.to_string()))?;
            parent.depth + 1
        }
    };

    // Get the task's max subtree depth before removing
    let task_max_depth = find_task_in_track(track, task_id)
        .map(max_subtree_depth)
        .unwrap_or(0);

    if new_depth + task_max_depth > 2 {
        return Err(TaskError::DepthExceeded);
    }

    // 4. Remove the task subtree
    let (mut task, actual_old_location) = remove_task_subtree(track, task_id)
        .ok_or_else(|| TaskError::NotFound(task_id.to_string()))?;

    // 5. Compute new ID by scanning in the *mover's* namespace (`token`). Only
    // the mover writes its own namespace, so the re-mint can't collide.
    let new_id = match &new_parent_id {
        None => {
            // Promote to top-level: next available top-level ID in the namespace.
            let next_num = next_id_number(track, prefix, token);
            TaskId::with_number(prefix, next_num as u32, token)
        }
        Some(pid) => {
            // Reparent under parent: find next available child slot.
            let parent = find_task_in_track(track, pid)
                .ok_or_else(|| TaskError::NotFound(pid.to_string()))?;
            next_child_id(parent, pid, token)
        }
    };

    // 6. Rekey subtree into the mover's namespace
    let id_mappings = rekey_subtree(&mut task, &new_id, token);

    // 7. Set depths
    set_subtree_depth(&mut task, new_depth);

    // 8. Insert at new location
    let section = actual_old_location.section;
    insert_task_subtree(track, task, new_parent_id, section, sibling_index)?;

    // 9. Update dep references across all tracks
    for (old_id, new_mapped_id) in &id_mappings {
        update_dep_references(all_tracks, old_id, new_mapped_id);
        // Also update within the current track (which may not be in all_tracks)
        update_dep_references_in_track(track, old_id, new_mapped_id);
    }

    Ok(ReparentResult {
        new_root_id: new_id.to_string(),
        id_mappings,
        old_location: actual_old_location,
    })
}

// ---------------------------------------------------------------------------
// Hard delete (physical removal, not mark-as-done)
// ---------------------------------------------------------------------------

/// Information about a deleted task (for undo and recovery logging)
#[derive(Debug, Clone)]
pub struct DeletedTask {
    pub track_id: String,
    pub section: SectionKind,
    pub parent_id: Option<String>,
    pub position: usize,
    pub task: Task,
}

/// Physically remove a task (and its entire subtree) from a track.
/// Returns the deleted task data for undo/recovery, or an error if not found.
pub fn hard_delete_task(
    track: &mut Track,
    task_id: &str,
    track_id: &str,
) -> Result<DeletedTask, TaskError> {
    let location = find_task_location_any_section(track, task_id)
        .ok_or_else(|| TaskError::NotFound(task_id.to_string()))?;

    let (task, _) = remove_task_subtree(track, task_id)
        .ok_or_else(|| TaskError::NotFound(task_id.to_string()))?;

    Ok(DeletedTask {
        track_id: track_id.to_string(),
        section: location.section,
        parent_id: location.parent_id,
        position: location.sibling_index,
        task,
    })
}

/// Reinsert a previously deleted task at its original position.
pub fn reinsert_task(track: &mut Track, deleted: &DeletedTask) -> Result<(), TaskError> {
    insert_task_subtree(
        track,
        deleted.task.clone(),
        deleted.parent_id.as_deref(),
        deleted.section,
        deleted.position,
    )
}

/// Count the total number of tasks in a subtree (including the root task itself).
pub fn count_subtree_size(task: &Task) -> usize {
    1 + task.subtasks.iter().map(count_subtree_size).sum::<usize>()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::parse_track;

    fn sample_track() -> Track {
        parse_track(
            "\
# Test Track

> A test track.

## Backlog

- [ ] `T-001` First task #core
  - added: 2025-05-01
- [>] `T-002` Second task
  - added: 2025-05-02
  - dep: T-001
- [ ] `T-003` Third task with subtasks
  - added: 2025-05-03
  - [ ] `T-003.1` Sub one
  - [ ] `T-003.2` Sub two

## Parked

- [~] `T-010` Parked idea

## Done

- [x] `T-000` Setup project
  - added: 2025-04-20
  - resolved: 2025-04-25
",
        )
    }

    // --- 2.1 State transitions ---

    #[test]
    fn test_cycle_state_todo_active_done() {
        let mut track = sample_track();
        let task = find_task_mut_in_track(&mut track, "T-001").unwrap();
        assert_eq!(task.state, TaskState::Todo);

        cycle_state(task);
        assert_eq!(task.state, TaskState::Active);
        assert!(task.dirty);

        cycle_state(task);
        assert_eq!(task.state, TaskState::Done);
        // Should have resolved date
        assert!(task.metadata.iter().any(|m| m.key() == "resolved"));

        cycle_state(task);
        assert_eq!(task.state, TaskState::Todo);
        // Resolved should be removed
        assert!(!task.metadata.iter().any(|m| m.key() == "resolved"));
    }

    #[test]
    fn test_toggle_blocked() {
        let mut track = sample_track();
        let task = find_task_mut_in_track(&mut track, "T-001").unwrap();

        set_blocked(task);
        assert_eq!(task.state, TaskState::Blocked);

        set_blocked(task);
        assert_eq!(task.state, TaskState::Todo);
    }

    #[test]
    fn test_toggle_parked() {
        let mut track = sample_track();
        let task = find_task_mut_in_track(&mut track, "T-001").unwrap();

        set_parked(task);
        assert_eq!(task.state, TaskState::Parked);

        set_parked(task);
        assert_eq!(task.state, TaskState::Todo);
    }

    #[test]
    fn test_set_done_adds_resolved() {
        let mut track = sample_track();
        let task = find_task_mut_in_track(&mut track, "T-001").unwrap();

        set_done(task);
        assert_eq!(task.state, TaskState::Done);
        assert!(task.metadata.iter().any(|m| m.key() == "resolved"));
    }

    #[test]
    fn test_set_state_noop_same_state() {
        let mut track = sample_track();
        let task = find_task_mut_in_track(&mut track, "T-001").unwrap();
        task.dirty = false;

        set_state(task, TaskState::Todo);
        assert!(!task.dirty); // no change
    }

    // --- 2.2 CRUD ---

    #[test]
    fn test_add_task_bottom() {
        let mut track = sample_track();
        let id = add_task(
            &mut track,
            "New task".into(),
            InsertPosition::Bottom,
            "T",
            None,
        )
        .unwrap();
        assert_eq!(id, "T-011");
        let tasks = track.backlog();
        assert_eq!(tasks.last().unwrap().title, "New task");
        assert!(
            tasks
                .last()
                .unwrap()
                .metadata
                .iter()
                .any(|m| m.key() == "added")
        );
    }

    #[test]
    fn test_add_task_top() {
        let mut track = sample_track();
        let id = add_task(
            &mut track,
            "Top task".into(),
            InsertPosition::Top,
            "T",
            None,
        )
        .unwrap();
        assert_eq!(id, "T-011");
        assert_eq!(track.backlog()[0].title, "Top task");
    }

    #[test]
    fn test_add_task_after() {
        let mut track = sample_track();
        let id = add_task(
            &mut track,
            "After first".into(),
            InsertPosition::After("T-001".into()),
            "T",
            None,
        )
        .unwrap();
        assert_eq!(id, "T-011");
        assert_eq!(track.backlog()[1].title, "After first");
    }

    #[test]
    fn test_add_subtask() {
        let mut track = sample_track();
        let id = add_subtask(&mut track, "T-001", "New sub".into(), None).unwrap();
        assert_eq!(id, "T-001.1");
        let parent = find_task_in_track(&track, "T-001").unwrap();
        assert_eq!(parent.subtasks.len(), 1);
        assert_eq!(parent.subtasks[0].title, "New sub");
    }

    #[test]
    fn test_add_subtask_max_depth() {
        let mut track = sample_track();
        // T-003.1 is depth 1, add sub to make depth 2
        let id = add_subtask(&mut track, "T-003.1", "Deep sub".into(), None).unwrap();
        assert!(id.starts_with("T-003.1."));

        // Now try to add another level (depth 3) — should fail
        let result = add_subtask(&mut track, &id, "Too deep".into(), None);
        assert!(result.is_err());
    }

    #[test]
    fn test_edit_title() {
        let mut track = sample_track();
        edit_title(&mut track, "T-001", "Updated title".into()).unwrap();
        let task = find_task_in_track(&track, "T-001").unwrap();
        assert_eq!(task.title, "Updated title");
        assert!(task.dirty);
    }

    #[test]
    fn test_delete_task() {
        let mut track = sample_track();
        delete_task(&mut track, "T-001").unwrap();
        let task = find_task_in_track(&track, "T-001").unwrap();
        assert_eq!(task.state, TaskState::Done);
        assert!(task.tags.contains(&"wontdo".to_string()));
    }

    // --- 2.3 Metadata ---

    #[test]
    fn test_add_remove_tag() {
        let mut track = sample_track();
        add_tag(&mut track, "T-001", "bug").unwrap();
        let task = find_task_in_track(&track, "T-001").unwrap();
        assert!(task.tags.contains(&"bug".to_string()));

        // Adding again is a no-op
        let mut track2 = sample_track();
        add_tag(&mut track2, "T-001", "#core").unwrap(); // with # prefix
        let task2 = find_task_in_track(&track2, "T-001").unwrap();
        assert_eq!(task2.tags.iter().filter(|t| *t == "core").count(), 1);

        // Remove
        remove_tag(&mut track, "T-001", "bug").unwrap();
        let task = find_task_in_track(&track, "T-001").unwrap();
        assert!(!task.tags.contains(&"bug".to_string()));
    }

    #[test]
    fn test_add_dep() {
        let mut track = sample_track();
        let tracks = vec![("test".to_string(), sample_track())];
        add_dep(&mut track, "T-003", "T-001", &tracks).unwrap();
        let task = find_task_in_track(&track, "T-003").unwrap();
        let deps: Vec<&str> = task
            .metadata
            .iter()
            .filter_map(|m| match m {
                Metadata::Dep(d) => Some(d.iter().map(|s| s.as_str()).collect::<Vec<_>>()),
                _ => None,
            })
            .flatten()
            .collect();
        assert!(deps.contains(&"T-001"));
    }

    #[test]
    fn test_add_dep_invalid_target() {
        let mut track = sample_track();
        let tracks = vec![("test".to_string(), sample_track())];
        let result = add_dep(&mut track, "T-001", "NONEXIST-999", &tracks);
        assert!(result.is_err());
    }

    #[test]
    fn test_remove_dep() {
        let mut track = sample_track();
        remove_dep(&mut track, "T-002", "T-001").unwrap();
        let task = find_task_in_track(&track, "T-002").unwrap();
        assert!(
            !task
                .metadata
                .iter()
                .any(|m| matches!(m, Metadata::Dep(d) if d.contains(&"T-001".to_string())))
        );
    }

    #[test]
    fn test_set_note() {
        let mut track = sample_track();
        set_note(&mut track, "T-001", "This is a note.".into()).unwrap();
        let task = find_task_in_track(&track, "T-001").unwrap();
        assert!(
            task.metadata
                .iter()
                .any(|m| matches!(m, Metadata::Note(n) if n == "This is a note."))
        );
    }

    #[test]
    fn test_append_note_no_existing() {
        let mut track = sample_track();
        append_note(&mut track, "T-001", "First note.".into()).unwrap();
        let task = find_task_in_track(&track, "T-001").unwrap();
        assert!(
            task.metadata
                .iter()
                .any(|m| matches!(m, Metadata::Note(n) if n == "First note."))
        );
    }

    #[test]
    fn test_append_note_with_existing() {
        let mut track = sample_track();
        set_note(&mut track, "T-001", "First note.".into()).unwrap();
        append_note(&mut track, "T-001", "Second note.".into()).unwrap();
        let task = find_task_in_track(&track, "T-001").unwrap();
        assert!(
            task.metadata
                .iter()
                .any(|m| matches!(m, Metadata::Note(n) if n == "First note.\n\nSecond note."))
        );
    }

    #[test]
    fn test_add_ref() {
        let mut track = sample_track();
        add_ref(&mut track, "T-001", "doc/design.md").unwrap();
        let task = find_task_in_track(&track, "T-001").unwrap();
        assert!(
            task.metadata
                .iter()
                .any(|m| matches!(m, Metadata::Ref(r) if r.contains(&"doc/design.md".to_string())))
        );
    }

    #[test]
    fn test_set_spec() {
        let mut track = sample_track();
        set_spec(&mut track, "T-001", "doc/spec.md#section".into()).unwrap();
        let task = find_task_in_track(&track, "T-001").unwrap();
        assert!(
            task.metadata
                .iter()
                .any(|m| matches!(m, Metadata::Spec(s) if s == "doc/spec.md#section"))
        );
    }

    // --- 2.4 Move ---

    #[test]
    fn test_move_task_to_top() {
        let mut track = sample_track();
        move_task(&mut track, "T-003", InsertPosition::Top).unwrap();
        assert_eq!(track.backlog()[0].id.as_deref(), Some("T-003"));
    }

    #[test]
    fn test_move_task_after() {
        let mut track = sample_track();
        move_task(&mut track, "T-001", InsertPosition::After("T-002".into())).unwrap();
        assert_eq!(track.backlog()[0].id.as_deref(), Some("T-002"));
        assert_eq!(track.backlog()[1].id.as_deref(), Some("T-001"));
    }

    #[test]
    fn test_move_task_to_bottom() {
        let mut track = sample_track();
        move_task(&mut track, "T-001", InsertPosition::Bottom).unwrap();
        let backlog = track.backlog();
        assert_eq!(backlog.last().unwrap().id.as_deref(), Some("T-001"));
    }

    // --- Section moves ---

    #[test]
    fn test_move_task_between_sections_backlog_to_done() {
        let mut track = sample_track();
        let backlog_count = track.backlog().len();
        let done_count = track.done().len();

        let idx = move_task_between_sections(
            &mut track,
            "T-001",
            SectionKind::Backlog,
            SectionKind::Done,
        );
        assert_eq!(idx, Some(0)); // was first in backlog
        assert_eq!(track.backlog().len(), backlog_count - 1);
        assert_eq!(track.done().len(), done_count + 1);
        // Should be at top of Done section
        assert_eq!(track.done()[0].id.as_deref(), Some("T-001"));
    }

    #[test]
    fn test_move_task_between_sections_with_subtasks() {
        let mut track = sample_track();
        // T-003 has 2 subtasks
        let sub_count = track.backlog()[2].subtasks.len();
        assert_eq!(sub_count, 2);

        let idx = move_task_between_sections(
            &mut track,
            "T-003",
            SectionKind::Backlog,
            SectionKind::Done,
        );
        assert_eq!(idx, Some(2)); // was third in backlog
        // Subtasks should travel with parent
        let done_task = &track.done()[0];
        assert_eq!(done_task.id.as_deref(), Some("T-003"));
        assert_eq!(done_task.subtasks.len(), 2);
    }

    #[test]
    fn test_move_task_between_sections_subtask_returns_none() {
        let mut track = sample_track();
        // T-003.1 is a subtask — should not be movable independently
        let result = move_task_between_sections(
            &mut track,
            "T-003.1",
            SectionKind::Backlog,
            SectionKind::Done,
        );
        assert_eq!(result, None);
    }

    #[test]
    fn test_move_task_between_sections_creates_missing_section() {
        // Track with only Backlog and Done (no Parked section)
        let mut track = parse_track(
            "\
# Test Track

## Backlog

- [ ] `T-001` First task

## Done
",
        );
        assert!(track.section_tasks_mut(SectionKind::Parked).is_none());

        let idx = move_task_between_sections(
            &mut track,
            "T-001",
            SectionKind::Backlog,
            SectionKind::Parked,
        );
        assert_eq!(idx, Some(0));
        // Task should now be in the newly-created Parked section
        assert_eq!(track.parked().len(), 1);
        assert_eq!(track.parked()[0].id.as_deref(), Some("T-001"));
        assert_eq!(track.backlog().len(), 0);

        // Verify Parked section was inserted between Backlog and Done
        let section_order: Vec<SectionKind> = track
            .nodes
            .iter()
            .filter_map(|n| {
                if let TrackNode::Section { kind, .. } = n {
                    Some(*kind)
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(
            section_order,
            vec![SectionKind::Backlog, SectionKind::Parked, SectionKind::Done]
        );
    }

    #[test]
    fn test_is_top_level_in_section() {
        let track = sample_track();
        assert!(is_top_level_in_section(
            &track,
            "T-001",
            SectionKind::Backlog
        ));
        assert!(!is_top_level_in_section(
            &track,
            "T-003.1",
            SectionKind::Backlog
        ));
        assert!(!is_top_level_in_section(&track, "T-001", SectionKind::Done));
        assert!(is_top_level_in_section(&track, "T-000", SectionKind::Done));
    }

    // --- Reparent helpers ---

    #[test]
    fn test_find_task_location_top_level() {
        let track = sample_track();
        let loc = find_task_location(&track, "T-001", SectionKind::Backlog).unwrap();
        assert_eq!(loc.section, SectionKind::Backlog);
        assert!(loc.parent_id.is_none());
        assert_eq!(loc.sibling_index, 0);

        let loc2 = find_task_location(&track, "T-003", SectionKind::Backlog).unwrap();
        assert!(loc2.parent_id.is_none());
        assert_eq!(loc2.sibling_index, 2);
    }

    #[test]
    fn test_find_task_location_nested() {
        let track = sample_track();
        let loc = find_task_location(&track, "T-003.1", SectionKind::Backlog).unwrap();
        assert_eq!(loc.section, SectionKind::Backlog);
        assert_eq!(loc.parent_id.as_deref(), Some("T-003"));
        assert_eq!(loc.sibling_index, 0);

        let loc2 = find_task_location(&track, "T-003.2", SectionKind::Backlog).unwrap();
        assert_eq!(loc2.parent_id.as_deref(), Some("T-003"));
        assert_eq!(loc2.sibling_index, 1);
    }

    #[test]
    fn test_find_task_location_not_found() {
        let track = sample_track();
        assert!(find_task_location(&track, "T-999", SectionKind::Backlog).is_none());
    }

    #[test]
    fn test_find_task_location_any_section() {
        let track = sample_track();
        let loc = find_task_location_any_section(&track, "T-000").unwrap();
        assert_eq!(loc.section, SectionKind::Done);
        assert!(loc.parent_id.is_none());

        let loc2 = find_task_location_any_section(&track, "T-010").unwrap();
        assert_eq!(loc2.section, SectionKind::Parked);
    }

    #[test]
    fn test_remove_insert_task_subtree_round_trip() {
        let mut track = sample_track();
        let original_count = track.backlog().len();

        // Remove T-002
        let (task, loc) = remove_task_subtree(&mut track, "T-002").unwrap();
        assert_eq!(task.id.as_deref(), Some("T-002"));
        assert!(loc.parent_id.is_none());
        assert_eq!(loc.sibling_index, 1);
        assert_eq!(track.backlog().len(), original_count - 1);

        // Re-insert at the same position
        insert_task_subtree(&mut track, task, None, SectionKind::Backlog, 1).unwrap();
        assert_eq!(track.backlog().len(), original_count);
        assert_eq!(track.backlog()[1].id.as_deref(), Some("T-002"));
    }

    #[test]
    fn test_remove_insert_subtask_round_trip() {
        let mut track = sample_track();
        let original_sub_count = track.backlog()[2].subtasks.len();

        // Remove T-003.1 (subtask)
        let (task, loc) = remove_task_subtree(&mut track, "T-003.1").unwrap();
        assert_eq!(task.id.as_deref(), Some("T-003.1"));
        assert_eq!(loc.parent_id.as_deref(), Some("T-003"));
        assert_eq!(loc.sibling_index, 0);
        assert_eq!(track.backlog()[2].subtasks.len(), original_sub_count - 1);

        // Re-insert
        insert_task_subtree(&mut track, task, Some("T-003"), SectionKind::Backlog, 0).unwrap();
        assert_eq!(track.backlog()[2].subtasks.len(), original_sub_count);
        assert_eq!(
            track.backlog()[2].subtasks[0].id.as_deref(),
            Some("T-003.1")
        );
    }

    #[test]
    fn test_max_subtree_depth() {
        let track = sample_track();
        // T-001 has no subtasks
        let t1 = find_task_in_track(&track, "T-001").unwrap();
        assert_eq!(max_subtree_depth(t1), 0);

        // T-003 has 2 subtasks (depth 1)
        let t3 = find_task_in_track(&track, "T-003").unwrap();
        assert_eq!(max_subtree_depth(t3), 1);
    }

    #[test]
    fn test_max_subtree_depth_deep() {
        // Create a 3-level task manually
        let track = parse_track(
            "\
# Deep Track

## Backlog

- [ ] `D-001` Root
  - [ ] `D-001.1` Child
    - [ ] `D-001.1.1` Grandchild
",
        );
        let root = find_task_in_track(&track, "D-001").unwrap();
        assert_eq!(max_subtree_depth(root), 2);

        let child = find_task_in_track(&track, "D-001.1").unwrap();
        assert_eq!(max_subtree_depth(child), 1);

        let grandchild = find_task_in_track(&track, "D-001.1.1").unwrap();
        assert_eq!(max_subtree_depth(grandchild), 0);
    }

    #[test]
    fn test_rekey_subtree() {
        let mut track = sample_track();
        // Extract T-003 with its subtasks
        let (mut task, _) = remove_task_subtree(&mut track, "T-003").unwrap();

        let mappings = rekey_subtree(&mut task, "T-005", None);
        assert_eq!(mappings.len(), 3); // T-003, T-003.1, T-003.2
        assert_eq!(mappings[0], ("T-003".to_string(), "T-005".to_string()));
        assert_eq!(mappings[1], ("T-003.1".to_string(), "T-005.1".to_string()));
        assert_eq!(mappings[2], ("T-003.2".to_string(), "T-005.2".to_string()));

        assert_eq!(task.id.as_deref(), Some("T-005"));
        assert_eq!(task.subtasks[0].id.as_deref(), Some("T-005.1"));
        assert_eq!(task.subtasks[1].id.as_deref(), Some("T-005.2"));
    }

    #[test]
    fn test_is_descendant_of() {
        let track = sample_track();
        assert!(is_descendant_of(&track, "T-003", "T-003.1"));
        assert!(is_descendant_of(&track, "T-003", "T-003.2"));
        assert!(!is_descendant_of(&track, "T-003", "T-001"));
        assert!(!is_descendant_of(&track, "T-001", "T-003"));
        // Not reflexive
        assert!(!is_descendant_of(&track, "T-003", "T-003"));
    }

    #[test]
    fn test_set_subtree_depth() {
        let mut track = sample_track();
        let (mut task, _) = remove_task_subtree(&mut track, "T-003").unwrap();

        // Originally T-003 is depth 0, subtasks depth 1
        set_subtree_depth(&mut task, 1);
        assert_eq!(task.depth, 1);
        assert_eq!(task.subtasks[0].depth, 2);
        assert_eq!(task.subtasks[1].depth, 2);
    }

    #[test]
    fn test_reparent_promote_to_top_level() {
        let mut track = sample_track();
        let mut all_tracks: Vec<(String, Track)> = Vec::new();

        let result = reparent_task(
            &mut track,
            "T-003.1",
            None, // promote to top-level
            usize::MAX,
            "T",
            &mut all_tracks,
            None,
        )
        .unwrap();

        // Should get a new top-level ID
        assert!(result.new_root_id.starts_with("T-"));
        assert!(!result.new_root_id.contains('.'));

        // Old location should show parent
        assert_eq!(result.old_location.parent_id.as_deref(), Some("T-003"));

        // T-003 should now have only 1 subtask
        let parent = find_task_in_track(&track, "T-003").unwrap();
        assert_eq!(parent.subtasks.len(), 1);

        // New task should be top-level in backlog
        let promoted = find_task_in_track(&track, &result.new_root_id).unwrap();
        assert_eq!(promoted.depth, 0);
    }

    #[test]
    fn test_reparent_under_new_parent() {
        let mut track = sample_track();
        let mut all_tracks: Vec<(String, Track)> = Vec::new();

        // Move T-001 to become a child of T-002
        let result = reparent_task(
            &mut track,
            "T-001",
            Some("T-002"),
            usize::MAX,
            "T",
            &mut all_tracks,
            None,
        )
        .unwrap();

        // New ID should be T-002.1
        assert_eq!(result.new_root_id, "T-002.1");

        // T-002 should now have T-001's content as a child
        let parent = find_task_in_track(&track, "T-002").unwrap();
        assert_eq!(parent.subtasks.len(), 1);
        assert_eq!(parent.subtasks[0].id.as_deref(), Some("T-002.1"));
        assert_eq!(parent.subtasks[0].title, "First task");

        // The reparented task should have depth 1
        let reparented = find_task_in_track(&track, "T-002.1").unwrap();
        assert_eq!(reparented.depth, 1);
    }

    #[test]
    fn test_reparent_updates_dep_references() {
        let mut track = sample_track();
        let mut all_tracks: Vec<(String, Track)> = Vec::new();

        // T-002 has dep: T-001. Promote T-003.1 (no deps involved, but let's
        // reparent T-001 and check that T-002's dep is updated)
        let result = reparent_task(
            &mut track,
            "T-001",
            Some("T-003"),
            0,
            "T",
            &mut all_tracks,
            None,
        )
        .unwrap();

        let new_id = &result.new_root_id;
        // T-002's dep should now reference the new ID
        let t2 = find_task_in_track(&track, "T-002").unwrap();
        let deps: Vec<&str> = t2
            .metadata
            .iter()
            .filter_map(|m| {
                if let Metadata::Dep(deps) = m {
                    Some(deps.iter().map(|s| s.as_str()).collect::<Vec<_>>())
                } else {
                    None
                }
            })
            .flatten()
            .collect();
        assert!(deps.contains(&new_id.as_str()));
        assert!(!deps.contains(&"T-001"));
    }

    #[test]
    fn test_reparent_depth_limit() {
        // Create a track with 2-level nesting, then try to go deeper
        let mut track = parse_track(
            "\
# Deep Track

## Backlog

- [ ] `D-001` Root
  - [ ] `D-001.1` Child
    - [ ] `D-001.1.1` Grandchild
- [ ] `D-002` Another root
",
        );
        let mut all_tracks: Vec<(String, Track)> = Vec::new();

        // Try to reparent D-002 under D-001.1.1 (would be depth 3) — should fail
        let result = reparent_task(
            &mut track,
            "D-002",
            Some("D-001.1.1"),
            usize::MAX,
            "D",
            &mut all_tracks,
            None,
        );
        assert!(matches!(result, Err(TaskError::DepthExceeded)));
    }

    #[test]
    fn test_reparent_depth_limit_with_subtree() {
        // A task with children can't go as deep
        let mut track = parse_track(
            "\
# Deep Track

## Backlog

- [ ] `D-001` Root
  - [ ] `D-001.1` Child
- [ ] `D-002` Has kids
  - [ ] `D-002.1` Sub
",
        );
        let mut all_tracks: Vec<(String, Track)> = Vec::new();

        // Try to reparent D-002 (which has depth-1 subtree) under D-001.1
        // That would put D-002 at depth 2 and D-002.1 at depth 3 — exceeds limit
        let result = reparent_task(
            &mut track,
            "D-002",
            Some("D-001.1"),
            usize::MAX,
            "D",
            &mut all_tracks,
            None,
        );
        assert!(matches!(result, Err(TaskError::DepthExceeded)));
    }

    #[test]
    fn test_reparent_cycle_detection() {
        let mut track = sample_track();
        let mut all_tracks: Vec<(String, Track)> = Vec::new();

        // Try to reparent T-003 under its own child T-003.1 — cycle
        let result = reparent_task(
            &mut track,
            "T-003",
            Some("T-003.1"),
            usize::MAX,
            "T",
            &mut all_tracks,
            None,
        );
        assert!(matches!(result, Err(TaskError::CycleDetected)));
    }

    #[test]
    fn test_reparent_self_cycle() {
        let mut track = sample_track();
        let mut all_tracks: Vec<(String, Track)> = Vec::new();

        // Try to reparent T-001 under itself
        let result = reparent_task(
            &mut track,
            "T-001",
            Some("T-001"),
            usize::MAX,
            "T",
            &mut all_tracks,
            None,
        );
        assert!(matches!(result, Err(TaskError::CycleDetected)));
    }

    #[test]
    fn test_update_dep_references_in_track() {
        let mut track = sample_track();
        // T-002 depends on T-001. Rename T-001 to T-099.
        update_dep_references_in_track(&mut track, "T-001", "T-099");

        let t2 = find_task_in_track(&track, "T-002").unwrap();
        let has_new_dep = t2.metadata.iter().any(|m| {
            if let Metadata::Dep(deps) = m {
                deps.contains(&"T-099".to_string())
            } else {
                false
            }
        });
        assert!(has_new_dep);
    }

    // --- Hard delete ---

    #[test]
    fn test_hard_delete_top_level() {
        let mut track = sample_track();
        let deleted = hard_delete_task(&mut track, "T-001", "test").unwrap();
        assert_eq!(deleted.section, SectionKind::Backlog);
        assert!(deleted.parent_id.is_none());
        assert_eq!(deleted.position, 0);
        assert_eq!(deleted.task.title, "First task");
        assert!(find_task_in_track(&track, "T-001").is_none());
        assert_eq!(track.backlog().len(), 2);
    }

    #[test]
    fn test_hard_delete_subtask() {
        let mut track = sample_track();
        let deleted = hard_delete_task(&mut track, "T-003.1", "test").unwrap();
        assert_eq!(deleted.parent_id.as_deref(), Some("T-003"));
        assert_eq!(deleted.position, 0);
        let parent = find_task_in_track(&track, "T-003").unwrap();
        assert_eq!(parent.subtasks.len(), 1);
    }

    #[test]
    fn test_hard_delete_with_subtree() {
        let mut track = sample_track();
        let deleted = hard_delete_task(&mut track, "T-003", "test").unwrap();
        assert_eq!(deleted.task.subtasks.len(), 2);
        assert!(find_task_in_track(&track, "T-003").is_none());
        assert!(find_task_in_track(&track, "T-003.1").is_none());
        assert!(find_task_in_track(&track, "T-003.2").is_none());
    }

    #[test]
    fn test_reinsert_round_trip() {
        let mut track = sample_track();
        let original_count = track.backlog().len();
        let deleted = hard_delete_task(&mut track, "T-002", "test").unwrap();
        assert_eq!(track.backlog().len(), original_count - 1);

        reinsert_task(&mut track, &deleted).unwrap();
        assert_eq!(track.backlog().len(), original_count);
        assert_eq!(track.backlog()[1].id.as_deref(), Some("T-002"));
    }

    #[test]
    fn test_count_subtree_size() {
        let track = sample_track();
        let t1 = find_task_in_track(&track, "T-001").unwrap();
        assert_eq!(count_subtree_size(t1), 1);

        let t3 = find_task_in_track(&track, "T-003").unwrap();
        assert_eq!(count_subtree_size(t3), 3); // T-003 + T-003.1 + T-003.2
    }

    #[test]
    fn test_hard_delete_not_found() {
        let mut track = sample_track();
        let result = hard_delete_task(&mut track, "NOPE", "test");
        assert!(result.is_err());
    }

    #[test]
    fn test_next_child_number_after_deletion() {
        use crate::parse::parse_track;

        let track = parse_track(
            "\
# Test

## Backlog

- [ ] `T-001` Parent
  - [ ] `T-001.1` Sub 1
  - [ ] `T-001.2` Sub 2
  - [ ] `T-001.3` Sub 3
  - [ ] `T-001.4` Sub 4

## Done",
        );

        // With all 4 subtasks, next should be 5
        let parent = find_task_in_track(&track, "T-001").unwrap();
        assert_eq!(next_child_number(parent, None), 5);

        // Delete subtask 3 — next should still be 5, not 4
        let mut track = track;
        hard_delete_task(&mut track, "T-001.3", "test").unwrap();
        let parent = find_task_in_track(&track, "T-001").unwrap();
        assert_eq!(parent.subtasks.len(), 3);
        assert_eq!(next_child_number(parent, None), 5); // must skip over existing .4

        // Delete subtask 4 too — now remaining are [.1, .2], max is 2, so next is 3
        hard_delete_task(&mut track, "T-001.4", "test").unwrap();
        let parent = find_task_in_track(&track, "T-001").unwrap();
        assert_eq!(parent.subtasks.len(), 2);
        assert_eq!(next_child_number(parent, None), 3); // .3 and .4 are gone, no collision
    }

    #[test]
    fn test_reparent_child_number_gap_safe() {
        // Regression: reparenting under a parent must use the gap-safe child
        // number, not the child count. After deleting a middle sibling, the new
        // child must skip the deleted number rather than collide.
        let mut track = parse_track(
            "\
# Test

## Backlog

- [ ] `T-001` Standalone
- [ ] `T-003` Parent
  - [ ] `T-003.1` Sub 1
  - [ ] `T-003.2` Sub 2
  - [ ] `T-003.3` Sub 3

## Done",
        );
        // Delete the middle child → children are [.1, .3], count is 2 but max is 3.
        hard_delete_task(&mut track, "T-003.2", "test").unwrap();

        let mut all_tracks: Vec<(String, Track)> = Vec::new();
        let result = reparent_task(
            &mut track,
            "T-001",
            Some("T-003"),
            usize::MAX,
            "T",
            &mut all_tracks,
            None,
        )
        .unwrap();

        // Gap-safe: next child is .4 (not .3, which exists, nor .2, deleted).
        assert_eq!(result.new_root_id, "T-003.4");
    }

    #[test]
    fn test_raw_id_invisible_to_mint() {
        // A non-conforming (Raw) ID must not perturb the next minted number.
        let mut track = parse_track(
            "\
# Test

## Backlog

- [ ] `T-005` Real task
- [ ] `weird id!` Raw id task

## Done",
        );
        let id = add_task(&mut track, "New".into(), InsertPosition::Bottom, "T", None).unwrap();
        assert_eq!(id, "T-006");
    }

    // --- Namespace-scoped minting (Phase 3) ---

    fn ns(s: &str) -> crate::model::task_id::Token {
        crate::model::task_id::Token::new(s).unwrap()
    }

    #[test]
    fn test_add_task_mints_in_token_namespace() {
        // A track containing EFF-a3, EFF-b7, EFF-a5: actor `a` mints EFF-a6,
        // actor `b` mints EFF-b8, actor `c` mints EFF-c1 (empty namespace → 1),
        // and null mints the next bare-digit.
        let src = "\
# Eff

## Backlog

- [ ] `EFF-014` Null task
- [ ] `EFF-a3` A task
- [ ] `EFF-b7` B task
- [ ] `EFF-a5` Another A task

## Done";
        let mint = |token: Option<&Token>| {
            let mut track = parse_track(src);
            add_task(
                &mut track,
                "New".into(),
                InsertPosition::Bottom,
                "EFF",
                token,
            )
            .unwrap()
        };
        assert_eq!(mint(Some(&ns("a"))), "EFF-a6");
        assert_eq!(mint(Some(&ns("b"))), "EFF-b8");
        assert_eq!(mint(Some(&ns("c"))), "EFF-c1");
        assert_eq!(mint(None), "EFF-015");
    }

    #[test]
    fn test_add_task_null_and_token_coexist() {
        // EFF-014 (null) and EFF-a14 (token a): null mints EFF-015 (ignoring a14),
        // actor a mints EFF-a15 (ignoring 014).
        let src = "\
# Eff

## Backlog

- [ ] `EFF-014` Null task
- [ ] `EFF-a14` A task

## Done";
        let mut t1 = parse_track(src);
        assert_eq!(
            add_task(&mut t1, "N".into(), InsertPosition::Bottom, "EFF", None).unwrap(),
            "EFF-015"
        );
        let mut t2 = parse_track(src);
        assert_eq!(
            add_task(
                &mut t2,
                "N".into(),
                InsertPosition::Bottom,
                "EFF",
                Some(&ns("a"))
            )
            .unwrap(),
            "EFF-a15"
        );
    }

    #[test]
    fn test_add_subtask_mints_in_token_namespace() {
        // Actor b adds a child under EFF-a14 → EFF-a14.b1; a second b child →
        // EFF-a14.b2. The scan is scoped to that parent and to b's namespace.
        let src = "\
# Eff

## Backlog

- [ ] `EFF-a14` Parent

## Done";
        let mut track = parse_track(src);
        let first = add_subtask(&mut track, "EFF-a14", "child".into(), Some(&ns("b"))).unwrap();
        assert_eq!(first, "EFF-a14.b1");
        let second = add_subtask(&mut track, "EFF-a14", "child2".into(), Some(&ns("b"))).unwrap();
        assert_eq!(second, "EFF-a14.b2");
        // A different actor's child starts its own sequence under the same parent.
        let other = add_subtask(&mut track, "EFF-a14", "child3".into(), Some(&ns("c"))).unwrap();
        assert_eq!(other, "EFF-a14.c1");
    }

    #[test]
    fn test_reclaim_continues_sequence() {
        // A track with EFF-b1..EFF-b9 where `b` was retired and re-claimed by a
        // new owner → next `b` mint is EFF-b10 (derive-from-max-scan is automatic).
        let mut src = String::from("# Eff\n\n## Backlog\n\n");
        for i in 1..=9 {
            src.push_str(&format!("- [ ] `EFF-b{}` Task {}\n", i, i));
        }
        src.push_str("\n## Done");
        let mut track = parse_track(&src);
        let id = add_task(
            &mut track,
            "New".into(),
            InsertPosition::Bottom,
            "EFF",
            Some(&ns("b")),
        )
        .unwrap();
        assert_eq!(id, "EFF-b10");
    }

    // --- Token-namespace lookup / compare (Phase 5a) ---
    //
    // Every lookup compares the whole canonical id string
    // (`task.id.as_deref() == Some(id)`), so a tokened target resolves to its
    // own task and never to a same-prefix id in another namespace. These tests
    // pin the shared primitive behind jump-to-task, `--after`, `--parent`,
    // dep-add validation, and `expand_parent_chain`'s ancestor walk.

    fn tokened_track() -> Track {
        parse_track(
            "\
# Eff

## Backlog

- [ ] `EFF-a14` Tokened parent
  - added: 2025-05-01
  - [ ] `EFF-a14.b2` Tokened subtask
    - added: 2025-05-01
- [ ] `EFF-14` Null-namespace sibling
  - added: 2025-05-02
- [ ] `EFF-b14` Different-token sibling
  - added: 2025-05-03

## Done",
        )
    }

    #[test]
    fn test_find_task_resolves_tokened_id() {
        let track = tokened_track();
        // A tokened id resolves to its own task, distinct from the same-prefix
        // ids in the null and `b` namespaces.
        assert_eq!(
            find_task_in_track(&track, "EFF-a14").unwrap().title,
            "Tokened parent"
        );
        assert_eq!(
            find_task_in_track(&track, "EFF-14").unwrap().title,
            "Null-namespace sibling"
        );
        assert_eq!(
            find_task_in_track(&track, "EFF-b14").unwrap().title,
            "Different-token sibling"
        );
        // A tokened subtask resolves through segment boundaries; its parent id
        // (the `expand_parent_chain` ancestor) resolves too.
        assert_eq!(
            find_task_in_track(&track, "EFF-a14.b2").unwrap().title,
            "Tokened subtask"
        );
        let parent_id = "EFF-a14.b2".rsplit_once('.').unwrap().0;
        assert_eq!(parent_id, "EFF-a14");
        assert!(find_task_in_track(&track, parent_id).is_some());
    }

    #[test]
    fn test_add_task_after_tokened_target() {
        // `--after` resolves a tokened target id when inserting.
        let mut track = tokened_track();
        let new_id = add_task(
            &mut track,
            "Inserted".into(),
            InsertPosition::After("EFF-a14".into()),
            "EFF",
            Some(&ns("a")),
        )
        .unwrap();
        let backlog = track.backlog();
        let pos = backlog
            .iter()
            .position(|t| t.id.as_deref() == Some(new_id.as_str()))
            .unwrap();
        // Landed immediately after the tokened target, not after a same-prefix
        // id in another namespace.
        assert_eq!(backlog[pos - 1].id.as_deref(), Some("EFF-a14"));
    }

    #[test]
    fn test_add_dep_on_tokened_target() {
        // dep-add validates against the tokened task; the same-prefix null id
        // does not stand in for a missing tokened id.
        let mut track = tokened_track();
        let all_tracks = vec![("eff".to_string(), tokened_track())];

        add_dep(&mut track, "EFF-14", "EFF-a14", &all_tracks).unwrap();
        let task = find_task_in_track(&track, "EFF-14").unwrap();
        assert!(
            task.metadata
                .iter()
                .any(|m| matches!(m, Metadata::Dep(d) if d.contains(&"EFF-a14".to_string())))
        );

        // A dep on a non-existent tokened id is rejected even though the
        // null-namespace `EFF-14` exists.
        let err = add_dep(&mut track, "EFF-14", "EFF-c99", &all_tracks);
        assert!(err.is_err());
    }

    // --- Cross-track / reparent re-key into the mover's namespace (Phase 4) ---

    #[test]
    fn test_cross_track_move_mints_in_movers_namespace() {
        // Actor c moves EFF-a14 (created by a) into INF, which already holds
        // INF-b3. The new id is scanned in c's namespace — b3 is invisible — so
        // it is INF-c1, not INF-a… and not a null INF-…. A dep elsewhere that
        // points at EFF-a14 is rewritten to the new id.
        let mut source =
            parse_track("# Eff\n\n## Backlog\n\n- [ ] `EFF-a14` Created by a\n\n## Done");
        let mut target =
            parse_track("# Inf\n\n## Backlog\n\n- [ ] `INF-b3` Created by b\n\n## Done");
        let mut others = vec![(
            "oth".to_string(),
            parse_track(
                "# Oth\n\n## Backlog\n\n- [ ] `OTH-001` Dependent\n  - dep: EFF-a14\n\n## Done",
            ),
        )];

        let new_id = move_task_to_track(
            &mut source,
            &mut target,
            "EFF-a14",
            InsertPosition::Bottom,
            "INF",
            &mut others,
            Some(&ns("c")),
        )
        .unwrap();

        assert_eq!(new_id, "INF-c1");
        assert!(find_task_in_track(&source, "EFF-a14").is_none());
        assert!(find_task_in_track(&target, "INF-c1").is_some());
        // Dep rewritten to the new id everywhere it appears.
        let dep = find_task_in_track(&others[0].1, "OTH-001").unwrap();
        assert!(
            dep.metadata
                .iter()
                .any(|m| matches!(m, Metadata::Dep(d) if d == &vec!["INF-c1".to_string()]))
        );
    }

    #[test]
    fn test_cross_track_move_rekeys_subtree_into_movers_namespace() {
        // A tokened task with children, moved by c, re-keys the whole subtree
        // into c's namespace: INF-c1, INF-c1.c1, INF-c1.c2.
        let mut source = parse_track(
            "# Eff\n\n## Backlog\n\n- [ ] `EFF-a14` Parent\n  - [ ] `EFF-a14.b1` Child one\n  - [ ] `EFF-a14.b2` Child two\n\n## Done",
        );
        let mut target = parse_track("# Inf\n\n## Backlog\n\n## Done");

        let new_id = move_task_to_track(
            &mut source,
            &mut target,
            "EFF-a14",
            InsertPosition::Bottom,
            "INF",
            &mut [],
            Some(&ns("c")),
        )
        .unwrap();

        assert_eq!(new_id, "INF-c1");
        let moved = find_task_in_track(&target, "INF-c1").unwrap();
        assert_eq!(moved.subtasks[0].id.as_deref(), Some("INF-c1.c1"));
        assert_eq!(moved.subtasks[1].id.as_deref(), Some("INF-c1.c2"));
    }

    #[test]
    fn test_cross_track_move_of_done_task_preserves_done_section() {
        // A completed task (in the Done section) can be moved cross-track without
        // reopening it: it lands in the *target's* Done section, keeping its
        // resolved date. Regression for the "task not found" bug where the move
        // only scanned the Backlog section.
        let mut source = parse_track(
            "# Eff\n\n## Backlog\n\n## Done\n\n- [x] `EFF-b8` Finished\n  - resolved: 2025-05-01",
        );
        let mut target = parse_track("# Inf\n\n## Backlog\n\n- [ ] `INF-b1` Existing\n\n## Done");

        let new_id = move_task_to_track(
            &mut source,
            &mut target,
            "EFF-b8",
            InsertPosition::Bottom,
            "INF",
            &mut [],
            Some(&ns("b")),
        )
        .unwrap();

        assert_eq!(new_id, "INF-b2");
        assert!(find_task_in_track(&source, "EFF-b8").is_none());
        // Landed in the target's Done section (not the Backlog), still done.
        let moved = target
            .section_tasks(SectionKind::Done)
            .iter()
            .find(|t| t.id.as_deref() == Some("INF-b2"))
            .expect("moved task should be in the target Done section");
        assert_eq!(moved.state, TaskState::Done);
        assert!(moved.metadata.iter().any(|m| m.key() == "resolved"));
    }

    #[test]
    fn test_cross_track_move_of_parked_task_preserves_parked_section() {
        // A parked task lands in the target's Parked section, created on demand
        // when the target has never had one.
        let mut source =
            parse_track("# Eff\n\n## Backlog\n\n## Parked\n\n- [~] `EFF-b3` Someday\n\n## Done");
        let mut target = parse_track("# Inf\n\n## Backlog\n\n## Done");

        let new_id = move_task_to_track(
            &mut source,
            &mut target,
            "EFF-b3",
            InsertPosition::Bottom,
            "INF",
            &mut [],
            Some(&ns("b")),
        )
        .unwrap();

        assert_eq!(new_id, "INF-b1");
        let moved = target
            .section_tasks(SectionKind::Parked)
            .iter()
            .find(|t| t.id.as_deref() == Some("INF-b1"))
            .expect("moved task should be in the target Parked section");
        assert_eq!(moved.state, TaskState::Parked);
    }

    #[test]
    fn test_move_task_reorders_within_done_section() {
        // Same-track reorder finds a task in the Done section, not only the
        // Backlog.
        let mut track = parse_track(
            "# Eff\n\n## Backlog\n\n## Done\n\n- [x] `EFF-b1` First\n- [x] `EFF-b2` Second",
        );
        move_task(&mut track, "EFF-b2", InsertPosition::Top).unwrap();
        let done = track.section_tasks(SectionKind::Done);
        assert_eq!(done[0].id.as_deref(), Some("EFF-b2"));
        assert_eq!(done[1].id.as_deref(), Some("EFF-b1"));
    }

    #[test]
    fn test_promote_mints_top_level_in_movers_namespace() {
        // Actor c promotes EFF-a14.b2 (whose subtree was minted by b) to
        // top-level: a new top-level id in c's namespace (EFF-c1), with the
        // former subtree re-keyed under it carrying c's token.
        let mut track = parse_track(
            "# Eff\n\n## Backlog\n\n- [ ] `EFF-a14` Parent\n  - [ ] `EFF-a14.b2` Child\n    - [ ] `EFF-a14.b2.b1` Grandchild\n\n## Done",
        );
        let mut others: Vec<(String, Track)> = Vec::new();

        let result = reparent_task(
            &mut track,
            "EFF-a14.b2",
            None,
            usize::MAX,
            "EFF",
            &mut others,
            Some(&ns("c")),
        )
        .unwrap();

        assert_eq!(result.new_root_id, "EFF-c1");
        let promoted = find_task_in_track(&track, "EFF-c1").unwrap();
        assert_eq!(promoted.depth, 0);
        assert_eq!(promoted.subtasks[0].id.as_deref(), Some("EFF-c1.c1"));
    }

    #[test]
    fn test_reparent_mints_child_in_movers_namespace() {
        // Actor c reparents EFF-a14 (created by a) under EFF-a20: the new child
        // id is EFF-a20.c1 — the parent segment (a20) is preserved and only the
        // new child segment carries c. This pins the mover-differs-from-creator
        // invariant: the re-mint uses the mover (c), not the creator (a).
        let mut track = parse_track(
            "# Eff\n\n## Backlog\n\n- [ ] `EFF-a14` Mover\n- [ ] `EFF-a20` New parent\n\n## Done",
        );
        let mut others: Vec<(String, Track)> = Vec::new();

        let result = reparent_task(
            &mut track,
            "EFF-a14",
            Some("EFF-a20"),
            usize::MAX,
            "EFF",
            &mut others,
            Some(&ns("c")),
        )
        .unwrap();

        assert_eq!(result.new_root_id, "EFF-a20.c1");
    }

    /// Verify that deleting a subtask from a parent with multiline notes
    /// produces correct serialization that round-trips through parse.
    #[test]
    fn test_delete_subtask_round_trip_with_note() {
        use crate::parse::{serialize_track, track_parser::parse_track as reparse};

        let source = "\
# Test Track

## Backlog

- [ ] `P-001` Parent with note
  - added: 2025-06-01
  - dep: X-001
  - note:

    Some note content here
  - [ ] `P-001.1` First sub
    - added: 2025-06-01
  - [ ] `P-001.2` Second sub
    - added: 2025-06-01
    - [ ] `P-001.2.1` Deep sub
      - added: 2025-06-01
- [ ] `P-002` Sibling task
  - added: 2025-06-01

## Done";

        let mut track = reparse(source);

        // Delete first subtask
        let deleted = hard_delete_task(&mut track, "P-001.1", "test").unwrap();
        assert_eq!(deleted.parent_id.as_deref(), Some("P-001"));
        assert_eq!(deleted.position, 0);

        // Parent should be marked dirty, siblings unaffected
        let parent = find_task_in_track(&track, "P-001").unwrap();
        assert!(parent.dirty);
        assert_eq!(parent.subtasks.len(), 1);
        assert_eq!(parent.subtasks[0].id.as_deref(), Some("P-001.2"));

        // Sibling task should still exist and be clean
        let sibling = find_task_in_track(&track, "P-002").unwrap();
        assert!(!sibling.dirty);

        // Serialize and re-parse
        let output = serialize_track(&track);
        let reparsed = reparse(&output);

        // Verify all expected tasks exist
        assert!(find_task_in_track(&reparsed, "P-001").is_some());
        assert!(find_task_in_track(&reparsed, "P-001.1").is_none()); // deleted
        assert!(find_task_in_track(&reparsed, "P-001.2").is_some());
        assert!(find_task_in_track(&reparsed, "P-001.2.1").is_some());
        assert!(find_task_in_track(&reparsed, "P-002").is_some());

        // Verify parent task metadata survived
        let parent = find_task_in_track(&reparsed, "P-001").unwrap();
        assert_eq!(parent.subtasks.len(), 1);
        assert!(parent.metadata.iter().any(|m| m.key() == "note"));
        assert!(parent.metadata.iter().any(|m| m.key() == "dep"));

        // Verify note content is correct
        let note = parent
            .metadata
            .iter()
            .find_map(|m| {
                if let crate::model::task::Metadata::Note(n) = m {
                    Some(n.clone())
                } else {
                    None
                }
            })
            .unwrap();
        assert!(note.contains("Some note content here"));

        // Verify second round-trip is stable
        let output2 = serialize_track(&reparsed);
        assert_eq!(output, output2);
    }

    /// Verify that deleting a subtask doesn't affect other tasks in the track.
    #[test]
    fn test_delete_subtask_no_collateral_damage() {
        use crate::parse::{serialize_track, track_parser::parse_track as reparse};

        let source = "\
# Test Track

## Backlog

- [ ] `A-001` Task A with subtasks
  - added: 2025-06-01
  - [ ] `A-001.1` Sub A1
    - added: 2025-06-01
  - [ ] `A-001.2` Sub A2
    - added: 2025-06-01
  - [ ] `A-001.3` Sub A3
    - added: 2025-06-01
- [ ] `A-002` Task B with subtasks
  - added: 2025-06-01
  - note:
    Long note here
  - [ ] `A-002.1` Sub B1
    - added: 2025-06-01
  - [ ] `A-002.2` Sub B2
    - added: 2025-06-01
  - [ ] `A-002.3` Sub B3
    - added: 2025-06-01

## Done";

        let mut track = reparse(source);

        // Delete middle subtask of first task
        hard_delete_task(&mut track, "A-001.2", "test").unwrap();

        // Serialize and re-parse
        let output = serialize_track(&track);
        let reparsed = reparse(&output);

        // A-001 should have 2 subtasks
        let a001 = find_task_in_track(&reparsed, "A-001").unwrap();
        assert_eq!(a001.subtasks.len(), 2);
        assert_eq!(a001.subtasks[0].id.as_deref(), Some("A-001.1"));
        assert_eq!(a001.subtasks[1].id.as_deref(), Some("A-001.3"));

        // A-002 should be completely untouched — still 3 subtasks
        let a002 = find_task_in_track(&reparsed, "A-002").unwrap();
        assert_eq!(a002.subtasks.len(), 3);
        assert_eq!(a002.subtasks[0].id.as_deref(), Some("A-002.1"));
        assert_eq!(a002.subtasks[1].id.as_deref(), Some("A-002.2"));
        assert_eq!(a002.subtasks[2].id.as_deref(), Some("A-002.3"));

        // Verify note on A-002 survived
        assert!(a002.metadata.iter().any(|m| {
            matches!(m, crate::model::task::Metadata::Note(n) if n.contains("Long note here"))
        }));
    }
}
