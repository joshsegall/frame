use crate::model::archive::Archive;
use crate::parse::LineEnding;
use crate::parse::task_parser::{parse_tasks, task_indent};

/// Parse a done-task archive: a header, a flat task list, and whatever follows.
///
/// Where the task list starts is the whole subtlety. Four readers used to answer
/// it with `line.starts_with("- [")`, which is not the same question the task
/// parser asks — an ordinary markdown link bullet like `- [context](notes.md)`
/// matches it. A header containing one made the *entire archive* read as empty,
/// because `parse_tasks` starting on a line that is not a task line stops
/// immediately. That is not a cosmetic misread: `ops/ids.rs` scans archives for
/// the highest minted id, and a working copy with no local frontier store — a
/// fresh clone, where `.ids.toml` has not been written yet — has nothing else to
/// stop it handing an archived task's number straight back out.
///
/// So the test is [`task_indent`], the parser's own, and it is applied at *any*
/// indent rather than only at column 0. Column 0 is where `fr clean` writes
/// them, but a file that starts its list indented should be visible and get
/// flattened by `parse_tasks` rather than silently read as having no tasks at
/// all. `tui/input/recent.rs` already did it this way; the four CLI readers did
/// not, so the TUI listed archived tasks the CLI could not find.
pub fn parse_archive(source: &str) -> Archive {
    let lines: Vec<String> = source.lines().map(|l| l.to_string()).collect();

    let start = lines
        .iter()
        .position(|l| task_indent(l).is_some())
        .unwrap_or(lines.len());
    let (tasks, next) = parse_tasks(&lines, start, 0, 0);

    Archive {
        header: lines[..start].to_vec(),
        tasks,
        trailing: lines[next.min(lines.len())..].to_vec(),
        eol: LineEnding::detect(source),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::serialize_archive;

    /// The shape `fr clean` actually writes.
    const CLEAN_WROTE: &str = "\
# Archive — main

- [x] `MAI-002` second task
  - added: 2026-08-05
  - resolved: 2026-08-05
- [x] `MAI-001` first task
  - resolved: 2026-08-05
";

    #[test]
    fn reads_the_shape_clean_writes() {
        let archive = parse_archive(CLEAN_WROTE);
        assert_eq!(
            archive.header,
            vec!["# Archive — main".to_string(), "".into()]
        );
        assert_eq!(archive.tasks.len(), 2);
        assert_eq!(archive.tasks[0].title, "second task");
        assert!(archive.trailing.is_empty());
        assert_eq!(serialize_archive(&archive), CLEAN_WROTE);
    }

    #[test]
    fn an_empty_or_headerless_file_is_not_a_panic() {
        assert!(parse_archive("").tasks.is_empty());
        assert!(parse_archive("# Archive — main\n").tasks.is_empty());
        assert_eq!(parse_archive("- [x] `M-1` bare\n").tasks.len(), 1);
    }

    /// A markdown link in the header is not a task line. Reading it as one used
    /// to hide every task below it.
    #[test]
    fn a_link_bullet_in_the_header_stays_in_the_header() {
        let source = "\
# Archive — main
- [context](notes.md)

- [x] `MAI-001` real task
";
        let archive = parse_archive(source);
        assert_eq!(archive.tasks.len(), 1, "the task must be visible");
        assert!(
            archive.header.iter().any(|l| l.contains("notes.md")),
            "and the link stays where it was: {:?}",
            archive.header
        );
        assert_eq!(serialize_archive(&archive), source);
    }

    /// Content after the last task belongs to the file, not to the void.
    #[test]
    fn trailing_content_is_kept() {
        let source = "\
# Archive — main

- [x] `MAI-001` task

<!-- archived 2025 sprint notes -->
";
        let archive = parse_archive(source);
        assert_eq!(archive.tasks.len(), 1);
        assert!(
            archive.trailing.iter().any(|l| l.contains("sprint notes")),
            "trailing content: {:?}",
            archive.trailing
        );
        assert_eq!(serialize_archive(&archive), source);
    }

    /// An indented first task is visible, where a column-0-only scan found
    /// nothing and reported the archive as empty.
    #[test]
    fn an_indented_first_task_is_still_found() {
        let archive = parse_archive("# Archive — main\n\n  - [x] `MAI-001` indented\n");
        assert_eq!(archive.tasks.len(), 1);
        assert_eq!(archive.tasks[0].title, "indented");
    }

    #[test]
    fn the_line_ending_is_detected() {
        assert_eq!(
            parse_archive("# Archive — main\r\n\r\n- [x] `M-1` t\r\n").eol,
            LineEnding::Crlf
        );
        assert_eq!(parse_archive(CLEAN_WROTE).eol, LineEnding::Lf);
    }
}
