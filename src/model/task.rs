use serde::{Deserialize, Serialize};

use crate::model::task_id::TaskId;

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
    /// `ref: path/to/file, other/file`
    ///
    /// Comma-separated, and **only** comma-separated: a ref may contain spaces.
    /// See [`crate::tui::fields`] for what that costs the editor.
    Ref(Vec<String>),
    /// `spec: path/to/spec#section, other/spec.md`
    ///
    /// Same shape and same rule as [`Metadata::Ref`]; the two differ only in
    /// what they mean, not in how they are written or parsed.
    Spec(Vec<String>),
    /// `note:` followed by block text
    Note(String),
    /// `added: 2025-05-14`
    Added(String),
    /// `resolved: 2025-05-14`
    Resolved(String),
    /// `conflict: both-edited 2026-08-03T04:08:38Z`
    ///
    /// Left by `fr merge` on a task it could not decide. Ours was kept and their
    /// version went to the recovery log at that timestamp.
    ///
    /// It exists because the merge writes **no conflict markers** — the file
    /// stays valid frame markdown, which is what keeps every other tool usable.
    /// Without a mark in the file, staging the path would quietly commit our side
    /// and drop theirs, with nothing but scrolled-away stderr to say so. `fr
    /// check` reports it as an error; `fr merge --resolve <ID>` clears it.
    Conflict(String),
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
            Metadata::Conflict(_) => "conflict",
        }
    }

    /// Where this field sits in the canonical order, low first.
    ///
    /// One rule: short scalar fields first, the one unbounded field last. A note
    /// has no length bound, so anything written after it is written past the
    /// fold — on a real task, a `resolved:` date 55 lines down, which reads as
    /// missing. Nothing enforced an order before this, and writes append:
    /// `set_state` pushes `resolved:` and `set_metadata` pushes any key the task
    /// did not already carry, so `added → note → resolved` and
    /// `added → note → ref` are what a working project accumulates. 54% of the
    /// tasks in the project this was found on were in some such order.
    ///
    /// `conflict:` leads because it is the most urgent thing a task can say: the
    /// merge that left it wrote no conflict markers, so this line is the only
    /// mark in the file that ours was kept and theirs went to the recovery log.
    ///
    /// **This is the one definition.** The serializer writes a dirty task's
    /// lines in it, `fr show` orders both its human forms by it, `TaskJson`
    /// declares its fields in it, and the TUI Detail view builds its regions
    /// from it. Four surfaces answering one question separately is the drift
    /// this codebase keeps paying for — `FilteredTasks` in `cli::output` is the
    /// same move for a different question.
    pub fn rank(&self) -> u8 {
        match self {
            Metadata::Conflict(_) => 0,
            Metadata::Added(_) => 1,
            Metadata::Resolved(_) => 2,
            Metadata::Dep(_) => 3,
            Metadata::Spec(_) => 4,
            Metadata::Ref(_) => 5,
            Metadata::Note(_) => 6,
        }
    }
}

/// A task's metadata in canonical order, borrowed — the task is not touched.
///
/// **Partitions, never filters.** The rank match is exhaustive and every entry
/// is emitted exactly once, so this cannot drop a field: a new [`Metadata`]
/// variant fails the build until it is ranked, rather than silently vanishing
/// from every surface at once.
///
/// **Stable, so duplicate keys keep their relative order.** That is reachable,
/// not theoretical — an *unknown* metadata key parses to a `Note` carrying its
/// own `key: value` text, so one task can hold several notes, and reordering
/// them against each other would scramble text a user wrote.
///
/// Display surfaces call this unconditionally. The **serializer** does not: a
/// task whose stranded lines would be absorbed by a note moved last keeps its
/// own order instead. That check needs the task's indent, which only the
/// serializer knows — see the comment in `parse::task_serializer`.
pub fn ordered_metadata(task: &Task) -> Vec<&Metadata> {
    let mut ordered: Vec<&Metadata> = task.metadata.iter().collect();
    ordered.sort_by_key(|m| m.rank());
    ordered
}

