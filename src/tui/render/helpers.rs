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

#[cfg(test)]
mod tests {
    use super::abbreviated_id;

    #[test]
    fn abbreviated_id_keeps_token_in_subtask_tail() {
        // The tail is a string slice, so the subtask token survives: the
        // tail of `EFF-a14.b2` is `.b2` (not `.2`).
        assert_eq!(abbreviated_id("EFF-a14.b2"), ".b2");
        assert_eq!(abbreviated_id("EFF-a14.b2.c3"), ".b2.c3");
        // Null-namespace subtasks abbreviate as before.
        assert_eq!(abbreviated_id("EFF-014.2"), ".2");
    }

    #[test]
    fn abbreviated_id_renders_top_level_tokened_id_in_full() {
        // A top-level tokened id has no `.` after the prefix, so it renders
        // verbatim, token included.
        assert_eq!(abbreviated_id("EFF-a14"), "EFF-a14");
        assert_eq!(abbreviated_id("EFF-14"), "EFF-14");
    }
}
