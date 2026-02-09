use crate::model::task::{Metadata, Task, TaskState};
use crate::parse::has_continuation_at_indent;

/// Maximum nesting depth (3 levels: top, sub, sub-sub)
const MAX_DEPTH: usize = 3;

/// Parse task lines starting from `start_idx` at the given `indent` level.
/// Returns parsed tasks and the line index where parsing stopped.
pub fn parse_tasks(
    lines: &[String],
    start_idx: usize,
    indent: usize,
    depth: usize,
) -> (Vec<Task>, usize) {
    let mut tasks = Vec::new();
    let mut idx = start_idx;

    while idx < lines.len() {
        let line = &lines[idx];

        // Check if this line is a task at the expected indent level
        if let Some(task_indent) = task_indent(line) {
            if task_indent == indent {
                let (task, next_idx) = parse_single_task(lines, idx, indent, depth);
                tasks.push(task);
                idx = next_idx;
            } else if task_indent < indent {
                // Dedented — we're done with this nesting level
                break;
            } else {
                // More indented than expected but not part of a task above — skip
                idx += 1;
            }
        } else {
            // Not a task line. Blank lines and orphaned deeper-indent content
            // can appear between tasks (e.g., after multi-line notes with
            // trailing blank lines, or orphaned subtasks from previous parse
            // errors). Skip past them if more tasks at our indent follow.
            if (line.trim().is_empty() || count_indent(line) > indent)
                && has_more_tasks_at_indent(lines, idx + 1, indent)
            {
                idx += 1;
                continue;
            }
            break;
        }
    }

    (tasks, idx)
}

/// Parse a single task and all its metadata and subtasks.
/// Returns the task and the next line index to process.
fn parse_single_task(
    lines: &[String],
    start_idx: usize,
    indent: usize,
    depth: usize,
) -> (Task, usize) {
    let line = &lines[start_idx];
    let (state, id, title, tags) = parse_task_line(line, indent);

    let mut task = Task {
        state,
        id,
        title,
        tags,
        metadata: Vec::new(),
        subtasks: Vec::new(),
        depth,
        source_lines: None,
        source_text: None,
        dirty: false,
    };

    let mut idx = start_idx + 1;
    let meta_indent = indent + 2;

    // Parse metadata lines (before subtasks)
    while idx < lines.len() {
        let line = &lines[idx];

        // If we hit a subtask line at the expected indent, stop collecting metadata
        if let Some(ti) = task_indent(line)
            && ti <= meta_indent
        {
            break;
        }

        // Check for metadata line at meta_indent
        if is_metadata_line(line, meta_indent) {
            let (meta, next_idx) = parse_metadata(lines, idx, meta_indent);
            task.metadata.push(meta);
            idx = next_idx;
            continue;
        }

        // Check if this is a continuation line at deeper indent (shouldn't happen
        // in well-formed input, but stop parsing)
        let line_indent = count_indent(line);
        if line_indent > indent && !line.trim().is_empty() {
            idx += 1;
            continue;
        }

        // Blank line — check if more metadata or subtasks follow.
        // This handles multi-line notes with trailing blank lines before subtasks,
        // and empty notes (- note:\n\n) followed by more metadata.
        if line.trim().is_empty() {
            let mut peek = idx + 1;
            while peek < lines.len() && lines[peek].trim().is_empty() {
                peek += 1;
            }
            if peek < lines.len()
                && (is_metadata_line(&lines[peek], meta_indent)
                    || task_indent(&lines[peek]).is_some_and(|ti| ti == meta_indent))
            {
                idx += 1;
                continue;
            }
        }

        // Unrecognized content or end of task — stop
        break;
    }

    // Record the task's OWN source text (task line + metadata, NOT subtask lines).
    // This enables selective rewrite: editing a subtask doesn't reformat the parent.
    let own_end_idx = idx;
    task.source_text = Some(lines[start_idx..own_end_idx].to_vec());

    // Now parse subtasks (they get their own independent source_text)
    if idx < lines.len()
        && let Some(ti) = task_indent(&lines[idx])
        && ti == meta_indent
        && depth + 1 < MAX_DEPTH
    {
        let (subtasks, next_idx) = parse_tasks(lines, idx, meta_indent, depth + 1);
        task.subtasks = subtasks;
        idx = next_idx;
    }

    task.source_lines = Some(start_idx..idx);

    (task, idx)
}