/// A task with all its parsed fields and source tracking
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    /// Checkbox state
    pub state: TaskState,
    /// Optional task ID like `EFF-014` or `EFF-014.2`
    pub id: Option<TaskId>,
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
    /// Lines that sit immediately *before* this task's line and belong to no
    /// task — mis-indented prose, metadata stranded after stray content, the
    /// residue of a bad hand edit or merge. They are carried verbatim and
    /// re-emitted ahead of the task line so a rewrite puts them back where they
    /// were.
    ///
    /// The parser used to drop them: any non-blank line more indented than the
    /// current level, with another task still to come at that level, was
    /// consumed by `parse_tasks` and recorded nowhere. Nothing surfaced it —
    /// the line was absent from the model, so `fr check` could not see it, and
    /// the next write of that file deleted it. `fr clean` made that routine,
    /// because filling one task's `resolved:` date rewrites the whole track.
    ///
    /// Attaching to the *following* task rather than the preceding one is what
    /// lets a single field cover every case **of this shape**: a line stranded
    /// between tasks is only ever droppable when a task follows it at the same
    /// level, so there is always a successor to hang it on. Content stranded
    /// *inside* a task is a different shape and wants the opposite anchor — see
    /// [`Task::trailing_lines`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub leading_lines: Vec<String>,
    /// Lines that sit *after* this task's metadata, indented past it, that are
    /// neither metadata nor a subtask nor part of a `- note:` block — a note
    /// that lost its `- note:` key, the residue of a bad hand edit. Carried
    /// verbatim and re-emitted in place, before any subtasks.
    ///
    /// The mirror of [`Task::leading_lines`], and it exists because the two
    /// shapes want opposite anchors. This content used to go to the *following*
    /// task as well, which put it on a task at a different nesting level: the
    /// line lived inside one task's subtree and was carried by that task's
    /// parent's *sibling*. Nothing recorded where it really sat, so a section
    /// move relocated the task and left the line behind — where, re-emitted at
    /// its original indent into a neighbourhood that had changed, it was read
    /// as part of a *different* task's note and destroyed by the next ordinary
    /// edit of that task.
    ///
    /// Anchoring it to the task it sits under is what makes it travel: a
    /// section move, an archive, a cross-track move all carry the task, and the
    /// line goes with it at the same relative position, so it parses back the
    /// same way. The successor argument above does not apply here — content
    /// stranded inside a task always has a predecessor, namely the task whose
    /// metadata it followed.
    ///
    /// An over-deep *task* line is not this: it still goes back to
    /// `parse_tasks`, which flattens it into a real task at the enclosing level.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub trailing_lines: Vec<String>,

    // --- Source tracking ---
    /// The original source lines for this task (for verbatim emission)
    #[serde(skip)]
    pub source_text: Option<Vec<String>>,
    /// Whether this task has been modified since parsing
    #[serde(skip)]
    pub dirty: bool,
}

impl Task {
    /// Create a new task with the given fields, marked dirty (no source)
    pub fn new(state: TaskState, id: Option<TaskId>, title: String) -> Self {
        Task {
            state,
            id,
            title,
            tags: Vec::new(),
            metadata: Vec::new(),
            subtasks: Vec::new(),
            depth: 0,
            leading_lines: Vec::new(),
            trailing_lines: Vec::new(),
            source_text: None,
            dirty: true,
        }
    }

    /// Mark this task as dirty (will be serialized in canonical format)
    pub fn mark_dirty(&mut self) {
        self.dirty = true;
    }
}

