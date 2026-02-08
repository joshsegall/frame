use chrono::Local;

use crate::model::task::{Metadata, Task, TaskState};
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
) -> Result<String, TaskError> {
    let next_num = next_id_number(track, prefix);
    let id = format!("{}-{:03}", prefix, next_num);

    let (parsed_title, tags) = parse_title_and_tags(&title);
    let mut task = Task::new(TaskState::Todo, Some(id.clone()), parsed_title);
    task.tags = tags;
    task.metadata.push(Metadata::Added(today_str()));

    let tasks = track
        .section_tasks_mut(SectionKind::Backlog)
        .ok_or_else(|| TaskError::InvalidPosition("no backlog section".into()))?;

    insert_at(tasks, task, &position)?;
    Ok(id)
}

/// Add a subtask to an existing task identified by `parent_id`.
/// Returns the assigned subtask ID.
pub fn add_subtask(track: &mut Track, parent_id: &str, title: String) -> Result<String, TaskError> {
    let parent = find_task_mut_in_track(track, parent_id)
        .ok_or_else(|| TaskError::NotFound(parent_id.to_string()))?;

    if parent.depth >= 2 {
        return Err(TaskError::MaxDepthReached);
    }

    let sub_num = parent.subtasks.len() + 1;
    let sub_id = format!("{}.{}", parent_id, sub_num);
    let (parsed_title, tags) = parse_title_and_tags(&title);
    let mut subtask = Task::new(TaskState::Todo, Some(sub_id.clone()), parsed_title);
    subtask.tags = tags;
    subtask.depth = parent.depth + 1;
    subtask.metadata.push(Metadata::Added(today_str()));
    parent.subtasks.push(subtask);
    parent.mark_dirty();

    Ok(sub_id)
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
    let tasks = track
        .section_tasks_mut(SectionKind::Backlog)
        .ok_or_else(|| TaskError::InvalidPosition("no backlog section".into()))?;

    let idx = tasks
        .iter()
        .position(|t| t.id.as_deref() == Some(task_id))
        .ok_or_else(|| TaskError::NotFound(task_id.to_string()))?;

    let task = tasks.remove(idx);
    insert_at(tasks, task, &position)?;
    Ok(())
}

