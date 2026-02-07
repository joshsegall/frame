use crate::model::inbox::InboxItem;
use crate::model::task::{Task, TaskState};
use crate::model::track::{SectionKind, Track};
use crate::ops::task_ops;

use super::app::DetailRegion;

/// Navigation target after undo/redo — tells the UI where to navigate
#[derive(Debug, Clone)]
pub enum UndoNavTarget {
    /// Navigate to a task in a track view (or detail view)
    Task {
        track_id: String,
        task_id: String,
        /// If Some, open detail view and scroll to this region
        detail_region: Option<DetailRegion>,
        /// True when undo removes a task (TaskAdd undo) — don't try to find the task
        task_removed: bool,
        /// Cursor fallback when task_removed is true
        position_hint: Option<usize>,
    },
    /// Navigate to the tracks overview and flash a track row
    TracksView {
        track_id: String,
    },
    /// Navigate to the inbox view, optionally to a specific item index
    Inbox {
        cursor: Option<usize>,
    },
    /// Navigate to the recent view
    Recent {
        cursor: Option<usize>,
    },
}

/// Derive a navigation target from an operation
pub fn nav_target_for_op(op: &Operation, is_undo: bool) -> Option<UndoNavTarget> {
    match op {
        Operation::StateChange {
            track_id, task_id, ..
        }
        | Operation::TitleEdit {
            track_id, task_id, ..
        }
        | Operation::TaskMove {
            track_id, task_id, ..
        } => Some(UndoNavTarget::Task {
            track_id: track_id.clone(),
            task_id: task_id.clone(),
            detail_region: None,
            task_removed: false,
            position_hint: None,
        }),
        Operation::TaskAdd {
            track_id,
            task_id,
            position_index,
        } => {
            if is_undo {
                Some(UndoNavTarget::Task {
                    track_id: track_id.clone(),
                    task_id: task_id.clone(),
                    detail_region: None,
                    task_removed: true,
                    position_hint: Some(*position_index),
                })
            } else {
                Some(UndoNavTarget::Task {
                    track_id: track_id.clone(),
                    task_id: task_id.clone(),
                    detail_region: None,
                    task_removed: false,
                    position_hint: None,
                })
            }
        }
        Operation::SubtaskAdd {
            track_id,
            parent_id,
            task_id,
        } => {
            if is_undo {
                Some(UndoNavTarget::Task {
                    track_id: track_id.clone(),
                    task_id: parent_id.clone(),
                    detail_region: None,
                    task_removed: false,
                    position_hint: None,
                })
            } else {
                Some(UndoNavTarget::Task {
                    track_id: track_id.clone(),
                    task_id: task_id.clone(),
                    detail_region: None,
                    task_removed: false,
                    position_hint: None,
                })
            }
        }
        Operation::FieldEdit { track_id, task_id, field, .. } => {
            let detail_region = match field.as_str() {
                "note" => Some(DetailRegion::Note),
                "deps" => Some(DetailRegion::Deps),
                "spec" => Some(DetailRegion::Spec),
                "refs" => Some(DetailRegion::Refs),
                _ => None,
            };
            Some(UndoNavTarget::Task {
                track_id: track_id.clone(),
                task_id: task_id.clone(),
                detail_region,
                task_removed: false,
                position_hint: None,
            })
        }
        Operation::TrackMove { track_id, .. } => {
            Some(UndoNavTarget::TracksView {
                track_id: track_id.clone(),
            })
        }
        Operation::InboxAdd { index } => {
            if is_undo {
                // Item was removed by undo — stay at same cursor
                Some(UndoNavTarget::Inbox { cursor: Some(index.saturating_sub(1).max(0)) })
            } else {
                Some(UndoNavTarget::Inbox { cursor: Some(*index) })
            }
        }
        Operation::InboxDelete { index, .. } => {
            if is_undo {
                // Item was restored
                Some(UndoNavTarget::Inbox { cursor: Some(*index) })
            } else {
                Some(UndoNavTarget::Inbox { cursor: Some(index.saturating_sub(1).max(0)) })
            }
        }
        Operation::InboxTitleEdit { index, .. }
        | Operation::InboxTagsEdit { index, .. } => {
            Some(UndoNavTarget::Inbox { cursor: Some(*index) })
        }
        Operation::InboxMove { old_index, new_index } => {
            if is_undo {
                Some(UndoNavTarget::Inbox { cursor: Some(*old_index) })
            } else {
                Some(UndoNavTarget::Inbox { cursor: Some(*new_index) })
            }
        }
        Operation::InboxTriage { inbox_index, track_id, task_id, .. } => {
            if is_undo {
                // Item restored to inbox
                Some(UndoNavTarget::Inbox { cursor: Some(*inbox_index) })
            } else {
                // Item triaged to track
                Some(UndoNavTarget::Task {
                    track_id: track_id.clone(),
                    task_id: task_id.clone(),
                    detail_region: None,
                    task_removed: false,
                    position_hint: None,
                })
            }
        }
        Operation::Reopen { track_id, task_id, .. } => {
            if is_undo {
                // Task was put back in Done section — navigate to Recent view
                Some(UndoNavTarget::Recent { cursor: None })
            } else {
                // Task was reopened — navigate to Track view
                Some(UndoNavTarget::Task {
                    track_id: track_id.clone(),
                    task_id: task_id.clone(),
                    detail_region: None,
                    task_removed: false,
                    position_hint: None,
                })
            }
        }
        Operation::SyncMarker => None,
    }
}