/// Compares the *semantic* fields only. `source_text`, `dirty`, `leading_lines`
/// and `trailing_lines` are carried source, not task identity:
/// two tasks that say the same thing are equal even if one of them is dragging a
/// stranded line behind it. Conservation of the stranded lines is checked at the
/// text level by the parse properties, not here.
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
        assert_eq!(Metadata::Spec(Vec::new()).key(), "spec");
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
        assert!(task.source_text.is_none());
        assert!(task.dirty);
    }

    #[test]
    fn task_new_no_id() {
        let task = Task::new(TaskState::Todo, None, "No ID".into());
        assert!(task.id.is_none());
    }

    /// Ordering partitions rather than filters: every entry comes back, exactly
    /// once, whatever order it went in as.
    #[test]
    fn ordering_keeps_every_entry() {
        let mut task = Task::new(TaskState::Done, None, "t".into());
        task.metadata = vec![
            Metadata::Note("n".into()),
            Metadata::Resolved("2025-05-14".into()),
            Metadata::Ref(vec!["a.rs".into()]),
            Metadata::Added("2025-05-01".into()),
            Metadata::Conflict("both-edited".into()),
            Metadata::Spec(vec!["s.md".into()]),
            Metadata::Dep(vec!["T-1".into()]),
        ];

        let keys: Vec<&str> = ordered_metadata(&task).iter().map(|m| m.key()).collect();
        assert_eq!(
            keys,
            [
                "conflict", "added", "resolved", "dep", "spec", "ref", "note"
            ]
        );
        assert_eq!(ordered_metadata(&task).len(), task.metadata.len());
    }

    /// Two entries sharing a key keep their relative order — an unknown metadata
    /// key parses to a `Note`, so a task can hold several, and they are text
    /// somebody wrote in an order they chose.
    #[test]
    fn ordering_is_stable_within_a_key() {
        let mut task = Task::new(TaskState::Todo, None, "t".into());
        task.metadata = vec![
            Metadata::Note("first".into()),
            Metadata::Added("2025-05-01".into()),
            Metadata::Note("second".into()),
        ];

        let notes: Vec<&str> = ordered_metadata(&task)
            .iter()
            .filter_map(|m| match m {
                Metadata::Note(n) => Some(n.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(notes, ["first", "second"]);
    }

    /// A dirty task is written in canonical order — this is the shape the whole
    /// change is for, `added → note → resolved` becoming `added → resolved →
    /// note`.
    #[test]
    fn serializing_a_dirty_task_reorders() {
        let mut task = Task::new(TaskState::Done, None, "t".into());
        task.metadata = vec![
            Metadata::Added("2025-05-01".into()),
            Metadata::Note("body".into()),
            Metadata::Resolved("2025-05-14".into()),
        ];
        task.mark_dirty();

        let text = crate::parse::serialize_tasks(std::slice::from_ref(&task), 0).join("\n");
        let added = text.find("added:").unwrap();
        let resolved = text.find("resolved:").unwrap();
        let note = text.find("note:").unwrap();
        assert!(added < resolved && resolved < note, "not ordered:\n{text}");
    }

    /// A **clean** task is written verbatim from `source_text`, so a file frame
    /// has not touched keeps whatever order it already had. This is what makes
    /// the change converge per-task instead of rewriting every track at once.
    #[test]
    fn serializing_a_clean_task_changes_nothing() {
        let source = vec![
            "- [x] `T-1` t".to_string(),
            "  - added: 2025-05-01".to_string(),
            "  - note: body".to_string(),
            "  - resolved: 2025-05-14".to_string(),
        ];
        let mut task = Task::new(TaskState::Done, None, "t".into());
        task.metadata = vec![
            Metadata::Added("2025-05-01".into()),
            Metadata::Note("body".into()),
            Metadata::Resolved("2025-05-14".into()),
        ];
        task.source_text = Some(source.clone());
        task.dirty = false;

        assert_eq!(
            crate::parse::serialize_tasks(std::slice::from_ref(&task), 0),
            source
        );
    }

    /// A task whose stranded lines sit at the note's block indent keeps its own
    /// order: moving the note last would put it directly above them and they
    /// would read back as its body. The `added:` line between the two is what
    /// closes the note, and reordering would take it away.
    #[test]
    fn a_task_whose_stranded_lines_would_be_absorbed_keeps_its_order() {
        let mut task = Task::new(TaskState::Todo, None, "t".into());
        task.metadata = vec![
            Metadata::Note(String::new()),
            Metadata::Added("2025-05-01".into()),
        ];
        task.trailing_lines = vec!["    ```rust".to_string(), "    stranded".to_string()];
        task.mark_dirty();

        let text = crate::parse::serialize_tasks(std::slice::from_ref(&task), 0).join("\n");
        let note = text.find("note:").unwrap();
        let added = text.find("added:").unwrap();
        assert!(note < added, "order should have been kept:\n{text}");

        // And the round trip proves why: the stranded lines come back as
        // stranded lines, not as note body.
        let parsed = crate::parse::parse_track(&format!("# T\n\n## Backlog\n\n{text}\n"));
        let reserialized = crate::parse::serialize_track(&parsed);
        assert!(reserialized.contains("stranded"), "{reserialized}");
    }

    /// Stranded lines that are *dedented* past the note cannot be absorbed, so
    /// the guard must not fire for them — otherwise it would refuse to order
    /// most damaged tasks for no reason.
    #[test]
    fn shallow_stranded_lines_do_not_block_ordering() {
        let mut task = Task::new(TaskState::Done, None, "t".into());
        task.metadata = vec![
            Metadata::Note("body".into()),
            Metadata::Resolved("2025-05-14".into()),
        ];
        task.trailing_lines = vec!["  not deep enough".to_string()];
        task.mark_dirty();

        let text = crate::parse::serialize_tasks(std::slice::from_ref(&task), 0).join("\n");
        let resolved = text.find("resolved:").unwrap();
        let note = text.find("note:").unwrap();
        assert!(resolved < note, "should have been ordered:\n{text}");
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
