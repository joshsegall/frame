use crate::model::inbox::InboxItem;
use crate::model::task::{Task, TaskState};
use crate::model::track::{SectionKind, Track};
use crate::ops::task_ops;

use super::app::DetailRegion;

const UNDO_STACK_LIMIT: usize = 500;

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
    TracksView { track_id: String },
    /// Navigate to the inbox view, optionally to a specific item index
    Inbox { cursor: Option<usize> },
    /// Navigate to the recent view
    Recent { cursor: Option<usize> },
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
            ..
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
            ..
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
        Operation::FieldEdit {
            track_id,
            task_id,
            field,
            ..
        } => {
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
        Operation::TrackMove { track_id, .. } => Some(UndoNavTarget::TracksView {
            track_id: track_id.clone(),
        }),
        Operation::InboxAdd { index, .. } => {
            if is_undo {
                // Item was removed by undo — stay at same cursor
                Some(UndoNavTarget::Inbox {
                    cursor: Some(index.saturating_sub(1).max(0)),
                })
            } else {
                Some(UndoNavTarget::Inbox {
                    cursor: Some(*index),
                })
            }
        }
        Operation::InboxDelete { index, .. } => {
            if is_undo {
                // Item was restored
                Some(UndoNavTarget::Inbox {
                    cursor: Some(*index),
                })
            } else {
                Some(UndoNavTarget::Inbox {
                    cursor: Some(index.saturating_sub(1).max(0)),
                })
            }
        }
        Operation::InboxTitleEdit { index, .. }
        | Operation::InboxTagsEdit { index, .. }
        | Operation::InboxNoteEdit { index, .. } => Some(UndoNavTarget::Inbox {
            cursor: Some(*index),
        }),
        Operation::InboxMove {
            old_index,
            new_index,
        } => {
            if is_undo {
                Some(UndoNavTarget::Inbox {
                    cursor: Some(*old_index),
                })
            } else {
                Some(UndoNavTarget::Inbox {
                    cursor: Some(*new_index),
                })
            }
        }
        Operation::InboxTriage {
            inbox_index,
            track_id,
            task_id,
            ..
        } => {
            if is_undo {
                // Item restored to inbox
                Some(UndoNavTarget::Inbox {
                    cursor: Some(*inbox_index),
                })
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
        Operation::SectionMove {
            track_id,
            task_id,
            from_section: _,
            to_section,
            from_index,
        } => {
            if is_undo {
                // Task moved back to original section — navigate to task in track view
                Some(UndoNavTarget::Task {
                    track_id: track_id.clone(),
                    task_id: task_id.clone(),
                    detail_region: None,
                    task_removed: false,
                    position_hint: Some(*from_index),
                })
            } else {
                // Task moved to target section (e.g., Done) — navigate to Recent view
                if *to_section == SectionKind::Done {
                    Some(UndoNavTarget::Recent { cursor: None })
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
        }
        Operation::Reopen {
            track_id, task_id, ..
        } => {
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
        Operation::TrackAdd { track_id } => Some(UndoNavTarget::TracksView {
            track_id: track_id.clone(),
        }),
        Operation::TrackNameEdit { track_id, .. }
        | Operation::TrackShelve { track_id, .. }
        | Operation::TrackArchive { track_id, .. }
        | Operation::TrackDelete { track_id, .. } => Some(UndoNavTarget::TracksView {
            track_id: track_id.clone(),
        }),
        Operation::TrackCcFocus {
            old_focus,
            new_focus,
        } => {
            let focus = if is_undo { old_focus } else { new_focus };
            Some(UndoNavTarget::TracksView {
                track_id: focus.clone().unwrap_or_default(),
            })
        }
        Operation::CrossTrackMove {
            source_track_id,
            target_track_id,
            task_id_old,
            task_id_new,
            ..
        } => {
            if is_undo {
                Some(UndoNavTarget::Task {
                    track_id: source_track_id.clone(),
                    task_id: task_id_old.clone(),
                    detail_region: None,
                    task_removed: false,
                    position_hint: None,
                })
            } else {
                Some(UndoNavTarget::Task {
                    track_id: target_track_id.clone(),
                    task_id: task_id_new.clone(),
                    detail_region: None,
                    task_removed: false,
                    position_hint: None,
                })
            }
        }
        Operation::Reparent {
            track_id,
            new_task_id,
            id_mappings,
            ..
        } => {
            if is_undo {
                // Navigate to the original root ID (first mapping's old_id)
                let old_root_id = id_mappings
                    .first()
                    .map(|(old, _)| old.clone())
                    .unwrap_or_default();
                Some(UndoNavTarget::Task {
                    track_id: track_id.clone(),
                    task_id: old_root_id,
                    detail_region: None,
                    task_removed: false,
                    position_hint: None,
                })
            } else {
                Some(UndoNavTarget::Task {
                    track_id: track_id.clone(),
                    task_id: new_task_id.clone(),
                    detail_region: None,
                    task_removed: false,
                    position_hint: None,
                })
            }
        }
        Operation::Bulk(ops) => {
            // Navigate to the first operation's target
            ops.first().and_then(|op| nav_target_for_op(op, is_undo))
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
        /// The title set during the add (so redo can restore it)
        title: String,
    },
    /// A new subtask was added
    SubtaskAdd {
        track_id: String,
        parent_id: String,
        task_id: String,
        /// The title set during the add (so redo can restore it)
        title: String,
    },
    /// A task was moved within its sibling list (top-level backlog or within a parent's subtasks)
    TaskMove {
        track_id: String,
        task_id: String,
        /// Parent task ID (None = top-level backlog)
        parent_id: Option<String>,
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
        /// The title set during the add (so redo can restore it)
        title: String,
    },
    /// An inbox item was deleted
    InboxDelete { index: usize, item: InboxItem },
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
    InboxMove { old_index: usize, new_index: usize },
    /// An inbox item's note/body was edited
    InboxNoteEdit {
        index: usize,
        old_body: Option<String>,
        new_body: Option<String>,
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
    /// A task was moved between sections (e.g., Backlog → Done after grace period)
    SectionMove {
        track_id: String,
        task_id: String,
        from_section: SectionKind,
        to_section: SectionKind,
        from_index: usize,
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
    /// A new track was created (in TUI)
    TrackAdd { track_id: String },
    /// A track's display name was edited
    TrackNameEdit {
        track_id: String,
        old_name: String,
        new_name: String,
    },
    /// A track's state was toggled between active and shelved
    TrackShelve {
        track_id: String,
        /// true = was active (now shelved), false = was shelved (now active)
        was_active: bool,
    },
    /// A track was archived
    TrackArchive { track_id: String, old_state: String },
    /// A track was deleted (empty track only)
    TrackDelete {
        track_id: String,
        track_name: String,
        old_state: String,
        prefix: Option<String>,
    },
    /// The cc-focus track was changed
    TrackCcFocus {
        old_focus: Option<String>,
        new_focus: Option<String>,
    },
    /// A task was moved to a different track
    CrossTrackMove {
        source_track_id: String,
        target_track_id: String,
        task_id_old: String,
        task_id_new: String,
        /// Index in source backlog (top-level) or parent.subtasks
        source_index: usize,
        /// Index in target backlog
        target_index: usize,
        /// Some if moving a subtask (promotion to top-level)
        source_parent_id: Option<String>,
        /// Original depth of the task before move
        old_depth: usize,
    },
    /// A task was reparented (moved to different parent/depth) with ID re-keying
    Reparent {
        track_id: String,
        new_task_id: String,
        old_parent_id: Option<String>,
        new_parent_id: Option<String>,
        old_sibling_index: usize,
        new_sibling_index: usize,
        old_depth: usize,
        id_mappings: Vec<(String, String)>, // (old_id, new_id)
    },
    /// A batch of operations applied as a single undo step (bulk SELECT mode actions)
    Bulk(Vec<Operation>),
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
        if self.undo.len() > UNDO_STACK_LIMIT {
            self.undo.drain(..self.undo.len() - UNDO_STACK_LIMIT);
        }
        self.redo.clear();
    }

    /// Push a sync marker. Clears the redo stack.
    pub fn push_sync_marker(&mut self) {
        self.undo.push(Operation::SyncMarker);
        if self.undo.len() > UNDO_STACK_LIMIT {
            self.undo.drain(..self.undo.len() - UNDO_STACK_LIMIT);
        }
        self.redo.clear();
    }

    /// Undo the last operation. Returns navigation target for the UI.
    /// Applies the inverse operation to the track data. Does NOT save to disk.
    pub fn undo(
        &mut self,
        tracks: &mut [(String, Track)],
        inbox: Option<&mut crate::model::inbox::Inbox>,
    ) -> Option<UndoNavTarget> {
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
    pub fn redo(
        &mut self,
        tracks: &mut [(String, Track)],
        inbox: Option<&mut crate::model::inbox::Inbox>,
    ) -> Option<UndoNavTarget> {
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
fn apply_inverse(
    op: &Operation,
    tracks: &mut [(String, Track)],
    inbox: Option<&mut crate::model::inbox::Inbox>,
) -> Option<String> {
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
            task.metadata.retain(|m| m.key() != "resolved");
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
            ..
        } => {
            // Undo subtask add = remove the subtask from parent
            let track = find_track_mut(tracks, track_id)?;
            let parent = task_ops::find_task_mut_in_track(track, parent_id)?;
            parent.subtasks.retain(|t| t.id.as_deref() != Some(task_id));
            parent.mark_dirty();
            Some(track_id.clone())
        }
        Operation::TaskMove {
            track_id,
            task_id,
            parent_id,
            old_index,
            ..
        } => {
            // Undo move = move back to old_index
            let track = find_track_mut(tracks, track_id)?;
            let siblings = if let Some(pid) = parent_id {
                &mut task_ops::find_task_mut_in_track(track, pid)?.subtasks
            } else {
                track.section_tasks_mut(SectionKind::Backlog)?
            };
            let cur = siblings
                .iter()
                .position(|t| t.id.as_deref() == Some(task_id))?;
            let task = siblings.remove(cur);
            let idx = (*old_index).min(siblings.len());
            siblings.insert(idx, task);
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
        // Track management operations are handled by the caller (need config + filesystem access)
        Operation::TrackAdd { .. }
        | Operation::TrackNameEdit { .. }
        | Operation::TrackShelve { .. }
        | Operation::TrackArchive { .. }
        | Operation::TrackDelete { .. }
        | Operation::TrackCcFocus { .. } => None,
        Operation::InboxAdd { index, .. } => {
            // Undo add = remove the item
            if let Some(inbox) = inbox
                && *index < inbox.items.len()
            {
                inbox.items.remove(*index);
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
        Operation::InboxTitleEdit {
            index, old_title, ..
        } => {
            if let Some(inbox) = inbox
                && let Some(item) = inbox.items.get_mut(*index)
            {
                item.title = old_title.clone();
                item.dirty = true;
            }
            None
        }
        Operation::InboxTagsEdit {
            index, old_tags, ..
        } => {
            if let Some(inbox) = inbox
                && let Some(item) = inbox.items.get_mut(*index)
            {
                item.tags = old_tags.clone();
                item.dirty = true;
            }
            None
        }
        Operation::InboxNoteEdit {
            index, old_body, ..
        } => {
            if let Some(inbox) = inbox
                && let Some(item) = inbox.items.get_mut(*index)
            {
                item.body = old_body.clone();
                item.dirty = true;
            }
            None
        }
        Operation::InboxMove {
            old_index,
            new_index,
        } => {
            // Undo move = move back from new_index to old_index
            if let Some(inbox) = inbox
                && *new_index < inbox.items.len()
            {
                let item = inbox.items.remove(*new_index);
                let idx = (*old_index).min(inbox.items.len());
                inbox.items.insert(idx, item);
            }
            None
        }
        Operation::InboxTriage {
            inbox_index,
            item,
            track_id,
            task_id,
        } => {
            // Undo triage = remove task from track, re-insert item into inbox
            let track = find_track_mut(tracks, track_id);
            if let Some(track) = track
                && let Some(tasks) = track.section_tasks_mut(SectionKind::Backlog)
            {
                tasks.retain(|t| t.id.as_deref() != Some(task_id));
            }
            if let Some(inbox) = inbox {
                let idx = (*inbox_index).min(inbox.items.len());
                inbox.items.insert(idx, item.clone());
            }
            // Return track_id so caller knows to save
            Some(track_id.clone())
        }
        Operation::SectionMove {
            track_id,
            task_id,
            from_section,
            to_section,
            from_index,
        } => {
            // Undo section move = move task back from to_section to from_section at from_index
            let track = find_track_mut(tracks, track_id)?;
            let task = {
                let dest = track.section_tasks_mut(*to_section)?;
                let idx = dest.iter().position(|t| t.id.as_deref() == Some(task_id))?;
                dest.remove(idx)
            };
            if let Some(source) = track.section_tasks_mut(*from_section) {
                let idx = (*from_index).min(source.len());
                source.insert(idx, task);
            }
            Some(track_id.clone())
        }
        Operation::Reopen {
            track_id,
            task_id,
            old_state,
            old_resolved,
            done_index,
        } => {
            // Undo reopen = restore state to Done. Task may be in Backlog (after flush)
            // or still in Done section (during grace period).
            let track = find_track_mut(tracks, track_id)?;

            // Try to find and remove from Backlog (post-flush case)
            let from_backlog = {
                if let Some(backlog) = track.section_tasks_mut(SectionKind::Backlog) {
                    backlog
                        .iter()
                        .position(|t| t.id.as_deref() == Some(task_id))
                        .map(|idx| backlog.remove(idx))
                } else {
                    None
                }
            };

            if let Some(mut task) = from_backlog {
                // Task was in Backlog — move back to Done at original index
                task.state = *old_state;
                task.metadata.retain(|m| m.key() != "resolved");
                if let Some(date) = old_resolved {
                    task.metadata
                        .push(crate::model::task::Metadata::Resolved(date.clone()));
                }
                task.mark_dirty();
                if let Some(done) = track.section_tasks_mut(SectionKind::Done) {
                    let idx = (*done_index).min(done.len());
                    done.insert(idx, task);
                }
            } else {
                // Task is still in Done section (grace period, pending move was cancelled)
                // Just restore the state and resolved date
                let task = task_ops::find_task_mut_in_track(track, task_id)?;
                task.state = *old_state;
                task.metadata.retain(|m| m.key() != "resolved");
                if let Some(date) = old_resolved {
                    task.metadata
                        .push(crate::model::task::Metadata::Resolved(date.clone()));
                }
                task.mark_dirty();
            }
            Some(track_id.clone())
        }
        Operation::CrossTrackMove {
            source_track_id,
            target_track_id,
            task_id_old,
            task_id_new,
            source_index,
            source_parent_id,
            old_depth,
            ..
        } => {
            // Undo: remove task from target, rename back to old ID, insert into source
            let target_track = find_track_mut(tracks, target_track_id)?;
            let target_tasks = target_track.section_tasks_mut(SectionKind::Backlog)?;
            let idx = target_tasks
                .iter()
                .position(|t| t.id.as_deref() == Some(task_id_new))?;
            let mut task = target_tasks.remove(idx);

            // Rename ID back
            task.id = Some(task_id_old.clone());
            task.mark_dirty();
            task_ops::renumber_subtasks(&mut task, task_id_old);

            if let Some(parent_id) = source_parent_id {
                // Was a subtask — restore depth and insert back as subtask
                task.depth = *old_depth;
                let source_track = find_track_mut(tracks, source_track_id)?;
                let parent = task_ops::find_task_mut_in_track(source_track, parent_id)?;
                let idx = (*source_index).min(parent.subtasks.len());
                parent.subtasks.insert(idx, task);
                parent.mark_dirty();
            } else {
                // Was top-level — restore depth and insert back into source backlog
                task.depth = *old_depth;
                let source_track = find_track_mut(tracks, source_track_id)?;
                let source_tasks = source_track.section_tasks_mut(SectionKind::Backlog)?;
                let idx = (*source_index).min(source_tasks.len());
                source_tasks.insert(idx, task);
            }

            // Update dep references back
            task_ops::update_dep_references(tracks, task_id_new, task_id_old);
            None // Both tracks saved by caller
        }
        Operation::Reparent {
            track_id,
            new_task_id,
            old_parent_id,
            old_sibling_index,
            old_depth,
            id_mappings,
            ..
        } => {
            // Undo reparent: find task by new_task_id, reverse rekey, restore to old location
            let track = find_track_mut(tracks, track_id)?;

            // Remove from current location
            let (mut task, _) = task_ops::remove_task_subtree(track, new_task_id)?;

            // Reverse ID mappings: rename new IDs back to old IDs
            for (old_id, new_id) in id_mappings.iter().rev() {
                reverse_rekey_task(&mut task, new_id, old_id);
            }

            // Restore depth
            task_ops::set_subtree_depth(&mut task, *old_depth);

            // Insert at old location
            let _ = task_ops::insert_task_subtree(
                track,
                task,
                old_parent_id.as_deref(),
                SectionKind::Backlog,
                *old_sibling_index,
            );

            // Reverse dep references across all tracks
            for (old_id, new_id) in id_mappings {
                task_ops::update_dep_references(tracks, new_id, old_id);
            }

            Some(track_id.clone())
        }
        Operation::Bulk(ops) => {
            // Apply inverse of each sub-operation in reverse order
            // Bulk operations don't involve inbox, so pass None for each sub-op
            let mut result = None;
            for op in ops.iter().rev() {
                if let Some(track_id) = apply_inverse(op, tracks, None) {
                    result = Some(track_id);
                }
            }
            result
        }
        Operation::SyncMarker => None,
    }
}

/// Reverse a single ID rename within a task tree (new_id -> old_id).
fn reverse_rekey_task(task: &mut Task, from_id: &str, to_id: &str) {
    if task.id.as_deref() == Some(from_id) {
        task.id = Some(to_id.to_string());
        task.mark_dirty();
    }
    for sub in &mut task.subtasks {
        reverse_rekey_task(sub, from_id, to_id);
    }
}

/// Apply an operation forward (for redo)
fn apply_forward(
    op: &Operation,
    tracks: &mut [(String, Track)],
    inbox: Option<&mut crate::model::inbox::Inbox>,
) -> Option<String> {
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
            task.metadata.retain(|m| m.key() != "resolved");
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
            title,
        } => {
            let track = find_track_mut(tracks, track_id)?;
            let tasks = track.section_tasks_mut(SectionKind::Backlog)?;
            let mut task = Task::new(TaskState::Todo, Some(task_id.clone()), title.clone());
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
            title,
        } => {
            let track = find_track_mut(tracks, track_id)?;
            let parent = task_ops::find_task_mut_in_track(track, parent_id)?;
            let mut sub = Task::new(TaskState::Todo, Some(task_id.clone()), title.clone());
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
            parent_id,
            new_index,
            ..
        } => {
            let track = find_track_mut(tracks, track_id)?;
            let siblings = if let Some(pid) = parent_id {
                &mut task_ops::find_task_mut_in_track(track, pid)?.subtasks
            } else {
                track.section_tasks_mut(SectionKind::Backlog)?
            };
            let cur = siblings
                .iter()
                .position(|t| t.id.as_deref() == Some(task_id))?;
            let task = siblings.remove(cur);
            let idx = (*new_index).min(siblings.len());
            siblings.insert(idx, task);
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
        // Track management operations are handled by the caller (need config + filesystem access)
        Operation::TrackAdd { .. }
        | Operation::TrackNameEdit { .. }
        | Operation::TrackShelve { .. }
        | Operation::TrackArchive { .. }
        | Operation::TrackDelete { .. }
        | Operation::TrackCcFocus { .. } => None,
        Operation::InboxAdd { index, title } => {
            if let Some(inbox) = inbox {
                let item = InboxItem::new(title.clone());
                let idx = (*index).min(inbox.items.len());
                inbox.items.insert(idx, item);
            }
            None
        }
        Operation::InboxDelete { index, .. } => {
            // Redo delete = remove the item again
            if let Some(inbox) = inbox
                && *index < inbox.items.len()
            {
                inbox.items.remove(*index);
            }
            None
        }
        Operation::InboxTitleEdit {
            index, new_title, ..
        } => {
            if let Some(inbox) = inbox
                && let Some(item) = inbox.items.get_mut(*index)
            {
                item.title = new_title.clone();
                item.dirty = true;
            }
            None
        }
        Operation::InboxTagsEdit {
            index, new_tags, ..
        } => {
            if let Some(inbox) = inbox
                && let Some(item) = inbox.items.get_mut(*index)
            {
                item.tags = new_tags.clone();
                item.dirty = true;
            }
            None
        }
        Operation::InboxNoteEdit {
            index, new_body, ..
        } => {
            if let Some(inbox) = inbox
                && let Some(item) = inbox.items.get_mut(*index)
            {
                item.body = new_body.clone();
                item.dirty = true;
            }
            None
        }
        Operation::InboxMove {
            old_index,
            new_index,
        } => {
            if let Some(inbox) = inbox
                && *old_index < inbox.items.len()
            {
                let item = inbox.items.remove(*old_index);
                let idx = (*new_index).min(inbox.items.len());
                inbox.items.insert(idx, item);
            }
            None
        }
        Operation::InboxTriage {
            inbox_index,
            track_id,
            task_id,
            item,
            ..
        } => {
            // Redo triage = remove from inbox, add task to track
            if let Some(inbox) = inbox
                && *inbox_index < inbox.items.len()
            {
                inbox.items.remove(*inbox_index);
            }
            // Re-create task in track
            let track = find_track_mut(tracks, track_id);
            if let Some(track) = track
                && let Some(tasks) = track.section_tasks_mut(SectionKind::Backlog)
            {
                let mut task =
                    Task::new(TaskState::Todo, Some(task_id.clone()), item.title.clone());
                task.tags = item.tags.clone();
                task.metadata.push(crate::model::task::Metadata::Added(
                    chrono::Local::now().format("%Y-%m-%d").to_string(),
                ));
                if let Some(body) = &item.body
                    && !body.is_empty()
                {
                    task.metadata
                        .push(crate::model::task::Metadata::Note(body.clone()));
                }
                tasks.push(task);
            }
            Some(track_id.clone())
        }
        Operation::SectionMove {
            track_id,
            task_id,
            from_section,
            to_section,
            ..
        } => {
            // Redo section move = move task from from_section to to_section
            let track = find_track_mut(tracks, track_id)?;
            let task = {
                let source = track.section_tasks_mut(*from_section)?;
                let idx = source
                    .iter()
                    .position(|t| t.id.as_deref() == Some(task_id))?;
                source.remove(idx)
            };
            if let Some(dest) = track.section_tasks_mut(*to_section) {
                dest.insert(0, task);
            }
            Some(track_id.clone())
        }
        Operation::Reopen {
            track_id, task_id, ..
        } => {
            // Redo reopen = set task to Todo (it's in Done section), then move to Backlog
            let track = find_track_mut(tracks, track_id)?;

            // Try to find in Done section first (normal case)
            let from_done = {
                if let Some(done) = track.section_tasks_mut(SectionKind::Done) {
                    done.iter()
                        .position(|t| t.id.as_deref() == Some(task_id))
                        .map(|idx| done.remove(idx))
                } else {
                    None
                }
            };

            if let Some(mut task) = from_done {
                task.state = TaskState::Todo;
                task.metadata.retain(|m| m.key() != "resolved");
                task.mark_dirty();
                if let Some(backlog) = track.section_tasks_mut(SectionKind::Backlog) {
                    backlog.insert(0, task);
                }
            } else {
                // Task might already be in Backlog (if pending move had been flushed after undo)
                // Just update its state
                let task = task_ops::find_task_mut_in_track(track, task_id)?;
                task.state = TaskState::Todo;
                task.metadata.retain(|m| m.key() != "resolved");
                task.mark_dirty();
            }
            Some(track_id.clone())
        }
        Operation::CrossTrackMove {
            source_track_id,
            target_track_id,
            task_id_old,
            task_id_new,
            target_index,
            source_parent_id,
            ..
        } => {
            // Redo: remove from source, rename to new ID, insert into target
            let task = if let Some(parent_id) = source_parent_id {
                // Was a subtask — remove from parent
                let source_track = find_track_mut(tracks, source_track_id)?;
                let parent = task_ops::find_task_mut_in_track(source_track, parent_id)?;
                let idx = parent
                    .subtasks
                    .iter()
                    .position(|t| t.id.as_deref() == Some(task_id_old))?;
                let task = parent.subtasks.remove(idx);
                parent.mark_dirty();
                task
            } else {
                let source_track = find_track_mut(tracks, source_track_id)?;
                let source_tasks = source_track.section_tasks_mut(SectionKind::Backlog)?;
                let idx = source_tasks
                    .iter()
                    .position(|t| t.id.as_deref() == Some(task_id_old))?;
                source_tasks.remove(idx)
            };

            let mut task = task;
            task.id = Some(task_id_new.clone());
            task.depth = 0;
            task.mark_dirty();
            task_ops::renumber_subtasks(&mut task, task_id_new);

            let target_track = find_track_mut(tracks, target_track_id)?;
            let target_tasks = target_track.section_tasks_mut(SectionKind::Backlog)?;
            let idx = (*target_index).min(target_tasks.len());
            target_tasks.insert(idx, task);

            // Update dep references
            task_ops::update_dep_references(tracks, task_id_old, task_id_new);
            None // Both tracks saved by caller
        }
        Operation::Reparent {
            track_id,
            new_task_id,
            new_parent_id,
            new_sibling_index,
            id_mappings,
            ..
        } => {
            // Redo reparent: find task by old root ID, apply forward rekey, move to new location
            let old_root_id = id_mappings
                .first()
                .map(|(old, _)| old.clone())
                .unwrap_or_default();

            let track = find_track_mut(tracks, track_id)?;

            // Remove from old location
            let (mut task, _) = task_ops::remove_task_subtree(track, &old_root_id)?;

            // Apply forward ID mappings
            let _ = task_ops::rekey_subtree(&mut task, new_task_id);

            // Compute new depth
            let new_depth = match new_parent_id {
                None => 0,
                Some(pid) => {
                    let parent = task_ops::find_task_in_track(track, pid);
                    parent.map_or(0, |p| p.depth + 1)
                }
            };
            task_ops::set_subtree_depth(&mut task, new_depth);

            // Insert at new location
            let _ = task_ops::insert_task_subtree(
                track,
                task,
                new_parent_id.as_deref(),
                SectionKind::Backlog,
                *new_sibling_index,
            );

            // Update dep references forward
            for (old_id, new_id) in id_mappings {
                task_ops::update_dep_references(tracks, old_id, new_id);
            }

            Some(track_id.clone())
        }
        Operation::Bulk(ops) => {
            // Apply each sub-operation forward in order
            let mut result = None;
            for op in ops.iter() {
                if let Some(track_id) = apply_forward(op, tracks, None) {
                    result = Some(track_id);
                }
            }
            result
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
            task.metadata
                .retain(|m| !matches!(m, crate::model::task::Metadata::Dep(_)));
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
            task.metadata
                .retain(|m| !matches!(m, crate::model::task::Metadata::Spec(_)));
            if !value.trim().is_empty() {
                task.metadata
                    .push(crate::model::task::Metadata::Spec(value.trim().to_string()));
            }
            task.mark_dirty();
        }
        "refs" => {
            task.metadata
                .retain(|m| !matches!(m, crate::model::task::Metadata::Ref(_)));
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
            task.metadata
                .retain(|m| !matches!(m, crate::model::task::Metadata::Note(_)));
            if !value.is_empty() {
                task.metadata
                    .push(crate::model::task::Metadata::Note(value.to_string()));
            }
            task.mark_dirty();
        }
        _ => {}
    }
}

fn find_track_mut<'a>(tracks: &'a mut [(String, Track)], track_id: &str) -> Option<&'a mut Track> {
    tracks
        .iter_mut()
        .find(|(id, _)| id == track_id)
        .map(|(_, track)| track)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::inbox::{Inbox, InboxItem};
    use crate::model::task::Metadata;
    use crate::parse::track_parser::parse_track;

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    fn sample_track() -> Track {
        parse_track(
            "# Test\n\n## Backlog\n\n- [ ] `T-001` First\n- [ ] `T-002` Second\n- [ ] `T-003` Third\n\n## Done\n",
        )
    }

    fn sample_inbox() -> Inbox {
        Inbox {
            header_lines: vec!["# Inbox".into()],
            items: vec![
                InboxItem::new("Item 1".into()),
                InboxItem::new("Item 2".into()),
            ],
            source_lines: vec![],
        }
    }

    fn tracks_vec(id: &str, track: Track) -> Vec<(String, Track)> {
        vec![(id.into(), track)]
    }

    /// Helper to unwrap Task variant fields
    fn expect_task(
        nav: UndoNavTarget,
    ) -> (String, String, Option<DetailRegion>, bool, Option<usize>) {
        match nav {
            UndoNavTarget::Task {
                track_id,
                task_id,
                detail_region,
                task_removed,
                position_hint,
            } => (
                track_id,
                task_id,
                detail_region,
                task_removed,
                position_hint,
            ),
            other => panic!("expected Task, got {:?}", other),
        }
    }

    fn expect_tracks_view(nav: UndoNavTarget) -> String {
        match nav {
            UndoNavTarget::TracksView { track_id } => track_id,
            other => panic!("expected TracksView, got {:?}", other),
        }
    }

    fn expect_inbox(nav: UndoNavTarget) -> Option<usize> {
        match nav {
            UndoNavTarget::Inbox { cursor } => cursor,
            other => panic!("expected Inbox, got {:?}", other),
        }
    }

    // -----------------------------------------------------------------------
    // UndoStack core
    // -----------------------------------------------------------------------

    #[test]
    fn new_stack_is_empty() {
        let stack = UndoStack::new();
        assert!(stack.is_empty());
        assert!(stack.peek_last_undo().is_none());
        assert!(stack.peek_last_redo().is_none());
    }

    #[test]
    fn push_adds_to_undo() {
        let mut stack = UndoStack::new();
        stack.push(Operation::StateChange {
            track_id: "t".into(),
            task_id: "T-001".into(),
            old_state: TaskState::Todo,
            new_state: TaskState::Active,
            old_resolved: None,
            new_resolved: None,
        });
        assert!(!stack.is_empty());
        assert!(stack.peek_last_undo().is_some());
    }

    #[test]
    fn push_clears_redo() {
        let mut stack = UndoStack::new();
        let mut tracks = tracks_vec("t", sample_track());
        stack.push(Operation::StateChange {
            track_id: "t".into(),
            task_id: "T-001".into(),
            old_state: TaskState::Todo,
            new_state: TaskState::Active,
            old_resolved: None,
            new_resolved: None,
        });
        stack.undo(&mut tracks, None);
        assert!(stack.peek_last_redo().is_some());
        // Pushing a new op should clear redo
        stack.push(Operation::TitleEdit {
            track_id: "t".into(),
            task_id: "T-001".into(),
            old_title: "old".into(),
            new_title: "new".into(),
        });
        assert!(stack.peek_last_redo().is_none());
    }

    #[test]
    fn stack_limit_enforcement() {
        let mut stack = UndoStack::new();
        for i in 0..=UNDO_STACK_LIMIT {
            stack.push(Operation::TitleEdit {
                track_id: "t".into(),
                task_id: format!("T-{:03}", i),
                old_title: "old".into(),
                new_title: "new".into(),
            });
        }
        // After pushing 501 items, the stack should be capped at 500
        assert_eq!(stack.undo.len(), UNDO_STACK_LIMIT);
    }

    #[test]
    fn peek_last_undo_after_push() {
        let mut stack = UndoStack::new();
        stack.push(Operation::TitleEdit {
            track_id: "t".into(),
            task_id: "T-001".into(),
            old_title: "a".into(),
            new_title: "b".into(),
        });
        assert!(matches!(
            stack.peek_last_undo(),
            Some(Operation::TitleEdit { .. })
        ));
    }

    #[test]
    fn peek_last_redo_after_undo() {
        let mut stack = UndoStack::new();
        let mut tracks = tracks_vec("t", sample_track());
        stack.push(Operation::TitleEdit {
            track_id: "t".into(),
            task_id: "T-001".into(),
            old_title: "First".into(),
            new_title: "Updated".into(),
        });
        stack.undo(&mut tracks, None);
        assert!(matches!(
            stack.peek_last_redo(),
            Some(Operation::TitleEdit { .. })
        ));
    }

    #[test]
    fn is_empty_after_undo() {
        let mut stack = UndoStack::new();
        let mut tracks = tracks_vec("t", sample_track());
        stack.push(Operation::TitleEdit {
            track_id: "t".into(),
            task_id: "T-001".into(),
            old_title: "First".into(),
            new_title: "Updated".into(),
        });
        stack.undo(&mut tracks, None);
        assert!(stack.is_empty());
    }

    #[test]
    fn undo_on_empty_stack_returns_none() {
        let mut stack = UndoStack::new();
        let mut tracks = tracks_vec("t", sample_track());
        assert!(stack.undo(&mut tracks, None).is_none());
    }

    #[test]
    fn redo_on_empty_stack_returns_none() {
        let mut stack = UndoStack::new();
        let mut tracks = tracks_vec("t", sample_track());
        assert!(stack.redo(&mut tracks, None).is_none());
    }

    // -----------------------------------------------------------------------
    // Sync marker blocking
    // -----------------------------------------------------------------------

    #[test]
    fn undo_stops_at_sync_marker() {
        let mut stack = UndoStack::new();
        let mut tracks = tracks_vec("t", sample_track());
        stack.push(Operation::TitleEdit {
            track_id: "t".into(),
            task_id: "T-001".into(),
            old_title: "First".into(),
            new_title: "Changed".into(),
        });
        stack.push_sync_marker();
        stack.push(Operation::TitleEdit {
            track_id: "t".into(),
            task_id: "T-002".into(),
            old_title: "Second".into(),
            new_title: "Changed2".into(),
        });
        // Undo the second edit
        let nav = stack.undo(&mut tracks, None);
        assert!(nav.is_some());
        // Next undo should hit the sync marker and return None
        let nav = stack.undo(&mut tracks, None);
        assert!(nav.is_none());
        // The sync marker should still be on the stack (put back)
        assert!(!stack.is_empty());
    }

    #[test]
    fn push_after_sync_clears_redo() {
        let mut stack = UndoStack::new();
        let mut tracks = tracks_vec("t", sample_track());
        stack.push(Operation::TitleEdit {
            track_id: "t".into(),
            task_id: "T-001".into(),
            old_title: "First".into(),
            new_title: "Changed".into(),
        });
        stack.undo(&mut tracks, None);
        assert!(stack.peek_last_redo().is_some());
        stack.push_sync_marker();
        assert!(stack.peek_last_redo().is_none());
    }

    #[test]
    fn sync_marker_limit_enforcement() {
        let mut stack = UndoStack::new();
        for _ in 0..=UNDO_STACK_LIMIT {
            stack.push_sync_marker();
        }
        assert_eq!(stack.undo.len(), UNDO_STACK_LIMIT);
    }

    // -----------------------------------------------------------------------
    // StateChange undo/redo
    // -----------------------------------------------------------------------

    #[test]
    fn state_change_undo_reverts() {
        let mut stack = UndoStack::new();
        let mut tracks = tracks_vec("t", sample_track());
        // Set T-001 to Active in the track
        {
            let track = &mut tracks[0].1;
            let task = task_ops::find_task_mut_in_track(track, "T-001").unwrap();
            task.state = TaskState::Active;
        }
        stack.push(Operation::StateChange {
            track_id: "t".into(),
            task_id: "T-001".into(),
            old_state: TaskState::Todo,
            new_state: TaskState::Active,
            old_resolved: None,
            new_resolved: None,
        });
        stack.undo(&mut tracks, None);
        let track = &tracks[0].1;
        let task = task_ops::find_task_in_track(track, "T-001").unwrap();
        assert_eq!(task.state, TaskState::Todo);
    }

    #[test]
    fn state_change_redo_reapplies() {
        let mut stack = UndoStack::new();
        let mut tracks = tracks_vec("t", sample_track());
        {
            let track = &mut tracks[0].1;
            let task = task_ops::find_task_mut_in_track(track, "T-001").unwrap();
            task.state = TaskState::Active;
        }
        stack.push(Operation::StateChange {
            track_id: "t".into(),
            task_id: "T-001".into(),
            old_state: TaskState::Todo,
            new_state: TaskState::Active,
            old_resolved: None,
            new_resolved: None,
        });
        stack.undo(&mut tracks, None);
        stack.redo(&mut tracks, None);
        let track = &tracks[0].1;
        let task = task_ops::find_task_in_track(track, "T-001").unwrap();
        assert_eq!(task.state, TaskState::Active);
    }

    #[test]
    fn state_change_with_resolved_date() {
        let mut stack = UndoStack::new();
        let mut tracks = tracks_vec("t", sample_track());
        {
            let track = &mut tracks[0].1;
            let task = task_ops::find_task_mut_in_track(track, "T-001").unwrap();
            task.state = TaskState::Done;
            task.metadata.push(Metadata::Resolved("2026-02-10".into()));
        }
        stack.push(Operation::StateChange {
            track_id: "t".into(),
            task_id: "T-001".into(),
            old_state: TaskState::Todo,
            new_state: TaskState::Done,
            old_resolved: None,
            new_resolved: Some("2026-02-10".into()),
        });
        // Undo should remove the resolved date
        stack.undo(&mut tracks, None);
        let track = &tracks[0].1;
        let task = task_ops::find_task_in_track(track, "T-001").unwrap();
        assert_eq!(task.state, TaskState::Todo);
        assert!(!task.metadata.iter().any(|m| m.key() == "resolved"));
    }

    // -----------------------------------------------------------------------
    // TitleEdit undo/redo
    // -----------------------------------------------------------------------

    #[test]
    fn title_edit_undo_restores_old() {
        let mut stack = UndoStack::new();
        let mut tracks = tracks_vec("t", sample_track());
        {
            let track = &mut tracks[0].1;
            let task = task_ops::find_task_mut_in_track(track, "T-001").unwrap();
            task.title = "Updated".into();
        }
        stack.push(Operation::TitleEdit {
            track_id: "t".into(),
            task_id: "T-001".into(),
            old_title: "First".into(),
            new_title: "Updated".into(),
        });
        stack.undo(&mut tracks, None);
        let track = &tracks[0].1;
        let task = task_ops::find_task_in_track(track, "T-001").unwrap();
        assert_eq!(task.title, "First");
    }

    #[test]
    fn title_edit_redo_applies_new() {
        let mut stack = UndoStack::new();
        let mut tracks = tracks_vec("t", sample_track());
        {
            let track = &mut tracks[0].1;
            let task = task_ops::find_task_mut_in_track(track, "T-001").unwrap();
            task.title = "Updated".into();
        }
        stack.push(Operation::TitleEdit {
            track_id: "t".into(),
            task_id: "T-001".into(),
            old_title: "First".into(),
            new_title: "Updated".into(),
        });
        stack.undo(&mut tracks, None);
        stack.redo(&mut tracks, None);
        let track = &tracks[0].1;
        let task = task_ops::find_task_in_track(track, "T-001").unwrap();
        assert_eq!(task.title, "Updated");
    }

    // -----------------------------------------------------------------------
    // TaskAdd undo/redo
    // -----------------------------------------------------------------------

    #[test]
    fn task_add_undo_removes_task() {
        let mut stack = UndoStack::new();
        let mut tracks = tracks_vec("t", sample_track());
        // Simulate adding a task
        {
            let track = &mut tracks[0].1;
            let tasks = track.section_tasks_mut(SectionKind::Backlog).unwrap();
            let mut task = Task::new(TaskState::Todo, Some("T-004".into()), "New task".into());
            task.metadata.push(Metadata::Added("2026-02-10".into()));
            tasks.push(task);
        }
        stack.push(Operation::TaskAdd {
            track_id: "t".into(),
            task_id: "T-004".into(),
            position_index: 3,
            title: "New task".into(),
        });
        stack.undo(&mut tracks, None);
        let track = &tracks[0].1;
        assert_eq!(track.backlog().len(), 3); // back to original 3
        assert!(task_ops::find_task_in_track(track, "T-004").is_none());
    }

    #[test]
    fn task_add_redo_reinserts() {
        let mut stack = UndoStack::new();
        let mut tracks = tracks_vec("t", sample_track());
        {
            let track = &mut tracks[0].1;
            let tasks = track.section_tasks_mut(SectionKind::Backlog).unwrap();
            let task = Task::new(TaskState::Todo, Some("T-004".into()), "New task".into());
            tasks.push(task);
        }
        stack.push(Operation::TaskAdd {
            track_id: "t".into(),
            task_id: "T-004".into(),
            position_index: 3,
            title: "New task".into(),
        });
        stack.undo(&mut tracks, None);
        stack.redo(&mut tracks, None);
        let track = &tracks[0].1;
        assert!(task_ops::find_task_in_track(track, "T-004").is_some());
    }

    // -----------------------------------------------------------------------
    // SubtaskAdd undo/redo
    // -----------------------------------------------------------------------

    #[test]
    fn subtask_add_undo_removes_from_parent() {
        let mut stack = UndoStack::new();
        let mut tracks = tracks_vec("t", sample_track());
        // Add a subtask to T-001
        {
            let track = &mut tracks[0].1;
            let parent = task_ops::find_task_mut_in_track(track, "T-001").unwrap();
            let mut sub = Task::new(TaskState::Todo, Some("T-001.1".into()), "Sub".into());
            sub.depth = 1;
            parent.subtasks.push(sub);
        }
        stack.push(Operation::SubtaskAdd {
            track_id: "t".into(),
            parent_id: "T-001".into(),
            task_id: "T-001.1".into(),
            title: "Sub".into(),
        });
        stack.undo(&mut tracks, None);
        let track = &tracks[0].1;
        let parent = task_ops::find_task_in_track(track, "T-001").unwrap();
        assert!(parent.subtasks.is_empty());
    }

    #[test]
    fn subtask_add_redo_reinserts() {
        let mut stack = UndoStack::new();
        let mut tracks = tracks_vec("t", sample_track());
        {
            let track = &mut tracks[0].1;
            let parent = task_ops::find_task_mut_in_track(track, "T-001").unwrap();
            let mut sub = Task::new(TaskState::Todo, Some("T-001.1".into()), "Sub".into());
            sub.depth = 1;
            parent.subtasks.push(sub);
        }
        stack.push(Operation::SubtaskAdd {
            track_id: "t".into(),
            parent_id: "T-001".into(),
            task_id: "T-001.1".into(),
            title: "Sub".into(),
        });
        stack.undo(&mut tracks, None);
        stack.redo(&mut tracks, None);
        let track = &tracks[0].1;
        let parent = task_ops::find_task_in_track(track, "T-001").unwrap();
        assert_eq!(parent.subtasks.len(), 1);
        assert_eq!(parent.subtasks[0].id.as_deref(), Some("T-001.1"));
    }

    // -----------------------------------------------------------------------
    // TaskMove undo/redo
    // -----------------------------------------------------------------------

    #[test]
    fn task_move_undo_restores_position() {
        let mut stack = UndoStack::new();
        let mut tracks = tracks_vec("t", sample_track());
        // Move T-001 from index 0 to index 2
        {
            let track = &mut tracks[0].1;
            let tasks = track.section_tasks_mut(SectionKind::Backlog).unwrap();
            let task = tasks.remove(0);
            tasks.insert(2, task);
        }
        stack.push(Operation::TaskMove {
            track_id: "t".into(),
            task_id: "T-001".into(),
            parent_id: None,
            old_index: 0,
            new_index: 2,
        });
        stack.undo(&mut tracks, None);
        let track = &tracks[0].1;
        let tasks = track.backlog();
        assert_eq!(tasks[0].id.as_deref(), Some("T-001"));
    }

    #[test]
    fn task_move_redo_applies() {
        let mut stack = UndoStack::new();
        let mut tracks = tracks_vec("t", sample_track());
        {
            let track = &mut tracks[0].1;
            let tasks = track.section_tasks_mut(SectionKind::Backlog).unwrap();
            let task = tasks.remove(0);
            tasks.insert(2, task);
        }
        stack.push(Operation::TaskMove {
            track_id: "t".into(),
            task_id: "T-001".into(),
            parent_id: None,
            old_index: 0,
            new_index: 2,
        });
        stack.undo(&mut tracks, None);
        stack.redo(&mut tracks, None);
        let track = &tracks[0].1;
        let tasks = track.backlog();
        assert_eq!(tasks[2].id.as_deref(), Some("T-001"));
    }

    #[test]
    fn task_move_position_clamping() {
        let mut stack = UndoStack::new();
        let mut tracks = tracks_vec("t", sample_track());
        // Move T-003 (index 2) with old_index=99 (out of range, should clamp)
        {
            let track = &mut tracks[0].1;
            let tasks = track.section_tasks_mut(SectionKind::Backlog).unwrap();
            let task = tasks.remove(2);
            tasks.insert(0, task);
        }
        stack.push(Operation::TaskMove {
            track_id: "t".into(),
            task_id: "T-003".into(),
            parent_id: None,
            old_index: 99, // should clamp to end
            new_index: 0,
        });
        stack.undo(&mut tracks, None);
        let track = &tracks[0].1;
        let tasks = track.backlog();
        // T-003 should be at the end (clamped to len)
        assert_eq!(tasks.last().unwrap().id.as_deref(), Some("T-003"));
    }

    // -----------------------------------------------------------------------
    // FieldEdit undo/redo (apply_field_value)
    // -----------------------------------------------------------------------

    #[test]
    fn field_edit_tags_undo_redo() {
        let mut stack = UndoStack::new();
        let mut tracks = tracks_vec("t", sample_track());
        {
            let track = &mut tracks[0].1;
            let task = task_ops::find_task_mut_in_track(track, "T-001").unwrap();
            task.tags = vec!["new".into()];
        }
        stack.push(Operation::FieldEdit {
            track_id: "t".into(),
            task_id: "T-001".into(),
            field: "tags".into(),
            old_value: "".into(),
            new_value: "#new".into(),
        });
        stack.undo(&mut tracks, None);
        let task = task_ops::find_task_in_track(&tracks[0].1, "T-001").unwrap();
        assert!(task.tags.is_empty());

        stack.redo(&mut tracks, None);
        let task = task_ops::find_task_in_track(&tracks[0].1, "T-001").unwrap();
        assert_eq!(task.tags, vec!["new"]);
    }

    #[test]
    fn field_edit_deps() {
        let mut stack = UndoStack::new();
        let mut tracks = tracks_vec("t", sample_track());
        {
            let track = &mut tracks[0].1;
            let task = task_ops::find_task_mut_in_track(track, "T-001").unwrap();
            task.metadata.push(Metadata::Dep(vec!["T-002".into()]));
        }
        stack.push(Operation::FieldEdit {
            track_id: "t".into(),
            task_id: "T-001".into(),
            field: "deps".into(),
            old_value: "".into(),
            new_value: "T-002".into(),
        });
        stack.undo(&mut tracks, None);
        let task = task_ops::find_task_in_track(&tracks[0].1, "T-001").unwrap();
        assert!(!task.metadata.iter().any(|m| matches!(m, Metadata::Dep(_))));
    }

    #[test]
    fn field_edit_spec() {
        let mut stack = UndoStack::new();
        let mut tracks = tracks_vec("t", sample_track());
        {
            let track = &mut tracks[0].1;
            let task = task_ops::find_task_mut_in_track(track, "T-001").unwrap();
            task.metadata.push(Metadata::Spec("doc/spec.md".into()));
        }
        stack.push(Operation::FieldEdit {
            track_id: "t".into(),
            task_id: "T-001".into(),
            field: "spec".into(),
            old_value: "".into(),
            new_value: "doc/spec.md".into(),
        });
        stack.undo(&mut tracks, None);
        let task = task_ops::find_task_in_track(&tracks[0].1, "T-001").unwrap();
        assert!(!task.metadata.iter().any(|m| matches!(m, Metadata::Spec(_))));
    }

    #[test]
    fn field_edit_refs() {
        let mut stack = UndoStack::new();
        let mut tracks = tracks_vec("t", sample_track());
        {
            let track = &mut tracks[0].1;
            let task = task_ops::find_task_mut_in_track(track, "T-001").unwrap();
            task.metadata.push(Metadata::Ref(vec!["file.md".into()]));
        }
        stack.push(Operation::FieldEdit {
            track_id: "t".into(),
            task_id: "T-001".into(),
            field: "refs".into(),
            old_value: "".into(),
            new_value: "file.md".into(),
        });
        stack.undo(&mut tracks, None);
        let task = task_ops::find_task_in_track(&tracks[0].1, "T-001").unwrap();
        assert!(!task.metadata.iter().any(|m| matches!(m, Metadata::Ref(_))));
    }

    #[test]
    fn field_edit_note() {
        let mut stack = UndoStack::new();
        let mut tracks = tracks_vec("t", sample_track());
        {
            let track = &mut tracks[0].1;
            let task = task_ops::find_task_mut_in_track(track, "T-001").unwrap();
            task.metadata.push(Metadata::Note("Hello world".into()));
        }
        stack.push(Operation::FieldEdit {
            track_id: "t".into(),
            task_id: "T-001".into(),
            field: "note".into(),
            old_value: "".into(),
            new_value: "Hello world".into(),
        });
        stack.undo(&mut tracks, None);
        let task = task_ops::find_task_in_track(&tracks[0].1, "T-001").unwrap();
        assert!(!task.metadata.iter().any(|m| matches!(m, Metadata::Note(_))));

        stack.redo(&mut tracks, None);
        let task = task_ops::find_task_in_track(&tracks[0].1, "T-001").unwrap();
        let note = task.metadata.iter().find_map(|m| match m {
            Metadata::Note(n) => Some(n.as_str()),
            _ => None,
        });
        assert_eq!(note, Some("Hello world"));
    }

    // -----------------------------------------------------------------------
    // Inbox ops undo/redo
    // -----------------------------------------------------------------------

    #[test]
    fn inbox_add_undo_removes() {
        let mut stack = UndoStack::new();
        let mut tracks: Vec<(String, Track)> = vec![];
        let mut inbox = sample_inbox();
        inbox.items.push(InboxItem::new("Item 3".into()));
        stack.push(Operation::InboxAdd {
            index: 2,
            title: "Item 3".into(),
        });
        stack.undo(&mut tracks, Some(&mut inbox));
        assert_eq!(inbox.items.len(), 2);
    }

    #[test]
    fn inbox_add_redo_reinserts() {
        let mut stack = UndoStack::new();
        let mut tracks: Vec<(String, Track)> = vec![];
        let mut inbox = sample_inbox();
        inbox.items.push(InboxItem::new("Item 3".into()));
        stack.push(Operation::InboxAdd {
            index: 2,
            title: "Item 3".into(),
        });
        stack.undo(&mut tracks, Some(&mut inbox));
        stack.redo(&mut tracks, Some(&mut inbox));
        assert_eq!(inbox.items.len(), 3);
        assert_eq!(inbox.items[2].title, "Item 3");
    }

    #[test]
    fn inbox_delete_undo_restores() {
        let mut stack = UndoStack::new();
        let mut tracks: Vec<(String, Track)> = vec![];
        let mut inbox = sample_inbox();
        let deleted = inbox.items.remove(0);
        stack.push(Operation::InboxDelete {
            index: 0,
            item: deleted,
        });
        stack.undo(&mut tracks, Some(&mut inbox));
        assert_eq!(inbox.items.len(), 2);
        assert_eq!(inbox.items[0].title, "Item 1");
    }

    #[test]
    fn inbox_title_edit_undo_redo() {
        let mut stack = UndoStack::new();
        let mut tracks: Vec<(String, Track)> = vec![];
        let mut inbox = sample_inbox();
        inbox.items[0].title = "Edited".into();
        stack.push(Operation::InboxTitleEdit {
            index: 0,
            old_title: "Item 1".into(),
            new_title: "Edited".into(),
        });
        stack.undo(&mut tracks, Some(&mut inbox));
        assert_eq!(inbox.items[0].title, "Item 1");

        stack.redo(&mut tracks, Some(&mut inbox));
        assert_eq!(inbox.items[0].title, "Edited");
    }

    #[test]
    fn inbox_tags_edit_undo_redo() {
        let mut stack = UndoStack::new();
        let mut tracks: Vec<(String, Track)> = vec![];
        let mut inbox = sample_inbox();
        inbox.items[0].tags = vec!["design".into()];
        stack.push(Operation::InboxTagsEdit {
            index: 0,
            old_tags: vec![],
            new_tags: vec!["design".into()],
        });
        stack.undo(&mut tracks, Some(&mut inbox));
        assert!(inbox.items[0].tags.is_empty());

        stack.redo(&mut tracks, Some(&mut inbox));
        assert_eq!(inbox.items[0].tags, vec!["design"]);
    }

    #[test]
    fn inbox_note_edit_undo_redo() {
        let mut stack = UndoStack::new();
        let mut tracks: Vec<(String, Track)> = vec![];
        let mut inbox = sample_inbox();
        inbox.items[0].body = Some("A note".into());
        stack.push(Operation::InboxNoteEdit {
            index: 0,
            old_body: None,
            new_body: Some("A note".into()),
        });
        stack.undo(&mut tracks, Some(&mut inbox));
        assert!(inbox.items[0].body.is_none());

        stack.redo(&mut tracks, Some(&mut inbox));
        assert_eq!(inbox.items[0].body.as_deref(), Some("A note"));
    }

    #[test]
    fn inbox_move_undo_redo() {
        let mut stack = UndoStack::new();
        let mut tracks: Vec<(String, Track)> = vec![];
        let mut inbox = sample_inbox();
        // Move item 0 to index 1
        let item = inbox.items.remove(0);
        inbox.items.insert(1, item);
        stack.push(Operation::InboxMove {
            old_index: 0,
            new_index: 1,
        });
        // After move: [Item 2, Item 1]
        assert_eq!(inbox.items[0].title, "Item 2");
        assert_eq!(inbox.items[1].title, "Item 1");

        stack.undo(&mut tracks, Some(&mut inbox));
        // After undo: [Item 1, Item 2]
        assert_eq!(inbox.items[0].title, "Item 1");
        assert_eq!(inbox.items[1].title, "Item 2");

        stack.redo(&mut tracks, Some(&mut inbox));
        // After redo: [Item 2, Item 1]
        assert_eq!(inbox.items[0].title, "Item 2");
        assert_eq!(inbox.items[1].title, "Item 1");
    }

    // -----------------------------------------------------------------------
    // SectionMove undo/redo
    // -----------------------------------------------------------------------

    #[test]
    fn section_move_undo_restores_position() {
        let mut stack = UndoStack::new();
        let mut tracks = tracks_vec("t", sample_track());
        // Move T-001 from Backlog (index 0) to Done
        {
            let track = &mut tracks[0].1;
            let tasks = track.section_tasks_mut(SectionKind::Backlog).unwrap();
            let task = tasks.remove(0);
            let done = track.section_tasks_mut(SectionKind::Done).unwrap();
            done.insert(0, task);
        }
        stack.push(Operation::SectionMove {
            track_id: "t".into(),
            task_id: "T-001".into(),
            from_section: SectionKind::Backlog,
            to_section: SectionKind::Done,
            from_index: 0,
        });
        stack.undo(&mut tracks, None);
        let track = &tracks[0].1;
        // T-001 should be back in backlog at position 0
        assert_eq!(track.backlog()[0].id.as_deref(), Some("T-001"));
        assert!(
            track
                .done()
                .iter()
                .all(|t| t.id.as_deref() != Some("T-001"))
        );
    }

    #[test]
    fn section_move_redo() {
        let mut stack = UndoStack::new();
        let mut tracks = tracks_vec("t", sample_track());
        {
            let track = &mut tracks[0].1;
            let tasks = track.section_tasks_mut(SectionKind::Backlog).unwrap();
            let task = tasks.remove(0);
            let done = track.section_tasks_mut(SectionKind::Done).unwrap();
            done.insert(0, task);
        }
        stack.push(Operation::SectionMove {
            track_id: "t".into(),
            task_id: "T-001".into(),
            from_section: SectionKind::Backlog,
            to_section: SectionKind::Done,
            from_index: 0,
        });
        stack.undo(&mut tracks, None);
        stack.redo(&mut tracks, None);
        let track = &tracks[0].1;
        // T-001 should be in Done
        assert!(
            track
                .done()
                .iter()
                .any(|t| t.id.as_deref() == Some("T-001"))
        );
    }

    // -----------------------------------------------------------------------
    // Bulk undo/redo
    // -----------------------------------------------------------------------

    #[test]
    fn bulk_undo_reverses_order() {
        let mut stack = UndoStack::new();
        let mut tracks = tracks_vec("t", sample_track());
        // Apply two state changes as a bulk op
        {
            let track = &mut tracks[0].1;
            let t1 = task_ops::find_task_mut_in_track(track, "T-001").unwrap();
            t1.state = TaskState::Active;
        }
        {
            let track = &mut tracks[0].1;
            let t2 = task_ops::find_task_mut_in_track(track, "T-002").unwrap();
            t2.state = TaskState::Done;
        }
        stack.push(Operation::Bulk(vec![
            Operation::StateChange {
                track_id: "t".into(),
                task_id: "T-001".into(),
                old_state: TaskState::Todo,
                new_state: TaskState::Active,
                old_resolved: None,
                new_resolved: None,
            },
            Operation::StateChange {
                track_id: "t".into(),
                task_id: "T-002".into(),
                old_state: TaskState::Todo,
                new_state: TaskState::Done,
                old_resolved: None,
                new_resolved: None,
            },
        ]));
        stack.undo(&mut tracks, None);
        let track = &tracks[0].1;
        assert_eq!(
            task_ops::find_task_in_track(track, "T-001").unwrap().state,
            TaskState::Todo
        );
        assert_eq!(
            task_ops::find_task_in_track(track, "T-002").unwrap().state,
            TaskState::Todo
        );
    }

    #[test]
    fn bulk_redo_applies_forward_order() {
        let mut stack = UndoStack::new();
        let mut tracks = tracks_vec("t", sample_track());
        {
            let track = &mut tracks[0].1;
            let t1 = task_ops::find_task_mut_in_track(track, "T-001").unwrap();
            t1.state = TaskState::Active;
        }
        {
            let track = &mut tracks[0].1;
            let t2 = task_ops::find_task_mut_in_track(track, "T-002").unwrap();
            t2.state = TaskState::Done;
        }
        stack.push(Operation::Bulk(vec![
            Operation::StateChange {
                track_id: "t".into(),
                task_id: "T-001".into(),
                old_state: TaskState::Todo,
                new_state: TaskState::Active,
                old_resolved: None,
                new_resolved: None,
            },
            Operation::StateChange {
                track_id: "t".into(),
                task_id: "T-002".into(),
                old_state: TaskState::Todo,
                new_state: TaskState::Done,
                old_resolved: None,
                new_resolved: None,
            },
        ]));
        stack.undo(&mut tracks, None);
        stack.redo(&mut tracks, None);
        let track = &tracks[0].1;
        assert_eq!(
            task_ops::find_task_in_track(track, "T-001").unwrap().state,
            TaskState::Active
        );
        assert_eq!(
            task_ops::find_task_in_track(track, "T-002").unwrap().state,
            TaskState::Done
        );
    }

    // -----------------------------------------------------------------------
    // Error / boundary cases
    // -----------------------------------------------------------------------

    #[test]
    fn undo_nonexistent_track_returns_none() {
        let mut stack = UndoStack::new();
        let mut tracks = tracks_vec("t", sample_track());
        stack.push(Operation::StateChange {
            track_id: "nonexistent".into(),
            task_id: "T-001".into(),
            old_state: TaskState::Todo,
            new_state: TaskState::Active,
            old_resolved: None,
            new_resolved: None,
        });
        // Undo returns a nav target, but the apply_inverse fails silently
        let nav = stack.undo(&mut tracks, None);
        assert!(nav.is_some()); // nav target is generated before applying
    }

    #[test]
    fn undo_nonexistent_task_returns_nav() {
        let mut stack = UndoStack::new();
        let mut tracks = tracks_vec("t", sample_track());
        stack.push(Operation::StateChange {
            track_id: "t".into(),
            task_id: "NOPE".into(),
            old_state: TaskState::Todo,
            new_state: TaskState::Active,
            old_resolved: None,
            new_resolved: None,
        });
        let nav = stack.undo(&mut tracks, None);
        assert!(nav.is_some());
    }

    #[test]
    fn inbox_op_with_none_inbox_is_safe() {
        let mut stack = UndoStack::new();
        let mut tracks: Vec<(String, Track)> = vec![];
        stack.push(Operation::InboxAdd {
            index: 0,
            title: "Test".into(),
        });
        // Passing None for inbox should not panic
        let nav = stack.undo(&mut tracks, None);
        assert!(nav.is_some());
    }

    // -----------------------------------------------------------------------
    // nav_target_for_op (existing tests, preserved)
    // -----------------------------------------------------------------------

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
        let (_, task_id, _, task_removed, _) = expect_task(nav_target_for_op(&op, false).unwrap());
        assert_eq!(task_id, "T-002");
        assert!(!task_removed);
    }

    #[test]
    fn nav_target_task_add_undo_removes() {
        let op = Operation::TaskAdd {
            track_id: "t1".into(),
            task_id: "T-003".into(),
            position_index: 2,
            title: "Test task".into(),
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
            title: "Test task".into(),
        };
        let (_, task_id, _, task_removed, _) = expect_task(nav_target_for_op(&op, false).unwrap());
        assert!(!task_removed);
        assert_eq!(task_id, "T-003");
    }

    #[test]
    fn nav_target_subtask_add_undo_goes_to_parent() {
        let op = Operation::SubtaskAdd {
            track_id: "t1".into(),
            parent_id: "T-010".into(),
            task_id: "T-010.1".into(),
            title: "Sub task".into(),
        };
        let (_, task_id, _, task_removed, _) = expect_task(nav_target_for_op(&op, true).unwrap());
        assert_eq!(task_id, "T-010");
        assert!(!task_removed);
    }

    #[test]
    fn nav_target_subtask_add_redo_goes_to_subtask() {
        let op = Operation::SubtaskAdd {
            track_id: "t1".into(),
            parent_id: "T-010".into(),
            task_id: "T-010.1".into(),
            title: "Sub task".into(),
        };
        let (_, task_id, _, _, _) = expect_task(nav_target_for_op(&op, false).unwrap());
        assert_eq!(task_id, "T-010.1");
    }

    #[test]
    fn nav_target_task_move() {
        let op = Operation::TaskMove {
            track_id: "t1".into(),
            task_id: "T-005".into(),
            parent_id: None,
            old_index: 0,
            new_index: 3,
        };
        let (_, task_id, detail_region, _, _) = expect_task(nav_target_for_op(&op, true).unwrap());
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
        let (_, _, detail_region, _, _) = expect_task(nav_target_for_op(&op, true).unwrap());
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
        let (_, _, detail_region, _, _) = expect_task(nav_target_for_op(&op, false).unwrap());
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
        let (_, _, detail_region, _, _) = expect_task(nav_target_for_op(&op, true).unwrap());
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
        let (_, _, detail_region, _, _) = expect_task(nav_target_for_op(&op, true).unwrap());
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
        let (_, _, detail_region, _, _) = expect_task(nav_target_for_op(&op, true).unwrap());
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

    #[test]
    fn nav_target_inbox_add_undo() {
        let op = Operation::InboxAdd {
            index: 3,
            title: "item".into(),
        };
        let cursor = expect_inbox(nav_target_for_op(&op, true).unwrap());
        assert_eq!(cursor, Some(2)); // index-1 for undo
    }

    #[test]
    fn nav_target_inbox_add_redo() {
        let op = Operation::InboxAdd {
            index: 3,
            title: "item".into(),
        };
        let cursor = expect_inbox(nav_target_for_op(&op, false).unwrap());
        assert_eq!(cursor, Some(3));
    }

    #[test]
    fn nav_target_inbox_delete_undo() {
        let op = Operation::InboxDelete {
            index: 1,
            item: InboxItem::new("deleted".into()),
        };
        let cursor = expect_inbox(nav_target_for_op(&op, true).unwrap());
        assert_eq!(cursor, Some(1)); // restored at index
    }

    #[test]
    fn nav_target_inbox_move_undo() {
        let op = Operation::InboxMove {
            old_index: 0,
            new_index: 2,
        };
        let cursor = expect_inbox(nav_target_for_op(&op, true).unwrap());
        assert_eq!(cursor, Some(0)); // goes back to old_index
    }

    #[test]
    fn nav_target_inbox_move_redo() {
        let op = Operation::InboxMove {
            old_index: 0,
            new_index: 2,
        };
        let cursor = expect_inbox(nav_target_for_op(&op, false).unwrap());
        assert_eq!(cursor, Some(2)); // goes to new_index
    }
}