/// A single undoable operation
#[derive(Debug, Clone)]
pub enum Operation {
    /// State change on a task
    StateChange {
        track_id: String,
        task_id: String,
        old_state: TaskState,
        new_state: TaskState,
        /// Old resolved date (if transitioning away from Done)
        old_resolved: Option<String>,
        /// New resolved date (if transitioning to Done)
        new_resolved: Option<String>,
    },
    /// Task title was edited
    TitleEdit {
        track_id: String,
        task_id: String,
        old_title: String,
        new_title: String,
    },
    /// A new task was added
    TaskAdd {
        track_id: String,
        task_id: String,
        /// Position index in backlog where it was inserted
        position_index: usize,
    },
    /// A new subtask was added
    SubtaskAdd {
        track_id: String,
        parent_id: String,
        task_id: String,
    },
    /// A task was moved within the backlog
    TaskMove {
        track_id: String,
        task_id: String,
        old_index: usize,
        new_index: usize,
    },
    /// A detail field was edited (tags, deps, spec, refs, note)
    FieldEdit {
        track_id: String,
        task_id: String,
        field: String,
        old_value: String,
        new_value: String,
    },
    /// A track was reordered in the tracks list
    TrackMove {
        track_id: String,
        old_index: usize,
        new_index: usize,
    },
    /// An inbox item was added
    InboxAdd {
        /// The index where it was inserted
        index: usize,
    },
    /// An inbox item was deleted
    InboxDelete {
        index: usize,
        item: InboxItem,
    },
    /// An inbox item's title was edited
    InboxTitleEdit {
        index: usize,
        old_title: String,
        new_title: String,
    },
    /// An inbox item's tags were edited
    InboxTagsEdit {
        index: usize,
        old_tags: Vec<String>,
        new_tags: Vec<String>,
    },
    /// An inbox item was moved (reordered)
    InboxMove {
        old_index: usize,
        new_index: usize,
    },
    /// An inbox item was triaged into a track
    InboxTriage {
        /// The inbox item that was removed
        inbox_index: usize,
        item: InboxItem,
        /// The track and task ID it was triaged to
        track_id: String,
        task_id: String,
    },
    /// A task was reopened from the recent view (moved from Done to Backlog)
    Reopen {
        track_id: String,
        task_id: String,
        old_state: TaskState,
        old_resolved: Option<String>,
        /// Original index in the Done section
        done_index: usize,
    },
    /// External file change sync marker — undo cannot cross this
    SyncMarker,
}

