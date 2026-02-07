use chrono::Local;

use crate::model::task::{Metadata, Task};
use crate::model::track::{SectionKind, Track};
use crate::ops::task_ops::{find_max_id_in_track, InsertPosition, TaskError};
use crate::parse::parse_tasks;

/// Error type for import operations
#[derive(Debug, thiserror::Error)]
pub enum ImportError {
    #[error("no tasks found in import file")]
    NoTasks,
    #[error("task error: {0}")]
    TaskError(#[from] TaskError),
}

/// Result of an import operation
#[derive(Debug)]
pub struct ImportResult {
    /// The IDs assigned to the imported top-level tasks
    pub assigned_ids: Vec<String>,
    /// Total number of tasks imported (including subtasks)
    pub total_count: usize,
}

/// Parse markdown text as a list of tasks and insert into a track's backlog.
/// Auto-assigns IDs using the given prefix and sets `added:` dates where missing.
/// Returns the assigned IDs and total count of imported tasks.
pub fn import_tasks(
    markdown: &str,
    track: &mut Track,
    position: InsertPosition,
    prefix: &str,
) -> Result<ImportResult, ImportError> {
    let lines: Vec<String> = markdown.lines().map(|l| l.to_string()).collect();

    // Parse all top-level tasks from the file, skipping non-task content
    let tasks = parse_all_tasks(&lines);

    if tasks.is_empty() {
        return Err(ImportError::NoTasks);
    }

    // Find the next available ID number for this prefix
    let mut next_num = {
        let mut max = 0usize;
        let prefix_dash = format!("{}-", prefix);
        find_max_id_in_track(track, &prefix_dash, &mut max);
        max + 1
    };

    let today = today_str();
    let mut assigned_ids = Vec::new();
    let mut total_count = 0;

    // Prepare tasks: assign IDs, set dates, mark dirty
    let mut prepared_tasks = Vec::new();
    for mut task in tasks {
        let id = format!("{}-{:03}", prefix, next_num);
        task.id = Some(id.clone());
        task.depth = 0;
        task.mark_dirty();

        // Add `added:` date if not already present
        if !task.metadata.iter().any(|m| m.key() == "added") {
            task.metadata.insert(0, Metadata::Added(today.clone()));
        }

        // Recursively assign subtask IDs and dates
        assign_subtask_ids(&mut task, &id, &today);

        total_count += 1 + count_subtasks(&task);
        assigned_ids.push(id);
        prepared_tasks.push(task);
        next_num += 1;
    }

    // Insert into the backlog section
    let backlog = track
        .section_tasks_mut(SectionKind::Backlog)
        .ok_or(ImportError::TaskError(TaskError::InvalidPosition(
            "no backlog section".into(),
        )))?;

    match &position {
        InsertPosition::Bottom => {
            backlog.extend(prepared_tasks);
        }
        InsertPosition::Top => {
            for (i, task) in prepared_tasks.into_iter().enumerate() {
                backlog.insert(i, task);
            }
        }
        InsertPosition::After(after_id) => {
            let idx = backlog
                .iter()
                .position(|t| t.id.as_deref() == Some(after_id.as_str()))
                .ok_or(ImportError::TaskError(TaskError::NotFound(format!(
                    "after target {}",
                    after_id
                ))))?;
            for (i, task) in prepared_tasks.into_iter().enumerate() {
                backlog.insert(idx + 1 + i, task);
            }
        }
    }

    Ok(ImportResult {
        assigned_ids,
        total_count,
    })
}

/// Parse all top-level tasks from a markdown file, skipping non-task lines
/// (headers, blank lines, descriptions) between task groups.
fn parse_all_tasks(lines: &[String]) -> Vec<Task> {
    let mut all_tasks = Vec::new();
    let mut idx = 0;

    while idx < lines.len() {
        // Skip non-task lines (headers, blank lines, descriptions)
        if is_task_line(&lines[idx], 0) {
            let (tasks, next_idx) = parse_tasks(lines, idx, 0, 0);
            all_tasks.extend(tasks);
            idx = next_idx;
        } else {
            idx += 1;
        }
    }

    all_tasks
}

/// Check if a line is a top-level task line (starts with `- [` at indent 0)
fn is_task_line(line: &str, indent: usize) -> bool {
    let line_indent = line.len() - line.trim_start_matches(' ').len();
    if line_indent != indent {
        return false;
    }
    let content = &line[indent..];
    content.starts_with("- [") && content.len() >= 5 && content.as_bytes().get(4) == Some(&b']')
}

/// Recursively assign subtask IDs and `added:` dates.
fn assign_subtask_ids(task: &mut Task, parent_id: &str, today: &str) {
    for (i, sub) in task.subtasks.iter_mut().enumerate() {
        let sub_id = format!("{}.{}", parent_id, i + 1);
        sub.id = Some(sub_id.clone());
        sub.depth = task.depth + 1;
        sub.mark_dirty();

        if !sub.metadata.iter().any(|m| m.key() == "added") {
            sub.metadata.insert(0, Metadata::Added(today.to_string()));
        }

        assign_subtask_ids(sub, &sub_id, today);
    }
}

/// Count all subtasks (recursively) of a task.
fn count_subtasks(task: &Task) -> usize {
    let mut count = task.subtasks.len();
    for sub in &task.subtasks {
        count += count_subtasks(sub);
    }
    count
}

fn today_str() -> String {
    Local::now().format("%Y-%m-%d").to_string()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ops::task_ops::find_task_in_track;
    use crate::parse::parse_track;

    fn sample_track() -> Track {
        parse_track(
            "\
# Test

## Backlog

- [ ] `T-001` First task
- [ ] `T-002` Second task

## Done
",
        )
    }

    fn simple_import_md() -> &'static str {
        "\
- [ ] Imported task one #ready
- [ ] Imported task two #design
- [ ] Imported task three
"
    }

