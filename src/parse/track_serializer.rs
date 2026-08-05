use crate::model::track::{Track, TrackNode};
use crate::parse::task_serializer::serialize_tasks;

/// Serialize a track back to its markdown representation.
/// Literal nodes are emitted verbatim. Task sections use the task serializer
/// (which respects the dirty flag for round-trip preservation).
pub fn serialize_track(track: &Track) -> String {
    let mut lines = Vec::new();

    for node in &track.nodes {
        match node {
            TrackNode::Literal(literal_lines) => {
                lines.extend(literal_lines.iter().cloned());
            }
            TrackNode::Section {
                header_lines,
                tasks,
                trailing_lines,
                ..
            } => {
                lines.extend(header_lines.iter().cloned());
                let task_lines = serialize_tasks(tasks, 0);
                lines.extend(task_lines);
                lines.extend(trailing_lines.iter().cloned());
            }
        }
    }

    // A section drained of its tasks — `fr clean` archiving the last Done task,
    // say — contributes only its header and the blank that separated it from
    // those tasks. At end of file that blank separates nothing, and leaving it
    // means every clean re-adds a blank row someone then has to strip again.
    if matches!(track.nodes.last(), Some(TrackNode::Section { tasks, .. }) if tasks.is_empty()) {
        crate::parse::pop_trailing_blanks(&mut lines);
    }

    let mut out = lines.join("\n");
    out.push('\n');
    track.eol.apply(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::track_parser::parse_track;

    #[test]
    fn test_round_trip_simple_track() {
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
";

        let track = parse_track(source);
        let output = serialize_track(&track);
        assert_eq!(output, source);
    }

    #[test]
    fn test_round_trip_empty_sections() {
        let source = "\
# Empty Track

## Backlog

## Parked

## Done
";

        let track = parse_track(source);
        let output = serialize_track(&track);
        assert_eq!(output, source);
    }

    #[test]
    fn test_emptied_last_section_leaves_no_blank_row() {
        // The blank line under `## Done` belongs to that section's header_lines,
        // so draining the section (what `fr clean` does when it archives) used
        // to strand it at end of file.
        let source = "\
# Test Track

## Backlog

- [ ] `T-100` Keep me

## Done

- [x] `T-001` Archived away
";

        let mut track = parse_track(source);
        for node in &mut track.nodes {
            if let TrackNode::Section {
                kind: crate::model::track::SectionKind::Done,
                tasks,
                ..
            } = node
            {
                tasks.clear();
            }
        }

        let output = serialize_track(&track);
        assert!(
            output.ends_with("## Done\n"),
            "expected no trailing blank row, got {:?}",
            output
        );
        // Idempotent: a second pass must not change it again.
        assert_eq!(serialize_track(&parse_track(&output)), output);
    }

    #[test]
    fn test_blanks_after_a_trailing_empty_section_are_dropped() {
        let track = parse_track("# T\n\n## Backlog\n\n## Done\n\n\n");
        assert_eq!(serialize_track(&track), "# T\n\n## Backlog\n\n## Done\n");
    }

    #[test]
    fn test_blanks_after_a_trailing_task_survive() {
        // Only an *empty* trailing section gets trimmed — blanks the user wrote
        // after real content are their formatting.
        let source = "# T\n\n## Done\n\n- [x] `T-001` Done thing\n\n\n";
        assert_eq!(serialize_track(&parse_track(source)), source);
    }

    #[test]
    fn test_round_trip_with_subtasks() {
        let source = "\
# Test Track

## Backlog

- [>] `T-001` Parent task
  - added: 2025-05-10
  - [ ] `T-001.1` First subtask
  - [>] `T-001.2` Second subtask #cc
    - [ ] `T-001.2.1` Deep subtask
    - [ ] `T-001.2.2` Another deep subtask
  - [ ] `T-001.3` Third subtask

## Done
";

        let track = parse_track(source);
        let output = serialize_track(&track);
        assert_eq!(output, source);
    }
}