/// The undo/redo stack
pub struct UndoStack {
    undo: Vec<Operation>,
    redo: Vec<Operation>,
}

impl Default for UndoStack {
    fn default() -> Self {
        Self::new()
    }
}

impl UndoStack {
    pub fn new() -> Self {
        UndoStack {
            undo: Vec::new(),
            redo: Vec::new(),
        }
    }

    /// Push a new operation. Clears the redo stack.
    pub fn push(&mut self, op: Operation) {
        self.undo.push(op);
        self.redo.clear();
    }

    /// Push a sync marker. Clears the redo stack.
    pub fn push_sync_marker(&mut self) {
        self.undo.push(Operation::SyncMarker);
        self.redo.clear();
    }

    /// Undo the last operation. Returns navigation target for the UI.
    /// Applies the inverse operation to the track data. Does NOT save to disk.
    pub fn undo(&mut self, tracks: &mut [(String, Track)], inbox: Option<&mut crate::model::inbox::Inbox>) -> Option<UndoNavTarget> {
        let op = self.undo.pop()?;

        // Can't undo past a sync marker
        if matches!(op, Operation::SyncMarker) {
            // Put it back — we stop here
            self.undo.push(op);
            return None;
        }

        let nav = nav_target_for_op(&op, true);
        apply_inverse(&op, tracks, inbox);
        // Push the forward operation onto redo
        self.redo.push(op);
        nav
    }

    /// Redo the last undone operation. Returns navigation target for the UI.
    pub fn redo(&mut self, tracks: &mut [(String, Track)], inbox: Option<&mut crate::model::inbox::Inbox>) -> Option<UndoNavTarget> {
        let op = self.redo.pop()?;

        if matches!(op, Operation::SyncMarker) {
            self.redo.push(op);
            return None;
        }

        let nav = nav_target_for_op(&op, false);
        apply_forward(&op, tracks, inbox);
        self.undo.push(op);
        nav
    }

    pub fn is_empty(&self) -> bool {
        self.undo.is_empty()
    }

    /// Peek at the last operation on the redo stack (just pushed during undo)
    pub fn peek_last_redo(&self) -> Option<&Operation> {
        self.redo.last()
    }

    /// Peek at the last operation on the undo stack (just pushed during redo)
    pub fn peek_last_undo(&self) -> Option<&Operation> {
        self.undo.last()
    }
}

