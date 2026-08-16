use serde::Serialize;

use crate::model::task::{Metadata, Task, TaskState, ordered_metadata};
use crate::model::track::Track;
use crate::ops::deps::{DepNode, DepStatus};
use crate::ops::track_ops::TrackStats;

// ---------------------------------------------------------------------------
// JSON output structs
// ---------------------------------------------------------------------------

/// **Field declaration order is output order.** These structs are serialized
/// straight through `serde_json::to_string_pretty` with no `Value` round-trip,
/// so serde emits keys in the order they are declared here. The order below is
/// [`Metadata::rank`]'s, so `--json` reads in the same sequence as `fr show` and
/// the TUI Detail view. (The markdown is not ordered — see [`ordered_metadata`].)
///
/// Nothing but a test can hold that: this order is fixed at compile time by the
/// declarations while the other two are computed from `rank` at run time.
/// `tests/parity.rs::human_and_json_agree_on_field_order` is what fails when
/// these drift apart.
#[derive(Serialize)]
pub struct TaskJson {
    pub id: Option<String>,
    pub title: String,
    pub state: TaskState,
    pub tags: Vec<String>,
    /// Where an archived task was read from. Present only when the task came out
    /// of an archive rather than a live track — absent, not null, so a consumer
    /// gates on it the way it gates on `conflict`, and so every command that
    /// emits a live task emits exactly the bytes it did before.
    ///
    /// Set on the shown task alone. Its subtasks and ancestors came out of the
    /// same file by construction, and repeating it down the tree would say
    /// nothing new.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub archived: Option<ArchivedIn>,
    /// An unresolved merge conflict left by `fr merge`. Present only while the
    /// task still carries one, so a consumer can gate on it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub conflict: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub added: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolved: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub deps: Vec<String>,
    /// Spec paths. An array since 0.1.8 — a task may carry several, the same way
    /// `refs` always could. It was a bare string before that.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub spec: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub refs: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub subtasks: Vec<TaskJson>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub ancestors: Vec<TaskJson>,
}

/// Where an archived task was read from, as `fr show` reports it.
///
/// The two strings are the whole of it, and both surfaces render *these* rather
/// than re-deriving anything: the human `archived:` line is [`Self::value`] and
/// `--json` serializes the fields. A path alone would have made the human line
/// long and the track id a thing to parse out of it; a track id alone would not
/// say which of the two archive shapes holds the task, since a done-task archive
/// and a whole archived track can share a track id.
#[derive(Serialize, Clone)]
pub struct ArchivedIn {
    pub track: String,
    /// Path from the project root, the way a person would type it to open the
    /// file: `frame/archive/bac.md`, or `frame/archive/_tracks/bac.md`.
    pub file: String,
}

impl ArchivedIn {
    /// Build from a track id and a path relative to `frame/`, as
    /// [`crate::io::project_io::ArchivedTasks`] carries it.
    pub fn new(track_id: &str, frame_relative_file: &str) -> Self {
        ArchivedIn {
            track: track_id.to_string(),
            file: format!("frame/{}", frame_relative_file),
        }
    }

    /// The human surface's `archived:` value: the track, then the file it is in.
    pub fn value(&self) -> String {
        format!("{} ({})", self.track, self.file)
    }
}

/// `fr clean --json`.
///
/// The clean report is flattened in rather than nested under a key, so the
/// document reads like `fr check --json` does — one object whose fields are the
/// finding categories. `field_order` is separate because it is the one category
/// clean *reports* without acting on.
///
/// Two flags carry what the arrays cannot say on their own. `dry_run` is whether
/// anything was written at all. `normalize` is what `field_order.reordered`
/// means: with it, those tasks were rewritten; without it, they were merely
/// found, and running `fr clean --normalize` is what would change them. A
/// consumer that ignores the flag would read a preview as a result.
#[derive(Serialize)]
pub struct CleanJson<'a> {
    pub dry_run: bool,
    pub normalize: bool,
    #[serde(flatten)]
    pub result: &'a crate::ops::clean::CleanResult,
    pub field_order: &'a crate::ops::clean::NormalizeResult,
}

