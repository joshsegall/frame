use serde::Serialize;

use crate::model::task::{Metadata, Task, TaskState};
use crate::model::track::Track;
use crate::ops::deps::{DepNode, DepStatus};
use crate::ops::track_ops::TrackStats;

// ---------------------------------------------------------------------------
// JSON output structs
// ---------------------------------------------------------------------------

#[derive(Serialize)]
pub struct TaskJson {
    pub id: Option<String>,
    pub title: String,
    pub state: TaskState,
    pub tags: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub deps: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spec: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub refs: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub added: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolved: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub subtasks: Vec<TaskJson>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub ancestors: Vec<TaskJson>,
}

#[derive(Serialize)]
pub struct TaskListJson {
    pub track: String,
    pub tasks: Vec<TaskJson>,
}

#[derive(Serialize)]
pub struct ReadyJson {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub focus_track: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cc_only: Option<bool>,
    pub tasks: Vec<TaskWithTrackJson>,
}

#[derive(Serialize)]
pub struct TaskWithTrackJson {
    pub track: String,
    #[serde(flatten)]
    pub task: TaskJson,
}

#[derive(Serialize)]
pub struct TrackInfoJson {
    pub id: String,
    pub name: String,
    pub state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cc_focus: Option<bool>,
    pub stats: TrackStatsJson,
}

#[derive(Serialize)]
pub struct TrackStatsJson {
    pub active: usize,
    pub blocked: usize,
    pub todo: usize,
    pub parked: usize,
    pub done: usize,
}

#[derive(Serialize)]
pub struct InboxItemJson {
    pub index: usize,
    pub title: String,
    pub tags: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
}

#[derive(Serialize)]
pub struct StatsJson {
    pub tracks: Vec<TrackStatsEntryJson>,
    pub totals: TrackStatsJson,
}

#[derive(Serialize)]
pub struct TrackStatsEntryJson {
    pub id: String,
    pub name: String,
    pub stats: TrackStatsJson,
}

/// A node in a `fr deps --json` tree.
///
/// Nested rather than a flat `{root, nodes, edges}` graph: the nesting mirrors
/// the human tree exactly, which is what lets `tests/parity.rs` compare the two
/// surfaces directly. A consumer that wants edges can flatten this; a parity
/// test cannot un-flatten a graph.
///
/// Everything but `id` and `status` is absent on a non-`resolved` node. A
/// `cycle` or `repeat` node points at a record that appears elsewhere in the
/// same document, and repeating the record would invite a consumer to count one
/// task twice; a `missing` node has no record at all.
#[derive(Serialize)]
pub struct DepNodeJson {
    pub id: String,
    pub status: DepStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub track: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<TaskState>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub deps: Vec<DepNodeJson>,
}

#[derive(Serialize)]
pub struct SearchHitJson {
    pub track: String,
    pub task_id: String,
    pub title: String,
    pub field: String,
}

// ---------------------------------------------------------------------------
// Conversions
// ---------------------------------------------------------------------------

pub fn task_to_json(task: &Task) -> TaskJson {
    let mut deps = Vec::new();
    let mut refs = Vec::new();
    let mut spec = None;
    let mut note = None;
    let mut added = None;
    let mut resolved = None;

    for m in &task.metadata {
        match m {
            Metadata::Dep(d) => deps.extend(d.iter().cloned()),
            Metadata::Ref(r) => refs.extend(r.iter().cloned()),
            Metadata::Spec(s) => spec = Some(s.clone()),
            Metadata::Note(n) => note = Some(n.clone()),
            Metadata::Added(a) => added = Some(a.clone()),
            Metadata::Resolved(r) => resolved = Some(r.clone()),
        }
    }

    TaskJson {
        id: task.id.as_ref().map(|i| i.to_string()),
        title: task.title.clone(),
        state: task.state,
        tags: task.tags.clone(),
        deps,
        spec,
        refs,
        note,
        added,
        resolved,
        subtasks: task.subtasks.iter().map(task_to_json).collect(),
        ancestors: Vec::new(),
    }
}

pub fn dep_tree_to_json(node: &DepNode) -> DepNodeJson {
    DepNodeJson {
        id: node.id.clone(),
        status: node.status,
        track: node.track_id.clone(),
        title: node.title.clone(),
        state: node.state,
        tags: node.tags.clone(),
        deps: node.deps.iter().map(dep_tree_to_json).collect(),
    }
}

pub fn stats_to_json(stats: &TrackStats) -> TrackStatsJson {
    TrackStatsJson {
        active: stats.active,
        blocked: stats.blocked,
        todo: stats.todo,
        parked: stats.parked,
        done: stats.done,
    }
}

// ---------------------------------------------------------------------------
// Human-readable formatting
// ---------------------------------------------------------------------------

fn state_char(state: TaskState) -> char {
    state.checkbox_char()
}

