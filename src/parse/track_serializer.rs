use crate::model::track::{Track, TrackNode};
use crate::parse::task_serializer::serialize_tasks;

/// Serialize a track back to its markdown representation.
/// Literal nodes are emitted verbatim. Task sections use the task serializer
/// (which respects the dirty flag for round-trip preservation).
pub fn serialize_track(track: &Track) -> String {
    let mut lines = Vec::new();

    let last_node = track.nodes.len().saturating_sub(1);
    for (i, node) in track.nodes.iter().enumerate() {
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
                // Deliberately *not* the mirror of the rule below. A section
                // that ended the file has no blank under its header, and the
                // first task moved into it comes out welded to `## Done`, which
                // does look wrong. But a header can be followed immediately by
                // stranded content the parser preserves verbatim
                // (`a_stray_line_above_the_first_task_survives_a_write`), and a
                // serializer that inserts a blank there rewrites a file nobody
                // edited. The weld is cosmetic; that guarantee is not.
                let task_lines = serialize_tasks(tasks, 0);
                lines.extend(task_lines);
                lines.extend(trailing_lines.iter().cloned());

                // One blank line between a section and whatever follows it —
                // no more, no fewer.
                //
                // This was the `ops` layer's job, added to the section move
                // because moving a task into an empty `## Done` welded it to
                // the next header. But a section move is one of a dozen things
                // that puts a task into a section: undo has its own inserts,
                // and every one of them that reached an emptied section
                // produced the same welded file. Rather than teach each caller
                // to remember, the rule lives where nothing can bypass it.
                //
                // Both halves matter. *Too few* is the weld. *Too many* is the
                // drained section — its header blank and its trailing blank are
                // one separator counted twice, so a file grew a line every time
                // a section emptied, and undoing the delete did not close the
                // gap because the task came back after the doubled blank.
                if i != last_node {
                    while lines.len() >= 2
                        && lines[lines.len() - 1].trim().is_empty()
                        && lines[lines.len() - 2].trim().is_empty()
                    {
                        lines.pop();
                    }
                    let welded = lines
                        .last()
                        .is_some_and(|l| !l.trim().is_empty() && !tasks.is_empty());
                    if welded {
                        lines.push(String::new());
                    }
                }
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
    use crate::model::SectionKind;
    use crate::parse::track_parser::parse_track;

    /// A section drained of its last task keeps *one* blank before the next
    /// header, not the two it inherits — its own, and the one under its header.
    ///
    /// Without this a file grew a line every time a section emptied, and the
    /// growth was invisible: the result round-trips through the parser
    /// unchanged, so no settledness check would report it.
    #[test]
    fn a_drained_section_does_not_double_its_separator() {
        let source = "\
# T

## Backlog

- [ ] `T-001` One

## Done

- [x] `T-000` Done
";
        let mut track = parse_track(source);
        track
            .section_tasks_mut(SectionKind::Backlog)
            .unwrap()
            .clear();
        assert_eq!(
            serialize_track(&track),
            "# T\n\n## Backlog\n\n## Done\n\n- [x] `T-000` Done\n"
        );
    }

    /// And a section that gains a task into that emptied space gets the blank
    /// back, rather than welding its last task to the next header.
    ///
    /// This is the half that used to live in `ops::task_ops` and fire only on a
    /// section move. Undo's own inserts went straight to the model and came out
    /// welded.
    #[test]
    fn a_refilled_section_separates_itself_from_the_next_header() {
        let source = "\
# T

## Backlog

## Done

- [x] `T-000` Done
";
        let mut track = parse_track(source);
        let task = parse_track("# X\n\n## Backlog\n\n- [ ] `T-001` One\n")
            .section_tasks(SectionKind::Backlog)[0]
            .clone();
        track
            .section_tasks_mut(SectionKind::Backlog)
            .unwrap()
            .push(task);
        assert_eq!(
            serialize_track(&track),
            "# T\n\n## Backlog\n\n- [ ] `T-001` One\n\n## Done\n\n- [x] `T-000` Done\n"
        );
    }

    /// The last section gets neither: a separator before end-of-file separates
    /// nothing, and every task marked done used to append one.
    #[test]
    fn the_last_section_gains_no_trailing_blank() {
        let source = "\
# T

## Backlog

## Done

- [x] `T-000` Done
";
        let mut track = parse_track(source);
        let task = parse_track("# X\n\n## Backlog\n\n- [x] `T-001` Two\n")
            .section_tasks(SectionKind::Backlog)[0]
            .clone();
        track
            .section_tasks_mut(SectionKind::Done)
            .unwrap()
            .push(task);
        assert!(
            serialize_track(&track).ends_with("- [x] `T-001` Two\n"),
            "no blank line at end of file"
        );
    }

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