/// Parse the task line itself: `- [x] \`ID\` Title text #tag1 #tag2`
fn parse_task_line(line: &str, indent: usize) -> (TaskState, Option<String>, String, Vec<String>) {
    let content = &line[indent..];

    // Parse checkbox: `- [X] `
    let state_char = content
        .strip_prefix("- [")
        .and_then(|rest| rest.chars().next())
        .unwrap_or(' ');
    let state = TaskState::from_checkbox_char(state_char).unwrap_or(TaskState::Todo);

    // Skip past `- [X] `
    let after_checkbox = &content[5..]; // "- [X] " is 5 chars: "- [" + char + "]"
    let after_checkbox = after_checkbox.strip_prefix(' ').unwrap_or(after_checkbox);

    // Parse optional ID: `\`PREFIX-NNN\``
    let (id, after_id) = if let Some(after_tick) = after_checkbox.strip_prefix('`') {
        if let Some(end_tick) = after_tick.find('`') {
            let id_text = &after_tick[..end_tick];
            let rest = &after_tick[end_tick + 1..];
            let rest = rest.strip_prefix(' ').unwrap_or(rest);
            (Some(id_text.to_string()), rest)
        } else {
            (None, after_checkbox)
        }
    } else {
        (None, after_checkbox)
    };

    // Parse tags from end of line, then title is everything before tags
    let (title, tags) = parse_title_and_tags(after_id);

    (state, id, title, tags)
}

/// Split a string into title and tags. Tags are `#word` tokens at the end.
pub fn parse_title_and_tags(s: &str) -> (String, Vec<String>) {
    let s = s.trim_end();
    if s.is_empty() {
        return (String::new(), Vec::new());
    }

    // Collect tags from the end
    let mut tags = Vec::new();
    let mut remaining = s;

    loop {
        let trimmed = remaining.trim_end();
        if trimmed.is_empty() {
            break;
        }

        // Find the last word
        if let Some(last_space) = trimmed.rfind(' ') {
            let last_word = &trimmed[last_space + 1..];
            if let Some(tag) = last_word.strip_prefix('#')
                && !tag.is_empty()
                && !tag.contains('#')
            {
                tags.push(tag.to_string());
                remaining = &trimmed[..last_space];
                continue;
            }
        } else {
            // Single word — check if it's a tag
            if let Some(tag) = trimmed.strip_prefix('#')
                && !tag.is_empty()
                && !tag.contains('#')
            {
                tags.push(tag.to_string());
                remaining = "";
                continue;
            }
        }
        break;
    }

    tags.reverse();
    (remaining.trim_end().to_string(), tags)
}

/// Check if a line is a task line (starts with `- [` at some indent)
/// Returns the indent level if it is.
fn task_indent(line: &str) -> Option<usize> {
    let indent = count_indent(line);
    let content = &line[indent..];
    if content.starts_with("- [") && content.len() >= 5 && content.as_bytes().get(4) == Some(&b']')
    {
        Some(indent)
    } else {
        None
    }
}

/// Count leading spaces
fn count_indent(line: &str) -> usize {
    line.len() - line.trim_start_matches(' ').len()
}

/// Look ahead through blank lines and deeper-indent content to check if
/// there are more tasks at the given indent level. Used by parse_tasks to
/// skip gaps caused by multi-line notes with trailing blank lines.
fn has_more_tasks_at_indent(lines: &[String], start: usize, indent: usize) -> bool {
    for line in lines.iter().skip(start) {
        if line.trim().is_empty() {
            continue;
        }
        if count_indent(line) > indent {
            continue; // skip deeper-indent content (orphaned subtasks/metadata)
        }
        // Found non-blank line at or below our indent — check if it's a task
        return task_indent(line).is_some_and(|ti| ti == indent);
    }
    false
}

/// Check if a line is a metadata line at the given indent: `  - key: value`
fn is_metadata_line(line: &str, indent: usize) -> bool {
    let line_indent = count_indent(line);
    if line_indent != indent {
        return false;
    }
    let content = line[indent..].trim_start();
    if !content.starts_with("- ") {
        return false;
    }
    let after_dash = &content[2..];
    // Must have a recognized key followed by ':'
    matches!(
        after_dash.split_once(':'),
        Some((key, _)) if is_metadata_key(key)
    )
}

fn is_metadata_key(key: &str) -> bool {
    matches!(
        key.trim(),
        "dep" | "ref" | "spec" | "note" | "added" | "resolved"
    )
}

