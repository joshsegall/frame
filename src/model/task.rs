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
