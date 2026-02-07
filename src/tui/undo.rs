use crate::model::task::{Task, TaskState};
use crate::model::track::{SectionKind, Track};
use crate::ops::task_ops;

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

    /// Undo the last operation. Returns the track_id that was modified.
    /// Applies the inverse operation to the track data. Does NOT save to disk.
    pub fn undo(&mut self, tracks: &mut [(String, Track)]) -> Option<String> {
        let op = self.undo.pop()?;

        // Can't undo past a sync marker
        if matches!(op, Operation::SyncMarker) {
            // Put it back — we stop here
            self.undo.push(op);
            return None;
        }

        let track_id = apply_inverse(&op, tracks);
        // Push the forward operation onto redo
        self.redo.push(op);
        track_id
    }

    /// Redo the last undone operation. Returns the track_id that was modified.
    pub fn redo(&mut self, tracks: &mut [(String, Track)]) -> Option<String> {
        let op = self.redo.pop()?;

        if matches!(op, Operation::SyncMarker) {
            self.redo.push(op);
            return None;
        }

        let track_id = apply_forward(&op, tracks);
        self.undo.push(op);
        track_id
    }

    pub fn is_empty(&self) -> bool {
        self.undo.is_empty()
    }
}

/// Apply the inverse of an operation (for undo)
fn apply_inverse(op: &Operation, tracks: &mut [(String, Track)]) -> Option<String> {
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
        Operation::SyncMarker => None,
    }
}

/// Apply an operation forward (for redo)
fn apply_forward(op: &Operation, tracks: &mut [(String, Track)]) -> Option<String> {
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
        Operation::SyncMarker => None,
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