/// Parse a metadata entry starting at `idx`. Returns the metadata and next line.
fn parse_metadata(lines: &[String], idx: usize, indent: usize) -> (Metadata, usize) {
    let line = &lines[idx];
    let content = line[indent..].trim_start();
    let after_dash = &content[2..]; // skip "- "

    let (key, value_part) = after_dash.split_once(':').unwrap();
    let key = key.trim();
    let value = value_part.trim();

    match key {
        "dep" => {
            let deps: Vec<String> = value
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            (Metadata::Dep(deps), idx + 1)
        }
        "ref" => {
            let refs: Vec<String> = value
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            (Metadata::Ref(refs), idx + 1)
        }
        "spec" => (Metadata::Spec(value.to_string()), idx + 1),
        "added" => (Metadata::Added(value.to_string()), idx + 1),
        "resolved" => (Metadata::Resolved(value.to_string()), idx + 1),
        "note" => {
            if !value.is_empty() {
                // Single-line note: `- note: some text`
                (Metadata::Note(value.to_string()), idx + 1)
            } else {
                // Block note: collect indented lines
                let block_indent = indent + 2;
                let (note_text, next_idx) = parse_note_block(lines, idx + 1, block_indent);
                (Metadata::Note(note_text), next_idx)
            }
        }
        _ => {
            // Unknown metadata — treat as a note
            (Metadata::Note(format!("{}: {}", key, value)), idx + 1)
        }
    }
}

/// Parse a multiline note block, respecting code fences.
/// Lines are at `block_indent` or deeper. Returns the note text and next line.
fn parse_note_block(lines: &[String], start_idx: usize, block_indent: usize) -> (String, usize) {
    let mut note_lines = Vec::new();
    let mut idx = start_idx;
    let mut in_code_fence = false;

    while idx < lines.len() {
        let line = &lines[idx];
        let line_indent = count_indent(line);

        if in_code_fence {
            // Inside a code fence, consume everything until closing fence
            note_lines.push(strip_block_indent(line, block_indent));
            if line.trim().starts_with("```") && idx != start_idx {
                // Check that this is actually a closing fence at the block indent
                if line_indent >= block_indent
                    && line[block_indent..].trim_start().starts_with("```")
                {
                    in_code_fence = false;
                }
            }
            idx += 1;
            continue;
        }

        if line.trim().is_empty() {
            // Blank line inside note — include it
            // But check if the next non-blank line is still part of the note
            if has_continuation_at_indent(lines, idx + 1, block_indent) {
                note_lines.push(String::new());
                idx += 1;
                continue;
            } else {
                break;
            }
        }

        if line_indent < block_indent {
            // Dedented — no longer part of the note
            break;
        }

        let stripped = strip_block_indent(line, block_indent);

        // Check for code fence opening
        if stripped.trim_start().starts_with("```") {
            in_code_fence = true;
        }

        note_lines.push(stripped);
        idx += 1;
    }

    // Trim trailing empty lines
    while note_lines.last().is_some_and(|l| l.is_empty()) {
        note_lines.pop();
    }

    (note_lines.join("\n"), idx)
}