/// Move a task to a different track. Reassigns the task ID using the target
/// track's prefix. Updates dependency references across all provided tracks.
pub fn move_task_to_track(
    source_track: &mut Track,
    target_track: &mut Track,
    task_id: &str,
    position: InsertPosition,
    target_prefix: &str,
    all_tracks_for_dep_update: &mut [(String, Track)],
) -> Result<String, TaskError> {
    // Remove from source backlog
    let source_tasks = source_track
        .section_tasks_mut(SectionKind::Backlog)
        .ok_or_else(|| TaskError::InvalidPosition("no backlog section in source".into()))?;

    let idx = source_tasks
        .iter()
        .position(|t| t.id.as_deref() == Some(task_id))
        .ok_or_else(|| TaskError::NotFound(task_id.to_string()))?;

    let mut task = source_tasks.remove(idx);

    // Assign new ID
    let next_num = next_id_number(target_track, target_prefix);
    let new_id = format!("{}-{:03}", target_prefix, next_num);
    let old_id = task.id.clone();
    task.id = Some(new_id.clone());
    task.mark_dirty();

    // Renumber subtask IDs
    renumber_subtasks(&mut task, &new_id);

    // Insert into target
    let target_tasks = target_track
        .section_tasks_mut(SectionKind::Backlog)
        .ok_or_else(|| TaskError::InvalidPosition("no backlog section in target".into()))?;
    insert_at(target_tasks, task, &position)?;

    // Update dep references across all tracks
    if let Some(old) = &old_id {
        update_dep_references(all_tracks_for_dep_update, old, &new_id);
    }

    Ok(new_id)
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

    // Insert at top of destination section
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

/// Find the next available ID number for a given prefix in a track.
fn next_id_number(track: &Track, prefix: &str) -> usize {
    let mut max = 0usize;
    let prefix_dash = format!("{}-", prefix);
    find_max_id_in_track(track, &prefix_dash, &mut max);
    max + 1
}

/// Scan a track for the highest ID number with the given prefix (e.g. "T-").
/// Updates `max` if a higher number is found.
pub fn find_max_id_in_track(track: &Track, prefix_dash: &str, max: &mut usize) {
    for_each_task_in_track(track, &mut |task: &Task| {
        if let Some(ref id) = task.id {
            if let Some(num_str) = id.strip_prefix(prefix_dash) {
                let num_part = num_str.split('.').next().unwrap_or("");
                if let Ok(n) = num_part.parse::<usize>() {
                    if n > *max {
                        *max = n;
                    }
                }
            }
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
        if let TrackNode::Section { tasks, .. } = node {
            if let Some(t) = find_task_mut_in_list(tasks, task_id) {
                return Some(t);
            }
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
        if let TrackNode::Section { tasks, .. } = node {
            if let Some(t) = find_task_in_list(tasks, task_id) {
                return Some(t);
            }
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

/// Recursively renumber subtask IDs based on the new parent ID.
fn renumber_subtasks(task: &mut Task, parent_id: &str) {
    for (i, sub) in task.subtasks.iter_mut().enumerate() {
        let new_sub_id = format!("{}.{}", parent_id, i + 1);
        sub.id = Some(new_sub_id.clone());
        sub.mark_dirty();
        renumber_subtasks(sub, &new_sub_id);
    }
}

/// Update all dep references across tracks from old_id to new_id.
fn update_dep_references(tracks: &mut [(String, Track)], old_id: &str, new_id: &str) {
    for (_, track) in tracks.iter_mut() {
        for node in &mut track.nodes {
            if let TrackNode::Section { tasks, .. } = node {
                update_deps_in_tasks(tasks, old_id, new_id);
            }
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

- [ ] `T-001` First task #ready
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
        let id = add_task(&mut track, "New task".into(), InsertPosition::Bottom, "T").unwrap();
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
        let id = add_task(&mut track, "Top task".into(), InsertPosition::Top, "T").unwrap();
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
        )
        .unwrap();
        assert_eq!(id, "T-011");
        assert_eq!(track.backlog()[1].title, "After first");
    }

    #[test]
    fn test_add_subtask() {
        let mut track = sample_track();
        let id = add_subtask(&mut track, "T-001", "New sub".into()).unwrap();
        assert_eq!(id, "T-001.1");
        let parent = find_task_in_track(&track, "T-001").unwrap();
        assert_eq!(parent.subtasks.len(), 1);
        assert_eq!(parent.subtasks[0].title, "New sub");
    }

    #[test]
    fn test_add_subtask_max_depth() {
        let mut track = sample_track();
        // T-003.1 is depth 1, add sub to make depth 2
        let id = add_subtask(&mut track, "T-003.1", "Deep sub".into()).unwrap();
        assert!(id.starts_with("T-003.1."));

        // Now try to add another level (depth 3) — should fail
        let result = add_subtask(&mut track, &id, "Too deep".into());
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
        add_tag(&mut track2, "T-001", "#ready").unwrap(); // with # prefix
        let task2 = find_task_in_track(&track2, "T-001").unwrap();
        assert_eq!(task2.tags.iter().filter(|t| *t == "ready").count(), 1);

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
    fn test_is_top_level_in_section() {
        let track = sample_track();
        assert!(is_top_level_in_section(&track, "T-001", SectionKind::Backlog));
        assert!(!is_top_level_in_section(&track, "T-003.1", SectionKind::Backlog));
        assert!(!is_top_level_in_section(&track, "T-001", SectionKind::Done));
        assert!(is_top_level_in_section(&track, "T-000", SectionKind::Done));
    }
}
