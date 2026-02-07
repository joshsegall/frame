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

    lines.join("\n")
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

- [>] `EFF-014` Implement effect inference for closures #ready
  - added: 2025-05-10
  - dep: EFF-003
- [ ] `EFF-015` Effect handler optimization pass #ready
  - dep: EFF-014

## Parked

- [~] `EFF-020` Higher-order effect handlers #research

## Done

- [x] `EFF-003` Implement effect handler desugaring #ready
  - resolved: 2025-05-14";

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

## Done";

        let track = parse_track(source);
        let output = serialize_track(&track);
        assert_eq!(output, source);
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

## Done";

        let track = parse_track(source);
        let output = serialize_track(&track);
        assert_eq!(output, source);
    }
}
