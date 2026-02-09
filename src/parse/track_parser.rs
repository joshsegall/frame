use crate::model::track::{SectionKind, Track, TrackNode};
use crate::parse::task_parser::parse_tasks;

/// Parse a track file from its source text
pub fn parse_track(source: &str) -> Track {
    let lines: Vec<String> = source.lines().map(|l| l.to_string()).collect();
    let mut nodes: Vec<TrackNode> = Vec::new();
    let mut title = String::new();
    let mut description = None;

    let mut idx = 0;
    let mut literal_buf: Vec<String> = Vec::new();

    while idx < lines.len() {
        let line = &lines[idx];
        let trimmed = line.trim();

        // Check for track title: `# Title`
        if trimmed.starts_with("# ") && !trimmed.starts_with("## ") {
            // Flush literal buffer
            flush_literal(&mut literal_buf, &mut nodes);
            title = trimmed[2..].to_string();
            literal_buf.push(lines[idx].clone());
            idx += 1;
            continue;
        }

        // Check for description: `> text`
        if let Some(desc) = trimmed.strip_prefix("> ") {
            flush_literal(&mut literal_buf, &mut nodes);
            description = Some(desc.to_string());
            literal_buf.push(lines[idx].clone());
            idx += 1;
            continue;
        }

        // Check for section header: `## Backlog`, `## Parked`, `## Done`
        if let Some(after_hashes) = trimmed.strip_prefix("## ") {
            flush_literal(&mut literal_buf, &mut nodes);

            let section_name = after_hashes.trim();
            let kind = match section_name.to_lowercase().as_str() {
                "backlog" => Some(SectionKind::Backlog),
                "parked" => Some(SectionKind::Parked),
                "done" => Some(SectionKind::Done),
                _ => None,
            };

            if let Some(kind) = kind {
                let header_line = lines[idx].clone();
                idx += 1;

                // Collect blank lines between header and first task
                let mut header_lines = vec![header_line];
                while idx < lines.len() && lines[idx].trim().is_empty() {
                    header_lines.push(lines[idx].clone());
                    idx += 1;
                }

                // Parse tasks in this section
                let (tasks, next_idx) = parse_tasks(&lines, idx, 0, 0);
                idx = next_idx;

                // Collect trailing blank lines
                let mut trailing_lines = Vec::new();
                while idx < lines.len() && lines[idx].trim().is_empty() {
                    trailing_lines.push(lines[idx].clone());
                    idx += 1;
                }

                nodes.push(TrackNode::Section {
                    kind,
                    header_lines,
                    tasks,
                    trailing_lines,
                });
            } else {
                // Unknown section header — treat as literal
                literal_buf.push(lines[idx].clone());
                idx += 1;
            }
            continue;
        }

        // Everything else is literal text
        literal_buf.push(lines[idx].clone());
        idx += 1;
    }

    // Flush remaining literal buffer
    flush_literal(&mut literal_buf, &mut nodes);

    Track {
        title,
        description,
        nodes,
        source_lines: lines,
    }
}

fn flush_literal(buf: &mut Vec<String>, nodes: &mut Vec<TrackNode>) {
    if !buf.is_empty() {
        nodes.push(TrackNode::Literal(std::mem::take(buf)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::task::TaskState;

    #[test]
    fn test_parse_track_structure() {
        let source = "\
# Effect System

> Design and implement the algebraic effect system for Lace.

## Backlog

- [>] `EFF-014` Implement effect inference for closures #core
  - added: 2025-05-10
  - dep: EFF-003
- [ ] `EFF-015` Effect handler optimization pass #core
  - dep: EFF-014

## Parked

- [~] `EFF-020` Higher-order effect handlers #research

## Done

- [x] `EFF-003` Implement effect handler desugaring #core
  - resolved: 2025-05-14
- [x] `EFF-002` Parse effect declarations #core
  - resolved: 2025-05-12
";

        let track = parse_track(source);
        assert_eq!(track.title, "Effect System");
        assert_eq!(
            track.description.as_deref(),
            Some("Design and implement the algebraic effect system for Lace.")
        );

        // Check sections
        let backlog = track.backlog();
        assert_eq!(backlog.len(), 2);
        assert_eq!(backlog[0].id.as_deref(), Some("EFF-014"));
        assert_eq!(backlog[0].state, TaskState::Active);
        assert_eq!(backlog[1].id.as_deref(), Some("EFF-015"));

        let parked = track.parked();
        assert_eq!(parked.len(), 1);
        assert_eq!(parked[0].state, TaskState::Parked);

        let done = track.done();
        assert_eq!(done.len(), 2);
        assert_eq!(done[0].state, TaskState::Done);
    }

    #[test]
    fn test_parse_track_empty_sections() {
        let source = "\
# Empty Track

## Backlog

## Parked

## Done
";
        let track = parse_track(source);
        assert_eq!(track.title, "Empty Track");
        assert!(track.backlog().is_empty());
        assert!(track.parked().is_empty());
        assert!(track.done().is_empty());
    }

    #[test]
    fn test_parse_track_with_subtasks() {
        let source = "\
# Test Track

## Backlog

- [>] `T-001` Parent task
  - [ ] `T-001.1` First subtask
  - [ ] `T-001.2` Second subtask
    - [ ] `T-001.2.1` Deep subtask
";
        let track = parse_track(source);
        let backlog = track.backlog();
        assert_eq!(backlog.len(), 1);
        assert_eq!(backlog[0].subtasks.len(), 2);
        assert_eq!(backlog[0].subtasks[1].subtasks.len(), 1);
    }

    #[test]
    fn test_parse_track_preserves_node_order() {
        let source = "\
# My Track

> A description.

## Backlog

- [ ] `T-001` A task

## Parked

## Done
";
        let track = parse_track(source);
        // We should have: Literal (title+desc), Section(Backlog), Section(Parked), Section(Done)
        let mut section_count = 0;
        let mut literal_count = 0;
        for node in &track.nodes {
            match node {
                TrackNode::Section { .. } => section_count += 1,
                TrackNode::Literal(_) => literal_count += 1,
            }
        }
        assert_eq!(section_count, 3);
        assert!(literal_count >= 1); // At least the title/desc block
    }
}