/// Format a single task as a one-line summary
pub fn format_task_line(task: &Task) -> String {
    let sc = state_char(task.state);
    let id_str = task
        .id
        .as_ref()
        .map(|id| format!("{} ", id))
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
    format!("[{}] {}{}{}", sc, id_str, task.title, tags_str)
}

/// Format a task with its subtasks, indented
pub fn format_task_tree(task: &Task, indent: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let prefix = "  ".repeat(indent);
    lines.push(format!("{}{}", prefix, format_task_line(task)));

    for sub in &task.subtasks {
        lines.extend(format_task_tree(sub, indent + 1));
    }
    lines
}

/// Format detailed task view
pub fn format_task_detail(task: &Task) -> Vec<String> {
    let mut lines = Vec::new();

    // Header
    let sc = state_char(task.state);
    let id_str = task
        .id
        .as_ref()
        .map(|id| format!("{} ", id))
        .unwrap_or_default();
    lines.push(format!("[{}] {}{}", sc, id_str, task.title));

    // Tags
    if !task.tags.is_empty() {
        lines.push(format!(
            "tags: {}",
            task.tags
                .iter()
                .map(|t| format!("#{}", t))
                .collect::<Vec<_>>()
                .join(" ")
        ));
    }

    // Metadata
    for m in &task.metadata {
        match m {
            Metadata::Added(d) => lines.push(format!("added: {}", d)),
            Metadata::Resolved(d) => lines.push(format!("resolved: {}", d)),
            Metadata::Dep(deps) => lines.push(format!("dep: {}", deps.join(", "))),
            Metadata::Spec(s) => lines.push(format!("spec: {}", s)),
            Metadata::Ref(refs) => {
                for r in refs {
                    lines.push(format!("ref: {}", r));
                }
            }
            Metadata::Note(n) => {
                lines.push("note:".to_string());
                for line in n.lines() {
                    lines.push(format!("  {}", line));
                }
            }
        }
    }

    // Subtasks
    if !task.subtasks.is_empty() {
        lines.push(String::new());
        lines.push("subtasks:".to_string());
        for sub in &task.subtasks {
            for line in format_task_tree(sub, 1) {
                lines.push(line);
            }
        }
    }

    lines
}

/// Format a separator line for context display
fn format_context_separator(label: &str, task: &Task) -> String {
    let id_str = task
        .id
        .as_ref()
        .map(|id| format!("{} ", id))
        .unwrap_or_default();
    format!("── {} ── {}{}", label, id_str, task.title)
}

/// Format task detail with ancestor context (--context flag)
pub fn format_task_detail_with_context(ancestors: &[&Task], task: &Task) -> Vec<String> {
    let mut lines = Vec::new();

    for ancestor in ancestors {
        lines.push(format_context_separator("Parent", ancestor));
        lines.extend(format_context_fields(ancestor));
        lines.push(String::new());
    }

    lines.push(format_context_separator("Task", task));
    lines.extend(format_context_fields(task));

    // Subtasks
    if !task.subtasks.is_empty() {
        lines.push(String::new());
        lines.push("subtasks:".to_string());
        for sub in &task.subtasks {
            for line in format_task_tree(sub, 1) {
                lines.push(line);
            }
        }
    }

    lines
}

/// Format the fields of a task for context display (indented, no header)
fn format_context_fields(task: &Task) -> Vec<String> {
    let mut lines = Vec::new();

    let state_str = match task.state {
        TaskState::Todo => "todo",
        TaskState::Active => "active",
        TaskState::Blocked => "blocked",
        TaskState::Done => "done",
        TaskState::Parked => "parked",
    };
    lines.push(format!("  state: {}", state_str));

    if !task.tags.is_empty() {
        lines.push(format!(
            "  tags: {}",
            task.tags
                .iter()
                .map(|t| format!("#{}", t))
                .collect::<Vec<_>>()
                .join(" ")
        ));
    }

    for m in &task.metadata {
        match m {
            Metadata::Added(d) => lines.push(format!("  added: {}", d)),
            Metadata::Resolved(d) => lines.push(format!("  resolved: {}", d)),
            Metadata::Dep(deps) => lines.push(format!("  dep: {}", deps.join(", "))),
            Metadata::Spec(s) => lines.push(format!("  spec: {}", s)),
            Metadata::Ref(refs) => {
                for r in refs {
                    lines.push(format!("  ref: {}", r));
                }
            }
            Metadata::Note(n) => {
                lines.push("  note:".to_string());
                for line in n.lines() {
                    lines.push(format!("    {}", line));
                }
            }
        }
    }

    lines
}

/// Format a track listing header
pub fn format_track_header(track_id: &str, track: &Track) -> String {
    format!("== {} ({}) ==", track.title, track_id)
}

/// The tasks a `fr list` invocation shows from one track, grouped the way the
/// human surface presents them.
///
/// One selection with two consumers: `--json` flattens it with [`Self::all`],
/// the human surface renders it with section headers. Having two
/// implementations of the selection is what `b664a3e` fixed — the human path
/// read only Backlog and Parked while the JSON path already included Done — and
/// the fix left both implementations in place. `tests/parity.rs` asserts the two
/// surfaces still agree; this type is why they cannot stop agreeing.
pub struct FilteredTasks<'a> {
    pub backlog: Vec<&'a Task>,
    pub parked: Vec<&'a Task>,
    pub done: Vec<&'a Task>,
}