    fn import_with_subtasks_md() -> &'static str {
        "\
- [ ] Parent task #ready
  - [ ] Sub one
  - [ ] Sub two
- [ ] Another top-level task
"
    }

    fn import_with_headers_md() -> &'static str {
        "\
# Tasks to import

Some description text here.

- [ ] Task after header #bug
- [ ] Second task after header
"
    }

    fn import_with_metadata_md() -> &'static str {
        "\
- [ ] Task with existing metadata
  - added: 2025-01-15
  - dep: EXT-001
  - note: Some existing note
- [ ] Task without metadata
"
    }

    fn import_with_blank_lines_md() -> &'static str {
        "\
- [ ] First group task one
- [ ] First group task two

- [ ] Second group task
"
    }

    // --- Basic import ---

    #[test]
    fn test_import_bottom() {
        let mut track = sample_track();
        let result =
            import_tasks(simple_import_md(), &mut track, InsertPosition::Bottom, "T").unwrap();

        assert_eq!(result.assigned_ids, vec!["T-003", "T-004", "T-005"]);
        assert_eq!(result.total_count, 3);

        let backlog = track.backlog();
        assert_eq!(backlog.len(), 5);
        assert_eq!(backlog[2].title, "Imported task one");
        assert_eq!(backlog[2].id.as_deref(), Some("T-003"));
        assert!(backlog[2].tags.contains(&"ready".to_string()));
        assert_eq!(backlog[3].id.as_deref(), Some("T-004"));
        assert_eq!(backlog[4].id.as_deref(), Some("T-005"));

        // All should have added dates
        for task in &backlog[2..] {
            assert!(task.metadata.iter().any(|m| m.key() == "added"));
        }
    }

    #[test]
    fn test_import_top() {
        let mut track = sample_track();
        let result =
            import_tasks(simple_import_md(), &mut track, InsertPosition::Top, "T").unwrap();

        assert_eq!(result.assigned_ids.len(), 3);

        let backlog = track.backlog();
        assert_eq!(backlog.len(), 5);
        assert_eq!(backlog[0].title, "Imported task one");
        assert_eq!(backlog[1].title, "Imported task two");
        assert_eq!(backlog[2].title, "Imported task three");
        assert_eq!(backlog[3].id.as_deref(), Some("T-001"));
        assert_eq!(backlog[4].id.as_deref(), Some("T-002"));
    }

    #[test]
    fn test_import_after() {
        let mut track = sample_track();
        let result = import_tasks(
            simple_import_md(),
            &mut track,
            InsertPosition::After("T-001".into()),
            "T",
        )
        .unwrap();

        assert_eq!(result.assigned_ids.len(), 3);

        let backlog = track.backlog();
        assert_eq!(backlog.len(), 5);
        assert_eq!(backlog[0].id.as_deref(), Some("T-001"));
        assert_eq!(backlog[1].title, "Imported task one");
        assert_eq!(backlog[2].title, "Imported task two");
        assert_eq!(backlog[3].title, "Imported task three");
        assert_eq!(backlog[4].id.as_deref(), Some("T-002"));
    }

    // --- Subtasks ---

    #[test]
    fn test_import_with_subtasks() {
        let mut track = sample_track();
        let result = import_tasks(
            import_with_subtasks_md(),
            &mut track,
            InsertPosition::Bottom,
            "T",
        )
        .unwrap();

        assert_eq!(result.assigned_ids, vec!["T-003", "T-004"]);
        assert_eq!(result.total_count, 4); // 2 top-level + 2 subtasks

        let parent = find_task_in_track(&track, "T-003").unwrap();
        assert_eq!(parent.title, "Parent task");
        assert_eq!(parent.subtasks.len(), 2);
        assert_eq!(parent.subtasks[0].id.as_deref(), Some("T-003.1"));
        assert_eq!(parent.subtasks[0].title, "Sub one");
        assert_eq!(parent.subtasks[1].id.as_deref(), Some("T-003.2"));
        assert_eq!(parent.subtasks[1].title, "Sub two");

        // Subtasks should also have added dates
        assert!(parent.subtasks[0]
            .metadata
            .iter()
            .any(|m| m.key() == "added"));
    }

    // --- Headers and non-task content ---

    #[test]
    fn test_import_skips_headers() {
        let mut track = sample_track();
        let result = import_tasks(
            import_with_headers_md(),
            &mut track,
            InsertPosition::Bottom,
            "T",
        )
        .unwrap();

        assert_eq!(result.assigned_ids.len(), 2);
        assert_eq!(result.total_count, 2);

        let backlog = track.backlog();
        assert_eq!(backlog[2].title, "Task after header");
        assert!(backlog[2].tags.contains(&"bug".to_string()));
    }

    // --- Preserves existing metadata ---

    #[test]
    fn test_import_preserves_existing_metadata() {
        let mut track = sample_track();
        let result = import_tasks(
            import_with_metadata_md(),
            &mut track,
            InsertPosition::Bottom,
            "T",
        )
        .unwrap();

        assert_eq!(result.assigned_ids.len(), 2);

        let task = find_task_in_track(&track, "T-003").unwrap();
        // Should keep the existing added date
        assert!(task
            .metadata
            .iter()
            .any(|m| matches!(m, Metadata::Added(d) if d == "2025-01-15")));
        // Should keep deps
        assert!(task
            .metadata
            .iter()
            .any(|m| matches!(m, Metadata::Dep(d) if d.contains(&"EXT-001".to_string()))));
        // Should keep note
        assert!(task
            .metadata
            .iter()
            .any(|m| matches!(m, Metadata::Note(n) if n.contains("existing note"))));

        // Task without metadata should get today's date
        let task2 = find_task_in_track(&track, "T-004").unwrap();
        assert!(task2.metadata.iter().any(|m| m.key() == "added"));
    }

    // --- Blank line separation ---

    #[test]
    fn test_import_with_blank_lines_between_tasks() {
        let mut track = sample_track();
        let result = import_tasks(
            import_with_blank_lines_md(),
            &mut track,
            InsertPosition::Bottom,
            "T",
        )
        .unwrap();

        assert_eq!(result.assigned_ids.len(), 3);
        assert_eq!(result.total_count, 3);

        let backlog = track.backlog();
        assert_eq!(backlog[2].title, "First group task one");
        assert_eq!(backlog[3].title, "First group task two");
        assert_eq!(backlog[4].title, "Second group task");
    }

    // --- Error cases ---

    #[test]
    fn test_import_empty_file() {
        let mut track = sample_track();
        let result = import_tasks("", &mut track, InsertPosition::Bottom, "T");
        assert!(matches!(result, Err(ImportError::NoTasks)));
    }

    #[test]
    fn test_import_no_tasks_in_file() {
        let mut track = sample_track();
        let result = import_tasks(
            "# Just a header\n\nSome text but no tasks.\n",
            &mut track,
            InsertPosition::Bottom,
            "T",
        );
        assert!(matches!(result, Err(ImportError::NoTasks)));
    }

    #[test]
    fn test_import_after_nonexistent_id() {
        let mut track = sample_track();
        let result = import_tasks(
            simple_import_md(),
            &mut track,
            InsertPosition::After("T-999".into()),
            "T",
        );
        assert!(result.is_err());
    }

    // --- All tasks are marked dirty ---

    #[test]
    fn test_imported_tasks_are_dirty() {
        let mut track = sample_track();
        import_tasks(
            import_with_subtasks_md(),
            &mut track,
            InsertPosition::Bottom,
            "T",
        )
        .unwrap();

        let parent = find_task_in_track(&track, "T-003").unwrap();
        assert!(parent.dirty);
        assert!(parent.subtasks[0].dirty);
        assert!(parent.subtasks[1].dirty);
    }

    // --- ID numbering continues from track max ---

    #[test]
    fn test_import_id_continues_from_max() {
        let mut track = parse_track(
            "\
# Test

## Backlog

- [ ] `T-050` Existing task

## Done
",
        );
        let result =
            import_tasks(simple_import_md(), &mut track, InsertPosition::Bottom, "T").unwrap();

        assert_eq!(result.assigned_ids, vec!["T-051", "T-052", "T-053"]);
    }

    // --- Deep nesting (3-level) ---

    #[test]
    fn test_import_three_level_nesting() {
        let md = "\
- [ ] Top level #ready
  - [ ] Sub level
    - [ ] Sub-sub level
";
        let mut track = sample_track();
        let result = import_tasks(md, &mut track, InsertPosition::Bottom, "T").unwrap();

        assert_eq!(result.assigned_ids, vec!["T-003"]);
        assert_eq!(result.total_count, 3);

        let top = find_task_in_track(&track, "T-003").unwrap();
        assert_eq!(top.subtasks.len(), 1);
        assert_eq!(top.subtasks[0].id.as_deref(), Some("T-003.1"));
        assert_eq!(top.subtasks[0].subtasks.len(), 1);
        assert_eq!(
            top.subtasks[0].subtasks[0].id.as_deref(),
            Some("T-003.1.1")
        );
    }
}
