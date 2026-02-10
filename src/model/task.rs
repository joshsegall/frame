use serde::{Deserialize, Serialize};
use std::ops::Range;

/// Task checkbox state
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskState {
    Todo,
    Active,
    Blocked,
    Done,
    Parked,
}

impl TaskState {
    /// The character used inside the checkbox `[ ]`
    pub fn checkbox_char(self) -> char {
        match self {
            TaskState::Todo => ' ',
            TaskState::Active => '>',
            TaskState::Blocked => '-',
            TaskState::Done => 'x',
            TaskState::Parked => '~',
        }
    }

    /// Parse a checkbox character into a state
    pub fn from_checkbox_char(c: char) -> Option<TaskState> {
        match c {
            ' ' => Some(TaskState::Todo),
            '>' => Some(TaskState::Active),
            '-' => Some(TaskState::Blocked),
            'x' => Some(TaskState::Done),
            '~' => Some(TaskState::Parked),
            _ => None,
        }
    }
}

/// A single metadata entry on a task
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value")]
pub enum Metadata {
    /// `dep: EFF-003, INFRA-007`
    Dep(Vec<String>),
    /// `ref: path/to/file`
    Ref(Vec<String>),
    /// `spec: path/to/spec#section`
    Spec(String),
    /// `note:` followed by block text
    Note(String),
    /// `added: 2025-05-14`
    Added(String),
    /// `resolved: 2025-05-14`
    Resolved(String),
}

impl Metadata {
    /// Returns the key name for this metadata variant
    pub fn key(&self) -> &'static str {
        match self {
            Metadata::Dep(_) => "dep",
            Metadata::Ref(_) => "ref",
            Metadata::Spec(_) => "spec",
            Metadata::Note(_) => "note",
            Metadata::Added(_) => "added",
            Metadata::Resolved(_) => "resolved",
        }
    }
}

/// A task with all its parsed fields and source tracking
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    /// Checkbox state
    pub state: TaskState,
    /// Optional task ID like `EFF-014` or `EFF-014.2`
    pub id: Option<String>,
    /// Task title text
    pub title: String,
    /// Tags (without the `#` prefix)
    pub tags: Vec<String>,
    /// Metadata entries in order
    pub metadata: Vec<Metadata>,
    /// Subtasks (recursive)
    pub subtasks: Vec<Task>,
    /// Nesting depth (0 = top-level)
    pub depth: usize,

    // --- Source tracking ---
    /// Line range in the original source file (0-indexed)
    #[serde(skip)]
    pub source_lines: Option<Range<usize>>,
    /// The original source lines for this task (for verbatim emission)
    #[serde(skip)]
    pub source_text: Option<Vec<String>>,
    /// Whether this task has been modified since parsing
    #[serde(skip)]
    pub dirty: bool,
}

impl Task {
    /// Create a new task with the given fields, marked dirty (no source)
    pub fn new(state: TaskState, id: Option<String>, title: String) -> Self {
        Task {
            state,
            id,
            title,
            tags: Vec::new(),
            metadata: Vec::new(),
            subtasks: Vec::new(),
            depth: 0,
            source_lines: None,
            source_text: None,
            dirty: true,
        }
    }

    /// Mark this task as dirty (will be serialized in canonical format)
    pub fn mark_dirty(&mut self) {
        self.dirty = true;
    }
}

impl PartialEq for Task {
    fn eq(&self, other: &Self) -> bool {
        self.state == other.state
            && self.id == other.id
            && self.title == other.title
            && self.tags == other.tags
            && self.metadata == other.metadata
            && self.subtasks == other.subtasks
            && self.depth == other.depth
    }
}

impl Eq for Task {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checkbox_char_all_states() {
        assert_eq!(TaskState::Todo.checkbox_char(), ' ');
        assert_eq!(TaskState::Active.checkbox_char(), '>');
        assert_eq!(TaskState::Blocked.checkbox_char(), '-');
        assert_eq!(TaskState::Done.checkbox_char(), 'x');
        assert_eq!(TaskState::Parked.checkbox_char(), '~');
    }