/// What a task-writing command did, for `--json`.
///
/// `tasks` carries the affected tasks in full, as [`TaskJson`] — the same shape
/// `fr show --json` returns, so a consumer that creates a task with `fr add` gets
/// back exactly what it would get by looking the task up afterwards, rather than
/// a second task shape to learn. A list because `delete` and `import` act on
/// several; for `delete` it is the snapshot taken *before* the deletion, since
/// afterwards there is nothing left to describe.
///
/// `changed` is whether the project actually differs. Distinct from success:
/// `fr tag T-1 add cc` on a task already tagged `cc` succeeds and changes
/// nothing, and a consumer deciding whether to commit needs to tell those apart.
/// Computed by comparing the task before and after — see [`Task`]'s hand-written
/// `PartialEq`, which ignores `source_text` and `dirty` and so compares exactly
/// what a reader would call a change.
#[derive(Serialize)]
pub struct TaskWriteJson {
    /// The subcommand, so a consumer piping several can tell them apart.
    pub command: &'static str,
    pub changed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub track: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tasks: Vec<TaskJson>,
}

/// What a track-writing command did. [`TaskWriteJson`]'s rules, one level up.
#[derive(Serialize)]
pub struct TrackWriteJson {
    pub command: &'static str,
    pub changed: bool,
    pub track: TrackInfoJson,
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

/// `fr search --json`.
///
/// Three named arrays rather than one flat array of tagged hits: each is
/// homogeneous, so a consumer gets a stable schema per array instead of a union
/// it has to discriminate. The grouping is not invented — it is the order the
/// human surface already prints (live, then archive, then inbox), so
/// concatenating the three reproduces the human sequence exactly.
///
/// `archived` is always present, empty under `--no-archive`, so the schema does
/// not change shape with the flag.
#[derive(Serialize)]
pub struct SearchJson {
    pub pattern: String,
    pub tasks: Vec<SearchHitJson>,
    pub archived: Vec<SearchHitJson>,
    pub inbox: Vec<InboxSearchHitJson>,
}

/// One task the search matched, with every field that matched it.
///
/// `matched_fields` is the one thing the JSON carries that the human line
/// cannot — the human surface names a field only when it *fails* to resolve the
/// task. It lists all matching fields, not the first: `cmd_search` collapses
/// per-field hits into one entry per task, and reporting only the first would
/// mean reporting whichever field the scan happened to reach first.
#[derive(Serialize)]
pub struct SearchHitJson {
    pub track: String,
    /// Absent when the hit does not resolve to a task. Reachable for a task
    /// with no id: hits carry `""` for those, and nothing can be looked up by
    /// it. `matched_fields` still says what matched.
    #[serde(flatten)]
    pub task: Option<TaskJson>,
    pub matched_fields: Vec<String>,
}

#[derive(Serialize)]
pub struct InboxSearchHitJson {
    pub index: usize,
    pub title: String,
    pub tags: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    pub matched_fields: Vec<String>,
}

// ---------------------------------------------------------------------------
// Conversions
// ---------------------------------------------------------------------------

pub fn task_to_json(task: &Task) -> TaskJson {
    let mut deps = Vec::new();
    let mut refs = Vec::new();
    let mut spec = Vec::new();
    let mut note = None;
    let mut added = None;
    let mut resolved = None;
    let mut conflict = None;

    for m in &task.metadata {
        match m {
            Metadata::Dep(d) => deps.extend(d.iter().cloned()),
            Metadata::Ref(r) => refs.extend(r.iter().cloned()),
            Metadata::Spec(s) => spec.extend(s.iter().cloned()),
            Metadata::Note(n) => note = Some(n.clone()),
            Metadata::Added(a) => added = Some(a.clone()),
            Metadata::Resolved(r) => resolved = Some(r.clone()),
            Metadata::Conflict(c) => conflict = Some(c.clone()),
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
        conflict,
        archived: None,
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
pub fn format_task_detail(task: &Task, archived: Option<&ArchivedIn>) -> Vec<String> {
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

    lines.extend(format_archived_line(archived, ""));
    lines.extend(format_metadata_lines(task, ""));

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
///
/// `archived` describes the file the whole block came from, so it prints under
/// the target task and not under each ancestor — an ancestor is in the same
/// archive by construction, and saying so once is saying it.
pub fn format_task_detail_with_context(
    ancestors: &[&Task],
    task: &Task,
    archived: Option<&ArchivedIn>,
) -> Vec<String> {
    let mut lines = Vec::new();

    for ancestor in ancestors {
        lines.push(format_context_separator("Parent", ancestor));
        lines.extend(format_context_fields(ancestor, None));
        lines.push(String::new());
    }

    lines.push(format_context_separator("Task", task));
    lines.extend(format_context_fields(task, archived));

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
fn format_context_fields(task: &Task, archived: Option<&ArchivedIn>) -> Vec<String> {
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

    lines.extend(format_archived_line(archived, "  "));
    lines.extend(format_metadata_lines(task, "  "));

    lines
}

/// The `archived:` line, or nothing for a live task.
///
/// **First of the field lines, ahead of `conflict:`.** The documented order —
/// `conflict`, `added`, `resolved`, `dep`, `spec`, `ref`, `note` — is
/// [`Metadata::rank`]'s, and this is not metadata: it is not in the file and no
/// write puts it there. Printing it above that sequence leaves the sequence
/// intact and contiguous, and puts the one fact that qualifies every line under
/// it — that this record is not live and no write command will touch it — where
/// it is read first rather than after a note of unbounded length.
fn format_archived_line(archived: Option<&ArchivedIn>, indent: &str) -> Vec<String> {
    archived
        .map(|a| format!("{indent}archived: {}", a.value()))
        .into_iter()
        .collect()
}

/// A task's metadata as display lines, in canonical order, each prefixed with
/// `indent`.
///
/// **One implementation, two consumers** — [`format_task_detail`] passes `""`
/// and [`format_context_fields`] passes `"  "`. They were the same match written
/// twice, differing in nothing but that indent, which is the shape
/// `FilteredTasks` further down this file exists to stop repeating: two code
/// paths answering one question drift, and `b664a3e` is what that costs.
///
/// Order comes from [`ordered_metadata`], not from the file: a field appended
/// after a note otherwise renders past the end of it.
fn format_metadata_lines(task: &Task, indent: &str) -> Vec<String> {
    let mut lines = Vec::new();
    for m in ordered_metadata(task) {
        match m {
            Metadata::Conflict(c) => lines.push(format!("{indent}conflict: {c}")),
            Metadata::Added(d) => lines.push(format!("{indent}added: {d}")),
            Metadata::Resolved(d) => lines.push(format!("{indent}resolved: {d}")),
            Metadata::Dep(deps) => lines.push(format!("{indent}dep: {}", deps.join(", "))),
            Metadata::Spec(specs) => {
                for s in specs {
                    lines.push(format!("{indent}spec: {s}"));
                }
            }
            Metadata::Ref(refs) => {
                for r in refs {
                    lines.push(format!("{indent}ref: {r}"));
                }
            }
            Metadata::Note(n) => {
                lines.push(format!("{indent}note:"));
                for line in n.lines() {
                    lines.push(format!("{indent}  {line}"));
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::TaskId;

    fn task_with(metadata: Vec<Metadata>) -> Task {
        let mut task = Task::new(
            TaskState::Done,
            Some(TaskId::parse("M-001")),
            "A task".into(),
        );
        task.metadata = metadata;
        task
    }

    /// The defect this came from: `resolved:` is appended when a task is
    /// completed, so a task carrying a long note printed its completion date
    /// after the whole note — 55 lines down on the task that surfaced it, which
    /// reads as though the date were missing.
    #[test]
    fn a_note_written_first_still_renders_last() {
        let task = task_with(vec![
            Metadata::Note("body".into()),
            Metadata::Resolved("2025-05-14".into()),
            Metadata::Added("2025-05-01".into()),
        ]);
        let lines = format_task_detail(&task, None);
        let keys: Vec<&str> = lines
            .iter()
            .filter_map(|l| l.split_once(':').map(|(k, _)| k))
            .filter(|k| ["added", "resolved", "note"].contains(k))
            .collect();
        assert_eq!(keys, ["added", "resolved", "note"], "{lines:?}");
    }

    /// Both human forms order the same way — they are one implementation, and
    /// this is what says so if someone splits them again.
    #[test]
    fn the_context_form_orders_identically() {
        let task = task_with(vec![
            Metadata::Note("body".into()),
            Metadata::Added("2025-05-01".into()),
            Metadata::Conflict("both-edited 2026-08-03T04:08:38Z".into()),
        ]);
        let plain: Vec<String> = format_task_detail(&task, None)
            .iter()
            .filter_map(|l| l.split_once(':').map(|(k, _)| k.trim().to_string()))
            .collect();
        let context: Vec<String> = format_context_fields(&task, None)
            .iter()
            .filter_map(|l| l.split_once(':').map(|(k, _)| k.trim().to_string()))
            .collect();
        assert!(context.starts_with(&["state".to_string()]), "{context:?}");
        assert_eq!(plain, context[1..], "{plain:?} vs {context:?}");
    }

    /// `archived:` leads the field lines in both human forms, and says the same
    /// thing `--json` does.
    ///
    /// Placement is the point: it is not metadata, so it sits ahead of the
    /// documented `conflict … note` sequence rather than inside it, and ahead of
    /// a note it cannot be pushed past.
    #[test]
    fn the_archived_line_leads_the_fields_in_both_forms() {
        let task = task_with(vec![
            Metadata::Note("body".into()),
            Metadata::Conflict("both-edited 2026-08-03T04:08:38Z".into()),
        ]);
        let origin = ArchivedIn::new("bac", "archive/bac.md");

        let plain = format_task_detail(&task, Some(&origin));
        let fields: Vec<&str> = plain
            .iter()
            .filter_map(|l| l.split_once(':').map(|(k, _)| k.trim()))
            .filter(|k| ["archived", "conflict", "note"].contains(k))
            .collect();
        assert_eq!(fields, ["archived", "conflict", "note"], "{plain:?}");
        assert_eq!(
            plain.iter().find(|l| l.starts_with("archived:")),
            Some(&"archived: bac (frame/archive/bac.md)".to_string()),
            "{plain:?}"
        );

        // The context form indents the same line, under the shown task only.
        let context = format_task_detail_with_context(&[&task], &task, Some(&origin));
        assert_eq!(
            context
                .iter()
                .filter(|l| l.trim_start().starts_with("archived:"))
                .collect::<Vec<_>>(),
            ["  archived: bac (frame/archive/bac.md)"],
            "{context:?}"
        );
    }

    /// Ordering must not drop a field, and must not shuffle two entries that
    /// share a key. An *unknown* metadata key parses to a `Note` carrying its own
    /// `key: value` text, so a task really can hold several notes, and their
    /// order is text a user wrote.
    #[test]
    fn duplicate_keys_keep_their_relative_order() {
        let task = task_with(vec![
            Metadata::Note("first".into()),
            Metadata::Added("2025-05-01".into()),
            Metadata::Note("second".into()),
        ]);
        let lines = format_task_detail(&task, None);
        let bodies: Vec<&String> = lines.iter().filter(|l| l.starts_with("  ")).collect();
        assert_eq!(bodies, ["  first", "  second"], "{lines:?}");
    }
}
