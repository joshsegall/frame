use crate::model::task::{Metadata, Task};

/// Serialize a list of tasks to markdown lines.
/// `indent` is the number of spaces for the current nesting level.
pub fn serialize_tasks(tasks: &[Task], indent: usize) -> Vec<String> {
    let mut lines = Vec::new();
    for task in tasks {
        serialize_task(task, indent, &mut lines);
    }
    lines
}

/// Serialize a single task. If clean (not dirty), emit verbatim source.
/// If dirty, emit canonical format.
fn serialize_task(task: &Task, indent: usize, lines: &mut Vec<String>) {
    if !task.dirty {
        // Emit verbatim source lines
        if let Some(ref source) = task.source_text {
            lines.extend(source.iter().cloned());
            return;
        }
    }

    // Canonical format
    let indent_str = " ".repeat(indent);

    // Task line: `- [X] \`ID\` Title #tag1 #tag2`
    let mut task_line = format!(
        "{}- [{}]",
        indent_str,
        task.state.checkbox_char()
    );

    if let Some(ref id) = task.id {
        task_line.push_str(&format!(" `{}`", id));
    }

    task_line.push(' ');
    task_line.push_str(&task.title);

    for tag in &task.tags {
        task_line.push_str(&format!(" #{}", tag));
    }

    lines.push(task_line);

    // Metadata lines at indent + 2
    let meta_indent = " ".repeat(indent + 2);
    for meta in &task.metadata {
        match meta {
            Metadata::Added(date) => {
                lines.push(format!("{}- added: {}", meta_indent, date));
            }
            Metadata::Resolved(date) => {
                lines.push(format!("{}- resolved: {}", meta_indent, date));
            }
            Metadata::Dep(deps) => {
                lines.push(format!("{}- dep: {}", meta_indent, deps.join(", ")));
            }
            Metadata::Ref(refs) => {
                lines.push(format!("{}- ref: {}", meta_indent, refs.join(", ")));
            }
            Metadata::Spec(spec) => {
                lines.push(format!("{}- spec: {}", meta_indent, spec));
            }
            Metadata::Note(note) => {
                if note.contains('\n') {
                    // Multiline note
                    lines.push(format!("{}- note:", meta_indent));
                    let block_indent = " ".repeat(indent + 4);
                    for note_line in note.lines() {
                        if note_line.is_empty() {
                            lines.push(String::new());
                        } else {
                            lines.push(format!("{}{}", block_indent, note_line));
                        }
                    }
                } else {
                    // Single-line note
                    lines.push(format!("{}- note: {}", meta_indent, note));
                }
            }
        }
    }

    // Subtasks at indent + 2
    for subtask in &task.subtasks {
        serialize_task(subtask, indent + 2, lines);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::task::TaskState;

    #[test]
    fn test_serialize_minimal_task() {
        let task = Task::new(TaskState::Todo, None, "Fix parser crash".to_string());
        let lines = serialize_tasks(&[task], 0);
        assert_eq!(lines, vec!["- [ ] Fix parser crash"]);
    }

    #[test]
    fn test_serialize_task_with_id_and_tags() {
        let mut task = Task::new(
            TaskState::Active,
            Some("EFF-014".to_string()),
            "Implement effect inference".to_string(),
        );
        task.tags = vec!["ready".to_string(), "cc".to_string()];
        let lines = serialize_tasks(&[task], 0);
        assert_eq!(
            lines,
            vec!["- [>] `EFF-014` Implement effect inference #ready #cc"]
        );
    }

    #[test]
    fn test_serialize_task_with_metadata() {
        let mut task = Task::new(
            TaskState::Active,
            Some("EFF-014".to_string()),
            "Test task".to_string(),
        );
        task.metadata = vec![
            Metadata::Added("2025-05-10".to_string()),
            Metadata::Dep(vec!["EFF-003".to_string(), "INFRA-007".to_string()]),
            Metadata::Spec("doc/spec/effects.md#closures".to_string()),
        ];
        let lines = serialize_tasks(&[task], 0);
        assert_eq!(lines[0], "- [>] `EFF-014` Test task");
        assert_eq!(lines[1], "  - added: 2025-05-10");
        assert_eq!(lines[2], "  - dep: EFF-003, INFRA-007");
        assert_eq!(lines[3], "  - spec: doc/spec/effects.md#closures");
    }

    #[test]
    fn test_serialize_multiline_note() {
        let mut task = Task::new(TaskState::Todo, None, "Test".to_string());
        task.metadata = vec![Metadata::Note(
            "First line.\n\nSecond paragraph.\n1. Item one".to_string(),
        )];
        let lines = serialize_tasks(&[task], 0);
        assert_eq!(lines[0], "- [ ] Test");
        assert_eq!(lines[1], "  - note:");
        assert_eq!(lines[2], "    First line.");
        assert_eq!(lines[3], "");
        assert_eq!(lines[4], "    Second paragraph.");
        assert_eq!(lines[5], "    1. Item one");
    }

    #[test]
    fn test_serialize_subtasks() {
        let mut parent = Task::new(
            TaskState::Active,
            Some("T-001".to_string()),
            "Parent".to_string(),
        );
        parent.subtasks = vec![
            Task::new(TaskState::Todo, Some("T-001.1".to_string()), "Sub 1".to_string()),
            Task::new(TaskState::Todo, Some("T-001.2".to_string()), "Sub 2".to_string()),
        ];
        let lines = serialize_tasks(&[parent], 0);
        assert_eq!(lines[0], "- [>] `T-001` Parent");
        assert_eq!(lines[1], "  - [ ] `T-001.1` Sub 1");
        assert_eq!(lines[2], "  - [ ] `T-001.2` Sub 2");
    }

    #[test]
    fn test_serialize_verbatim_when_clean() {
        let mut task = Task::new(TaskState::Todo, None, "Test".to_string());
        task.dirty = false;
        task.source_text = Some(vec![
            "- [ ] Test  ".to_string(), // note: trailing spaces preserved
            "  - added: 2025-01-01".to_string(),
        ]);
        let lines = serialize_tasks(&[task], 0);
        assert_eq!(lines[0], "- [ ] Test  ");
        assert_eq!(lines[1], "  - added: 2025-01-01");
    }
}