/// Apply the inverse of an operation (for undo)
fn apply_inverse(op: &Operation, tracks: &mut [(String, Track)], inbox: Option<&mut crate::model::inbox::Inbox>) -> Option<String> {
    match op {
        Operation::StateChange {
            track_id,
            task_id,
            old_state,
            old_resolved,
            ..
        } => {
            let track = find_track_mut(tracks, track_id)?;
            let task = task_ops::find_task_mut_in_track(track, task_id)?;
            task.state = *old_state;
            task.mark_dirty();
            // Restore resolved date
            task.metadata
                .retain(|m| m.key() != "resolved");
            if let Some(date) = old_resolved {
                task.metadata
                    .push(crate::model::task::Metadata::Resolved(date.clone()));
            }
            Some(track_id.clone())
        }
        Operation::TitleEdit {
            track_id,
            task_id,
            old_title,
            ..
        } => {
            let track = find_track_mut(tracks, track_id)?;
            let task = task_ops::find_task_mut_in_track(track, task_id)?;
            task.title = old_title.clone();
            task.mark_dirty();
            Some(track_id.clone())
        }
        Operation::TaskAdd {
            track_id, task_id, ..
        } => {
            // Undo add = remove the task
            let track = find_track_mut(tracks, track_id)?;
            if let Some(tasks) = track.section_tasks_mut(SectionKind::Backlog) {
                tasks.retain(|t| t.id.as_deref() != Some(task_id));
            }
            Some(track_id.clone())
        }
        Operation::SubtaskAdd {
            track_id,
            parent_id,
            task_id,
        } => {
            // Undo subtask add = remove the subtask from parent
            let track = find_track_mut(tracks, track_id)?;
            let parent = task_ops::find_task_mut_in_track(track, parent_id)?;
            parent
                .subtasks
                .retain(|t| t.id.as_deref() != Some(task_id));
            parent.mark_dirty();
            Some(track_id.clone())
        }
        Operation::TaskMove {
            track_id,
            task_id,
            old_index,
            ..
        } => {
            // Undo move = move back to old_index
            let track = find_track_mut(tracks, track_id)?;
            let tasks = track.section_tasks_mut(SectionKind::Backlog)?;
            let cur = tasks
                .iter()
                .position(|t| t.id.as_deref() == Some(task_id))?;
            let task = tasks.remove(cur);
            let idx = (*old_index).min(tasks.len());
            tasks.insert(idx, task);
            Some(track_id.clone())
        }
        Operation::FieldEdit {
            track_id,
            task_id,
            field,
            old_value,
            ..
        } => {
            let track = find_track_mut(tracks, track_id)?;
            let task = task_ops::find_task_mut_in_track(track, task_id)?;
            apply_field_value(task, field, old_value);
            Some(track_id.clone())
        }
        // TrackMove is handled by the caller (needs config access)
        Operation::TrackMove { .. } => None,
        Operation::InboxAdd { index } => {
            // Undo add = remove the item
            if let Some(inbox) = inbox {
                if *index < inbox.items.len() {
                    inbox.items.remove(*index);
                }
            }
            None
        }
        Operation::InboxDelete { index, item } => {
            // Undo delete = re-insert the item
            if let Some(inbox) = inbox {
                let idx = (*index).min(inbox.items.len());
                inbox.items.insert(idx, item.clone());
            }
            None
        }
        Operation::InboxTitleEdit { index, old_title, .. } => {
            if let Some(inbox) = inbox {
                if let Some(item) = inbox.items.get_mut(*index) {
                    item.title = old_title.clone();
                    item.dirty = true;
                }
            }
            None
        }
        Operation::InboxTagsEdit { index, old_tags, .. } => {
            if let Some(inbox) = inbox {
                if let Some(item) = inbox.items.get_mut(*index) {
                    item.tags = old_tags.clone();
                    item.dirty = true;
                }
            }
            None
        }
        Operation::InboxMove { old_index, new_index } => {
            // Undo move = move back from new_index to old_index
            if let Some(inbox) = inbox {
                if *new_index < inbox.items.len() {
                    let item = inbox.items.remove(*new_index);
                    let idx = (*old_index).min(inbox.items.len());
                    inbox.items.insert(idx, item);
                }
            }
            None
        }
        Operation::InboxTriage { inbox_index, item, track_id, task_id } => {
            // Undo triage = remove task from track, re-insert item into inbox
            let track = find_track_mut(tracks, track_id);
            if let Some(track) = track {
                if let Some(tasks) = track.section_tasks_mut(SectionKind::Backlog) {
                    tasks.retain(|t| t.id.as_deref() != Some(task_id));
                }
            }
            if let Some(inbox) = inbox {
                let idx = (*inbox_index).min(inbox.items.len());
                inbox.items.insert(idx, item.clone());
            }
            // Return track_id so caller knows to save
            Some(track_id.clone())
        }
        Operation::Reopen { track_id, task_id, old_state, old_resolved, done_index } => {
            // Undo reopen = move task from Backlog back to Done, restore state
            let track = find_track_mut(tracks, track_id)?;
            let mut task = {
                let backlog = track.section_tasks_mut(SectionKind::Backlog)?;
                let idx = backlog.iter().position(|t| t.id.as_deref() == Some(task_id))?;
                backlog.remove(idx)
            };
            task.state = *old_state;
            task.metadata.retain(|m| m.key() != "resolved");
            if let Some(date) = old_resolved {
                task.metadata.push(crate::model::task::Metadata::Resolved(date.clone()));
            }
            task.mark_dirty();
            if let Some(done) = track.section_tasks_mut(SectionKind::Done) {
                let idx = (*done_index).min(done.len());
                done.insert(idx, task);
            }
            Some(track_id.clone())
        }
        Operation::SyncMarker => None,
    }
}

