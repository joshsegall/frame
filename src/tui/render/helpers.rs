use ratatui::text::Span;

use crate::model::task::{Metadata, Task, TaskState};
use crate::util::unicode;

/// State symbols for each task state (markdown checkbox style)
pub(super) fn state_symbol(state: TaskState) -> &'static str {
    match state {
        TaskState::Todo => "[ ]",
        TaskState::Active => "[>]",
        TaskState::Blocked => "[-]",
        TaskState::Done => "[x]",
        TaskState::Parked => "[~]",
    }
}

/// Get abbreviated ID (e.g., "EFF-014.2" -> ".2")
pub(super) fn abbreviated_id(id: &str) -> &str {
    if let Some(dash_pos) = id.find('-') {
        let after_prefix = &id[dash_pos + 1..];
        if let Some(dot_pos) = after_prefix.find('.') {
            return &after_prefix[dot_pos..];
        }
    }
    id
}

/// Collect metadata list items (deps or refs) from a task
pub(super) fn collect_metadata_list(
    task: &Task,
    f: impl Fn(&Metadata) -> Option<&Vec<String>>,
) -> Vec<String> {
    let mut result = Vec::new();
    for meta in &task.metadata {
        if let Some(items) = f(meta) {
            result.extend(items.clone());
        }
    }
    result
}

/// Compute total display width of a slice of spans
pub(super) fn spans_width(spans: &[Span]) -> usize {
    spans
        .iter()
        .map(|s| unicode::display_width(&s.content))
        .sum()
}