/// Strip block indent from a line, preserving relative indentation
fn strip_block_indent(line: &str, block_indent: usize) -> String {
    if line.len() >= block_indent {
        line[block_indent..].to_string()
    } else if line.trim().is_empty() {
        String::new()
    } else {
        line.trim_start().to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(s: &str) -> Vec<String> {
        s.lines().map(|l| l.to_string()).collect()
    }

    #[test]
    fn test_parse_minimal_task() {
        let input = lines("- [ ] Fix parser crash on empty blocks");
        let (tasks, _) = parse_tasks(&input, 0, 0, 0);
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].state, TaskState::Todo);
        assert_eq!(tasks[0].id, None);
        assert_eq!(tasks[0].title, "Fix parser crash on empty blocks");
        assert!(tasks[0].tags.is_empty());
    }

    #[test]
    fn test_parse_task_with_id_and_tags() {
        let input = lines("- [ ] `EFF-003` Implement effect handler desugaring #core #cc");
        let (tasks, _) = parse_tasks(&input, 0, 0, 0);
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].id.as_deref(), Some("EFF-003"));
        assert_eq!(tasks[0].title, "Implement effect handler desugaring");
        assert_eq!(tasks[0].tags, vec!["core", "cc"]);
    }

    #[test]
    fn test_parse_task_states() {
        for (ch, expected) in [
            (' ', TaskState::Todo),
            ('>', TaskState::Active),
            ('-', TaskState::Blocked),
            ('x', TaskState::Done),
            ('~', TaskState::Parked),
        ] {
            let input = lines(&format!("- [{}] Test task", ch));
            let (tasks, _) = parse_tasks(&input, 0, 0, 0);
            assert_eq!(tasks[0].state, expected);
        }
    }

    #[test]
    fn test_parse_task_with_metadata() {
        let input = lines(
            "- [>] `EFF-014` Implement effect inference #core\n\
             \x20\x20- added: 2025-05-10\n\
             \x20\x20- dep: EFF-003\n\
             \x20\x20- spec: doc/spec/effects.md#closure-effects\n\
             \x20\x20- ref: doc/design/effect-handlers-v2.md",
        );
        let (tasks, _) = parse_tasks(&input, 0, 0, 0);
        assert_eq!(tasks[0].metadata.len(), 4);
        assert!(matches!(&tasks[0].metadata[0], Metadata::Added(d) if d == "2025-05-10"));
        assert!(matches!(&tasks[0].metadata[1], Metadata::Dep(d) if d == &["EFF-003"]));
        assert!(
            matches!(&tasks[0].metadata[2], Metadata::Spec(s) if s == "doc/spec/effects.md#closure-effects")
        );
        assert!(
            matches!(&tasks[0].metadata[3], Metadata::Ref(r) if r == &["doc/design/effect-handlers-v2.md"])
        );
    }

    #[test]
    fn test_parse_subtasks() {
        let input = lines(
            "- [>] `EFF-014` Implement effect inference #core\n\
             \x20\x20- added: 2025-05-10\n\
             \x20\x20- [ ] `EFF-014.1` Add effect variables\n\
             \x20\x20- [>] `EFF-014.2` Unify effect rows #cc\n\
             \x20\x20- [ ] `EFF-014.3` Test with nested closures",
        );
        let (tasks, _) = parse_tasks(&input, 0, 0, 0);
        assert_eq!(tasks[0].subtasks.len(), 3);
        assert_eq!(tasks[0].subtasks[0].id.as_deref(), Some("EFF-014.1"));
        assert_eq!(tasks[0].subtasks[1].tags, vec!["cc"]);
        assert_eq!(tasks[0].subtasks[2].state, TaskState::Todo);
    }

    #[test]
    fn test_parse_note_block() {
        let input = lines(
            "- [ ] `EFF-014` Test task\n\
             \x20\x20- note:\n\
             \x20\x20\x20\x20Found while working on EFF-002.\n\
             \x20\x20\x20\x20\n\
             \x20\x20\x20\x20The desugaring needs to handle three cases:\n\
             \x20\x20\x20\x20 1. Simple perform\n\
             \x20\x20\x20\x20 2. Single-shot resumption",
        );
        let (tasks, _) = parse_tasks(&input, 0, 0, 0);
        assert_eq!(tasks[0].metadata.len(), 1);
        if let Metadata::Note(note) = &tasks[0].metadata[0] {
            assert!(note.contains("Found while working"));
            assert!(note.contains("three cases"));
        } else {
            panic!("Expected Note metadata");
        }
    }

    #[test]
    fn test_parse_note_with_code_fence() {
        let input = lines(
            "- [ ] `EFF-014` Test task\n\
             \x20\x20- note:\n\
             \x20\x20\x20\x20See the Koka paper:\n\
             \x20\x20\x20\x20```lace\n\
             \x20\x20\x20\x20handle(e) { ... } with {\n\
             \x20\x20\x20\x20  op(x, resume) -> resume(x + 1)\n\
             \x20\x20\x20\x20}\n\
             \x20\x20\x20\x20```",
        );
        let (tasks, _) = parse_tasks(&input, 0, 0, 0);
        if let Metadata::Note(note) = &tasks[0].metadata[0] {
            assert!(note.contains("```lace"));
            assert!(note.contains("handle(e)"));
            assert!(note.contains("```"));
        } else {
            panic!("Expected Note metadata");
        }
    }

    #[test]
    fn test_parse_multiple_deps() {
        let input = lines(
            "- [-] `EFF-012` Effect-aware DCE #core\n\
             \x20\x20- dep: EFF-014, INFRA-003",
        );
        let (tasks, _) = parse_tasks(&input, 0, 0, 0);
        if let Metadata::Dep(deps) = &tasks[0].metadata[0] {
            assert_eq!(deps, &["EFF-014", "INFRA-003"]);
        } else {
            panic!("Expected Dep metadata");
        }
    }

    #[test]
    fn test_three_level_nesting() {
        let input = lines(
            "- [>] `EFF-014` Top level\n\
             \x20\x20- [>] `EFF-014.2` Second level #cc\n\
             \x20\x20\x20\x20- [ ] `EFF-014.2.1` Third level\n\
             \x20\x20\x20\x20- [ ] `EFF-014.2.2` Third level 2",
        );
        let (tasks, _) = parse_tasks(&input, 0, 0, 0);
        assert_eq!(tasks[0].subtasks.len(), 1);
        assert_eq!(tasks[0].subtasks[0].subtasks.len(), 2);
        assert_eq!(
            tasks[0].subtasks[0].subtasks[0].id.as_deref(),
            Some("EFF-014.2.1")
        );
    }

    #[test]
    fn test_blank_lines_between_note_and_subtasks() {
        // Multi-line note with trailing blank lines before subtasks
        let input = lines(
            "- [ ] `T-001` Parent task\n\
             \x20\x20- note:\n\
             \x20\x20\x20\x20Some note content\n\
             \n\
             \n\
             \x20\x20- [ ] `T-001.1` First subtask\n\
             \x20\x20- [ ] `T-001.2` Second subtask",
        );
        let (tasks, _) = parse_tasks(&input, 0, 0, 0);
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].subtasks.len(), 2);
        assert_eq!(tasks[0].subtasks[0].id.as_deref(), Some("T-001.1"));
        assert_eq!(tasks[0].subtasks[1].id.as_deref(), Some("T-001.2"));
        if let Metadata::Note(note) = &tasks[0].metadata[0] {
            assert!(note.contains("Some note content"));
        } else {
            panic!("Expected Note metadata");
        }
    }

    #[test]
    fn test_blank_line_between_empty_note_and_metadata() {
        // Empty note (- note:\n\n) followed by more metadata
        let input = lines(
            "- [ ] `T-001` Task\n\
             \x20\x20- note:\n\
             \n\
             \x20\x20- spec: some-file.md\n\
             \x20\x20- dep: T-002",
        );
        let (tasks, _) = parse_tasks(&input, 0, 0, 0);
        assert_eq!(tasks[0].metadata.len(), 3); // note, spec, dep
        assert!(matches!(&tasks[0].metadata[0], Metadata::Note(n) if n.is_empty()));
        assert!(matches!(&tasks[0].metadata[1], Metadata::Spec(s) if s == "some-file.md"));
        assert!(matches!(&tasks[0].metadata[2], Metadata::Dep(d) if d == &["T-002"]));
    }

    #[test]
    fn test_blank_lines_between_sibling_tasks() {
        // Blank lines between two top-level tasks should not lose the second task
        let input = lines(
            "- [ ] `T-001` First task\n\
             \x20\x20- added: 2025-01-01\n\
             \n\
             - [ ] `T-002` Second task",
        );
        let (tasks, _) = parse_tasks(&input, 0, 0, 0);
        assert_eq!(tasks.len(), 2);
        assert_eq!(tasks[0].id.as_deref(), Some("T-001"));
        assert_eq!(tasks[1].id.as_deref(), Some("T-002"));
    }

    #[test]
    fn test_blank_lines_before_section_header_stops() {
        // Blank lines followed by non-task content (like a section header)
        // should still stop parsing
        let input = lines(
            "- [ ] `T-001` First task\n\
             \n\
             ## Done",
        );
        let (tasks, next_idx) = parse_tasks(&input, 0, 0, 0);
        assert_eq!(tasks.len(), 1);
        assert_eq!(next_idx, 1); // stopped at blank line, not past it
    }

    #[test]
    fn test_parse_title_and_tags_edge_cases() {
        // Title with no tags
        let (title, tags) = parse_title_and_tags("Fix parser crash");
        assert_eq!(title, "Fix parser crash");
        assert!(tags.is_empty());

        // Only tags (no title text)
        let (title, tags) = parse_title_and_tags("#core #cc");
        assert!(title.is_empty());
        assert_eq!(tags, vec!["core", "cc"]);

        // Tag-like content in the middle of the title is still title
        let (title, tags) = parse_title_and_tags("Fix #3 parser crash #bug");
        assert_eq!(title, "Fix #3 parser crash");
        assert_eq!(tags, vec!["bug"]);
    }
}