/// Apply an operation forward (for redo)
fn apply_forward(op: &Operation, tracks: &mut [(String, Track)], inbox: Option<&mut crate::model::inbox::Inbox>) -> Option<String> {
    match op {
        Operation::StateChange {
            track_id,
            task_id,
            new_state,
            new_resolved,
            ..
        } => {
            let track = find_track_mut(tracks, track_id)?;
            let task = task_ops::find_task_mut_in_track(track, task_id)?;
            task.state = *new_state;
            task.mark_dirty();
            task.metadata
                .retain(|m| m.key() != "resolved");
            if let Some(date) = new_resolved {
                task.metadata
                    .push(crate::model::task::Metadata::Resolved(date.clone()));
            }
            Some(track_id.clone())
        }
        Operation::TitleEdit {
            track_id,
            task_id,
            new_title,
            ..
        } => {
            let track = find_track_mut(tracks, track_id)?;
            let task = task_ops::find_task_mut_in_track(track, task_id)?;
            task.title = new_title.clone();
            task.mark_dirty();
            Some(track_id.clone())
        }
        Operation::TaskAdd {
            track_id,
            task_id,
            position_index,
        } => {
            // Redo: re-add the task (it was removed during undo)
            // We need to recreate it — this is a limitation, but we store enough info
            // Actually, for simplicity, we can't perfectly redo an add because the task
            // object was removed. We'll create a minimal placeholder.
            // In practice, redo of add is less common. For now, create a new task.
            let track = find_track_mut(tracks, track_id)?;
            let tasks = track.section_tasks_mut(SectionKind::Backlog)?;
            let mut task = Task::new(TaskState::Todo, Some(task_id.clone()), String::new());
            task.metadata.push(crate::model::task::Metadata::Added(
                chrono::Local::now().format("%Y-%m-%d").to_string(),
            ));
            let idx = (*position_index).min(tasks.len());
            tasks.insert(idx, task);
            Some(track_id.clone())
        }
        Operation::SubtaskAdd {
            track_id,
            parent_id,
            task_id,
        } => {
            let track = find_track_mut(tracks, track_id)?;
            let parent = task_ops::find_task_mut_in_track(track, parent_id)?;
            let mut sub = Task::new(TaskState::Todo, Some(task_id.clone()), String::new());
            sub.depth = parent.depth + 1;
            sub.metadata.push(crate::model::task::Metadata::Added(
                chrono::Local::now().format("%Y-%m-%d").to_string(),
            ));
            parent.subtasks.push(sub);
            parent.mark_dirty();
            Some(track_id.clone())
        }
        Operation::TaskMove {
            track_id,
            task_id,
            new_index,
            ..
        } => {
            let track = find_track_mut(tracks, track_id)?;
            let tasks = track.section_tasks_mut(SectionKind::Backlog)?;
            let cur = tasks
                .iter()
                .position(|t| t.id.as_deref() == Some(task_id))?;
            let task = tasks.remove(cur);
            let idx = (*new_index).min(tasks.len());
            tasks.insert(idx, task);
            Some(track_id.clone())
        }
        Operation::FieldEdit {
            track_id,
            task_id,
            field,
            new_value,
            ..
        } => {
            let track = find_track_mut(tracks, track_id)?;
            let task = task_ops::find_task_mut_in_track(track, task_id)?;
            apply_field_value(task, field, new_value);
            Some(track_id.clone())
        }
        // TrackMove is handled by the caller (needs config access)
        Operation::TrackMove { .. } => None,
        Operation::InboxAdd { index } => {
            // Redo add = re-insert a blank item
            if let Some(inbox) = inbox {
                let item = InboxItem::new(String::new());
                let idx = (*index).min(inbox.items.len());
                inbox.items.insert(idx, item);
            }
            None
        }
        Operation::InboxDelete { index, .. } => {
            // Redo delete = remove the item again
            if let Some(inbox) = inbox {
                if *index < inbox.items.len() {
                    inbox.items.remove(*index);
                }
            }
            None
        }
        Operation::InboxTitleEdit { index, new_title, .. } => {
            if let Some(inbox) = inbox {
                if let Some(item) = inbox.items.get_mut(*index) {
                    item.title = new_title.clone();
                    item.dirty = true;
                }
            }
            None
        }
        Operation::InboxTagsEdit { index, new_tags, .. } => {
            if let Some(inbox) = inbox {
                if let Some(item) = inbox.items.get_mut(*index) {
                    item.tags = new_tags.clone();
                    item.dirty = true;
                }
            }
            None
        }
        Operation::InboxMove { old_index, new_index } => {
            if let Some(inbox) = inbox {
                if *old_index < inbox.items.len() {
                    let item = inbox.items.remove(*old_index);
                    let idx = (*new_index).min(inbox.items.len());
                    inbox.items.insert(idx, item);
                }
            }
            None
        }
        Operation::InboxTriage { inbox_index, track_id, task_id, item, .. } => {
            // Redo triage = remove from inbox, add task to track
            if let Some(inbox) = inbox {
                if *inbox_index < inbox.items.len() {
                    inbox.items.remove(*inbox_index);
                }
            }
            // Re-create task in track
            let track = find_track_mut(tracks, track_id);
            if let Some(track) = track {
                if let Some(tasks) = track.section_tasks_mut(SectionKind::Backlog) {
                    let mut task = Task::new(TaskState::Todo, Some(task_id.clone()), item.title.clone());
                    task.tags = item.tags.clone();
                    task.metadata.push(crate::model::task::Metadata::Added(
                        chrono::Local::now().format("%Y-%m-%d").to_string(),
                    ));
                    if let Some(body) = &item.body {
                        if !body.is_empty() {
                            task.metadata.push(crate::model::task::Metadata::Note(body.clone()));
                        }
                    }
                    tasks.push(task);
                }
            }
            Some(track_id.clone())
        }
        Operation::Reopen { track_id, task_id, .. } => {
            // Redo reopen = move task from Done to Backlog top, set Todo
            let track = find_track_mut(tracks, track_id)?;
            let mut task = {
                let done = track.section_tasks_mut(SectionKind::Done)?;
                let idx = done.iter().position(|t| t.id.as_deref() == Some(task_id))?;
                done.remove(idx)
            };
            task.state = TaskState::Todo;
            task.metadata.retain(|m| m.key() != "resolved");
            task.mark_dirty();
            if let Some(backlog) = track.section_tasks_mut(SectionKind::Backlog) {
                backlog.insert(0, task);
            }
            Some(track_id.clone())
        }
        Operation::SyncMarker => None,
    }
}

