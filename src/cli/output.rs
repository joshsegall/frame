use serde::Serialize;

use crate::model::task::{Metadata, Task, TaskState};
use crate::model::track::Track;
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
        id: task.id.clone(),
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

/// Format a track listing header
pub fn format_track_header(track_id: &str, track: &Track) -> String {
    format!("== {} ({}) ==", track.title, track_id)
}

/// Format track info for the tracks listing
pub fn format_track_info(
    track_id: &str,
    name: &str,
    state: &str,
    cc_focus: bool,
    stats: &TrackStats,
) -> String {
    let cc_str = if cc_focus { " ★cc" } else { "" };
    format!(
        "  {} ({}) [{}]  {}▸ {}⊘ {}○ {}◇ {}✓{}",
        name,
        track_id,
        state,
        stats.active,
        stats.blocked,
        stats.todo,
        stats.parked,
        stats.done,
        cc_str
    )
}

/// Format a track's task listing
pub fn format_track_listing(
    track_id: &str,
    track: &Track,
    state_filter: Option<TaskState>,
    tag_filter: Option<&str>,
) -> Vec<String> {
    let mut lines = Vec::new();
    lines.push(format_track_header(track_id, track));
    lines.push(String::new());

    let backlog = track.backlog();
    let parked = track.parked();

    let filter = |task: &&Task| -> bool {
        if let Some(sf) = state_filter {
            if task.state != sf {
                return false;
            }
        }
        if let Some(tf) = tag_filter {
            if !task.tags.iter().any(|t| t == tf) {
                return false;
            }
        }
        true
    };

    let filtered_backlog: Vec<_> = backlog.iter().filter(filter).collect();
    let filtered_parked: Vec<_> = parked.iter().filter(filter).collect();

    for task in &filtered_backlog {
        for line in format_task_tree(task, 0) {
            lines.push(line);
        }
    }

    if !filtered_parked.is_empty() {
        if !filtered_backlog.is_empty() {
            lines.push(String::new());
        }
        lines.push("-- Parked --".to_string());
        for task in &filtered_parked {
            for line in format_task_tree(task, 0) {
                lines.push(line);
            }
        }
    }

    lines
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