impl<'a> FilteredTasks<'a> {
    /// Every selected task, in the order the human surface prints its sections.
    pub fn all(&self) -> impl Iterator<Item = &'a Task> + '_ {
        self.backlog
            .iter()
            .chain(&self.parked)
            .chain(&self.done)
            .copied()
    }
}

/// Select the tasks a `fr list` invocation shows from one track.
pub fn select_tasks<'a>(
    track: &'a Track,
    state_filter: Option<TaskState>,
    tag_filter: Option<&str>,
) -> FilteredTasks<'a> {
    let matches = |task: &&Task| -> bool {
        if let Some(sf) = state_filter
            && task.state != sf
        {
            return false;
        }
        if let Some(tf) = tag_filter
            && !task.tags.iter().any(|t| t == tf)
        {
            return false;
        }
        true
    };

    FilteredTasks {
        backlog: track.backlog().iter().filter(matches).collect(),
        parked: track.parked().iter().filter(matches).collect(),
        // Done tasks are only surfaced when explicitly filtered for; otherwise
        // the completed pile would drown out the live backlog.
        done: if state_filter == Some(TaskState::Done) {
            track.done().iter().filter(matches).collect()
        } else {
            Vec::new()
        },
    }
}

/// Format a track's task listing
pub fn format_track_listing(track_id: &str, track: &Track, tasks: &FilteredTasks) -> Vec<String> {
    let mut lines = Vec::new();
    lines.push(format_track_header(track_id, track));
    lines.push(String::new());

    let mut any_shown = false;
    for task in &tasks.backlog {
        for line in format_task_tree(task, 0) {
            lines.push(line);
        }
    }
    any_shown |= !tasks.backlog.is_empty();

    if !tasks.parked.is_empty() {
        if any_shown {
            lines.push(String::new());
        }
        lines.push("-- Parked --".to_string());
        for task in &tasks.parked {
            for line in format_task_tree(task, 0) {
                lines.push(line);
            }
        }
        any_shown = true;
    }

    if !tasks.done.is_empty() {
        if any_shown {
            lines.push(String::new());
        }
        lines.push("-- Done --".to_string());
        for task in &tasks.done {
            for line in format_task_tree(task, 0) {
                lines.push(line);
            }
        }
    }

    lines
}

/// Render a dependency tree.
///
/// The root line carries tags and the descendants do not; that asymmetry is
/// how `fr deps` has always printed and is preserved deliberately.
pub fn format_dep_tree(root: &DepNode) -> Vec<String> {
    let mut lines = Vec::new();

    let state = root.state.unwrap_or(TaskState::Todo);
    let tags = if root.tags.is_empty() {
        String::new()
    } else {
        format!(
            " {}",
            root.tags
                .iter()
                .map(|t| format!("#{}", t))
                .collect::<Vec<_>>()
                .join(" ")
        )
    };
    lines.push(format!(
        "[{}] {} {}{}",
        state_char(state),
        root.id,
        root.title.clone().unwrap_or_default(),
        tags
    ));

    if root.deps.is_empty() {
        lines.push("  (no dependencies)".to_string());
    } else {
        for dep in &root.deps {
            format_dep_node(dep, 1, &mut lines);
        }
    }
    lines
}

fn format_dep_node(node: &DepNode, indent: usize, lines: &mut Vec<String>) {
    let prefix = "  ".repeat(indent);
    match node.status {
        DepStatus::Resolved => {
            lines.push(format!(
                "{}└─ [{}] {} {}",
                prefix,
                state_char(node.state.unwrap_or(TaskState::Todo)),
                node.id,
                node.title.clone().unwrap_or_default()
            ));
            for child in &node.deps {
                format_dep_node(child, indent + 1, lines);
            }
        }
        // Three distinct conditions the output used to collapse into two: a
        // task reached twice is not a cycle, and saying so was the whole point
        // of splitting the status.
        DepStatus::Cycle => lines.push(format!("{}└─ {} (circular)", prefix, node.id)),
        DepStatus::Repeat => lines.push(format!("{}└─ {} (already shown)", prefix, node.id)),
        DepStatus::Missing => lines.push(format!("{}└─ {} (not found)", prefix, node.id)),
    }
}

/// Parse a state string into TaskState
pub fn parse_task_state(s: &str) -> Result<TaskState, String> {
    match s {
        "todo" => Ok(TaskState::Todo),
        "active" => Ok(TaskState::Active),
        "blocked" => Ok(TaskState::Blocked),
        "done" => Ok(TaskState::Done),
        "parked" => Ok(TaskState::Parked),
        _ => Err(format!(
            "unknown state '{}' (expected: todo, active, blocked, done, parked)",
            s
        )),
    }
}