/// Apply a value to the appropriate task field based on field name
fn apply_field_value(task: &mut Task, field: &str, value: &str) {
    match field {
        "tags" => {
            task.tags = value
                .split_whitespace()
                .map(|s| s.strip_prefix('#').unwrap_or(s).to_string())
                .filter(|s| !s.is_empty())
                .collect();
            task.mark_dirty();
        }
        "deps" => {
            task.metadata.retain(|m| !matches!(m, crate::model::task::Metadata::Dep(_)));
            let deps: Vec<String> = value
                .split(|c: char| c == ',' || c.is_whitespace())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            if !deps.is_empty() {
                task.metadata.push(crate::model::task::Metadata::Dep(deps));
            }
            task.mark_dirty();
        }
        "spec" => {
            task.metadata.retain(|m| !matches!(m, crate::model::task::Metadata::Spec(_)));
            if !value.trim().is_empty() {
                task.metadata.push(crate::model::task::Metadata::Spec(value.trim().to_string()));
            }
            task.mark_dirty();
        }
        "refs" => {
            task.metadata.retain(|m| !matches!(m, crate::model::task::Metadata::Ref(_)));
            let refs: Vec<String> = value
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            if !refs.is_empty() {
                task.metadata.push(crate::model::task::Metadata::Ref(refs));
            }
            task.mark_dirty();
        }
        "note" => {
            task.metadata.retain(|m| !matches!(m, crate::model::task::Metadata::Note(_)));
            if !value.is_empty() {
                task.metadata.push(crate::model::task::Metadata::Note(value.to_string()));
            }
            task.mark_dirty();
        }
        _ => {}
    }
}