    #[test]
    fn from_checkbox_char_valid() {
        assert_eq!(TaskState::from_checkbox_char(' '), Some(TaskState::Todo));
        assert_eq!(TaskState::from_checkbox_char('>'), Some(TaskState::Active));
        assert_eq!(TaskState::from_checkbox_char('-'), Some(TaskState::Blocked));
        assert_eq!(TaskState::from_checkbox_char('x'), Some(TaskState::Done));
        assert_eq!(TaskState::from_checkbox_char('~'), Some(TaskState::Parked));
    }

    #[test]
    fn from_checkbox_char_invalid() {
        assert_eq!(TaskState::from_checkbox_char('?'), None);
        assert_eq!(TaskState::from_checkbox_char('X'), None);
        assert_eq!(TaskState::from_checkbox_char('a'), None);
    }

    #[test]
    fn metadata_key_all_variants() {
        assert_eq!(Metadata::Dep(vec![]).key(), "dep");
        assert_eq!(Metadata::Ref(vec![]).key(), "ref");
        assert_eq!(Metadata::Spec(String::new()).key(), "spec");
        assert_eq!(Metadata::Note(String::new()).key(), "note");
        assert_eq!(Metadata::Added(String::new()).key(), "added");
        assert_eq!(Metadata::Resolved(String::new()).key(), "resolved");
    }

    #[test]
    fn task_new_fields() {
        let task = Task::new(TaskState::Active, Some("T-001".into()), "My task".into());
        assert_eq!(task.state, TaskState::Active);
        assert_eq!(task.id.as_deref(), Some("T-001"));
        assert_eq!(task.title, "My task");
        assert!(task.tags.is_empty());
        assert!(task.metadata.is_empty());
        assert!(task.subtasks.is_empty());
        assert_eq!(task.depth, 0);
        assert!(task.source_lines.is_none());
        assert!(task.source_text.is_none());
        assert!(task.dirty);
    }

    #[test]
    fn task_new_no_id() {
        let task = Task::new(TaskState::Todo, None, "No ID".into());
        assert!(task.id.is_none());
    }

    #[test]
    fn mark_dirty_sets_flag() {
        let mut task = Task::new(TaskState::Todo, None, "test".into());
        task.dirty = false;
        task.mark_dirty();
        assert!(task.dirty);
    }

    #[test]
    fn partial_eq_equal_tasks() {
        let a = Task::new(TaskState::Todo, Some("T-001".into()), "Same".into());
        let b = Task::new(TaskState::Todo, Some("T-001".into()), "Same".into());
        assert_eq!(a, b);
    }

    #[test]
    fn partial_eq_ignores_source_and_dirty() {
        let mut a = Task::new(TaskState::Todo, Some("T-001".into()), "Same".into());
        let mut b = Task::new(TaskState::Todo, Some("T-001".into()), "Same".into());
        a.source_text = Some(vec!["- [ ] `T-001` Same".into()]);
        a.source_lines = Some(0..1);
        a.dirty = false;
        b.dirty = true;
        assert_eq!(a, b);
    }

    #[test]
    fn partial_eq_differs_by_state() {
        let a = Task::new(TaskState::Todo, Some("T-001".into()), "Same".into());
        let b = Task::new(TaskState::Done, Some("T-001".into()), "Same".into());
        assert_ne!(a, b);
    }

    #[test]
    fn partial_eq_differs_by_title() {
        let a = Task::new(TaskState::Todo, Some("T-001".into()), "Alpha".into());
        let b = Task::new(TaskState::Todo, Some("T-001".into()), "Beta".into());
        assert_ne!(a, b);
    }

    #[test]
    fn partial_eq_differs_by_id() {
        let a = Task::new(TaskState::Todo, Some("T-001".into()), "Same".into());
        let b = Task::new(TaskState::Todo, Some("T-002".into()), "Same".into());
        assert_ne!(a, b);
    }
}