fn find_track_mut<'a>(
    tracks: &'a mut [(String, Track)],
    track_id: &str,
) -> Option<&'a mut Track> {
    tracks
        .iter_mut()
        .find(|(id, _)| id == track_id)
        .map(|(_, track)| track)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper to unwrap Task variant fields
    fn expect_task(nav: UndoNavTarget) -> (String, String, Option<DetailRegion>, bool, Option<usize>) {
        match nav {
            UndoNavTarget::Task { track_id, task_id, detail_region, task_removed, position_hint } =>
                (track_id, task_id, detail_region, task_removed, position_hint),
            other => panic!("expected Task, got {:?}", other),
        }
    }

    fn expect_tracks_view(nav: UndoNavTarget) -> String {
        match nav {
            UndoNavTarget::TracksView { track_id } => track_id,
            other => panic!("expected TracksView, got {:?}", other),
        }
    }

    #[test]
    fn nav_target_state_change() {
        let op = Operation::StateChange {
            track_id: "t1".into(),
            task_id: "T-001".into(),
            old_state: TaskState::Todo,
            new_state: TaskState::Active,
            old_resolved: None,
            new_resolved: None,
        };
        let (track_id, task_id, detail_region, task_removed, _) =
            expect_task(nav_target_for_op(&op, true).unwrap());
        assert_eq!(track_id, "t1");
        assert_eq!(task_id, "T-001");
        assert!(detail_region.is_none());
        assert!(!task_removed);
    }

    #[test]
    fn nav_target_title_edit() {
        let op = Operation::TitleEdit {
            track_id: "t1".into(),
            task_id: "T-002".into(),
            old_title: "old".into(),
            new_title: "new".into(),
        };
        let (_, task_id, _, task_removed, _) =
            expect_task(nav_target_for_op(&op, false).unwrap());
        assert_eq!(task_id, "T-002");
        assert!(!task_removed);
    }

    #[test]
    fn nav_target_task_add_undo_removes() {
        let op = Operation::TaskAdd {
            track_id: "t1".into(),
            task_id: "T-003".into(),
            position_index: 2,
        };
        let (_, _, _, task_removed, position_hint) =
            expect_task(nav_target_for_op(&op, true).unwrap());
        assert!(task_removed);
        assert_eq!(position_hint, Some(2));
    }

    #[test]
    fn nav_target_task_add_redo_creates() {
        let op = Operation::TaskAdd {
            track_id: "t1".into(),
            task_id: "T-003".into(),
            position_index: 2,
        };
        let (_, task_id, _, task_removed, _) =
            expect_task(nav_target_for_op(&op, false).unwrap());
        assert!(!task_removed);
        assert_eq!(task_id, "T-003");
    }

    #[test]
    fn nav_target_subtask_add_undo_goes_to_parent() {
        let op = Operation::SubtaskAdd {
            track_id: "t1".into(),
            parent_id: "T-010".into(),
            task_id: "T-010.1".into(),
        };
        let (_, task_id, _, task_removed, _) =
            expect_task(nav_target_for_op(&op, true).unwrap());
        assert_eq!(task_id, "T-010");
        assert!(!task_removed);
    }

    #[test]
    fn nav_target_subtask_add_redo_goes_to_subtask() {
        let op = Operation::SubtaskAdd {
            track_id: "t1".into(),
            parent_id: "T-010".into(),
            task_id: "T-010.1".into(),
        };
        let (_, task_id, _, _, _) =
            expect_task(nav_target_for_op(&op, false).unwrap());
        assert_eq!(task_id, "T-010.1");
    }

    #[test]
    fn nav_target_task_move() {
        let op = Operation::TaskMove {
            track_id: "t1".into(),
            task_id: "T-005".into(),
            old_index: 0,
            new_index: 3,
        };
        let (_, task_id, detail_region, _, _) =
            expect_task(nav_target_for_op(&op, true).unwrap());
        assert_eq!(task_id, "T-005");
        assert!(detail_region.is_none());
    }

    #[test]
    fn nav_target_field_edit_note_opens_detail() {
        let op = Operation::FieldEdit {
            track_id: "t1".into(),
            task_id: "T-006".into(),
            field: "note".into(),
            old_value: "old note".into(),
            new_value: "new note".into(),
        };
        let (_, _, detail_region, _, _) =
            expect_task(nav_target_for_op(&op, true).unwrap());
        assert_eq!(detail_region, Some(DetailRegion::Note));
    }

    #[test]
    fn nav_target_field_edit_deps_opens_detail() {
        let op = Operation::FieldEdit {
            track_id: "t1".into(),
            task_id: "T-007".into(),
            field: "deps".into(),
            old_value: "".into(),
            new_value: "T-001".into(),
        };
        let (_, _, detail_region, _, _) =
            expect_task(nav_target_for_op(&op, false).unwrap());
        assert_eq!(detail_region, Some(DetailRegion::Deps));
    }

    #[test]
    fn nav_target_field_edit_tags_stays_in_track_view() {
        let op = Operation::FieldEdit {
            track_id: "t1".into(),
            task_id: "T-008".into(),
            field: "tags".into(),
            old_value: "#foo".into(),
            new_value: "#bar".into(),
        };
        let (_, _, detail_region, _, _) =
            expect_task(nav_target_for_op(&op, true).unwrap());
        assert!(detail_region.is_none());
    }

    #[test]
    fn nav_target_field_edit_spec_opens_detail() {
        let op = Operation::FieldEdit {
            track_id: "t1".into(),
            task_id: "T-009".into(),
            field: "spec".into(),
            old_value: "".into(),
            new_value: "spec.md".into(),
        };
        let (_, _, detail_region, _, _) =
            expect_task(nav_target_for_op(&op, true).unwrap());
        assert_eq!(detail_region, Some(DetailRegion::Spec));
    }

    #[test]
    fn nav_target_field_edit_refs_opens_detail() {
        let op = Operation::FieldEdit {
            track_id: "t1".into(),
            task_id: "T-010".into(),
            field: "refs".into(),
            old_value: "".into(),
            new_value: "ref.md".into(),
        };
        let (_, _, detail_region, _, _) =
            expect_task(nav_target_for_op(&op, true).unwrap());
        assert_eq!(detail_region, Some(DetailRegion::Refs));
    }

    #[test]
    fn nav_target_track_move() {
        let op = Operation::TrackMove {
            track_id: "effects".into(),
            old_index: 0,
            new_index: 2,
        };
        let track_id = expect_tracks_view(nav_target_for_op(&op, true).unwrap());
        assert_eq!(track_id, "effects");
    }

    #[test]
    fn nav_target_sync_marker_returns_none() {
        let op = Operation::SyncMarker;
        assert!(nav_target_for_op(&op, true).is_none());
        assert!(nav_target_for_op(&op, false).is_none());
    }
}
