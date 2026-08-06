use crate::model::archive::Archive;
use crate::parse::task_serializer::serialize_tasks;

/// Serialize a done-task archive back to markdown.
///
/// Header and trailing lines are emitted verbatim — frame does not know what
/// they mean, and the tasks in between go through the task serializer, which
/// emits a clean task from its `source_text`. So rewriting an archive nothing
/// has touched gives back the same bytes.
///
/// The two things this adds that no previous archive writer had: the file's own
/// line ending, and exactly one terminal newline. Before, every writer joined
/// with `"\n"` regardless, and `fr clean`'s append did not even do that
/// consistently — it kept the existing text raw and appended an LF block, so
/// appending to a CRLF archive produced a file with both. Terminal newlines
/// disagreed the same way: `clean` wrote none and the other writers added one,
/// so which one you got depended on which command touched the file last.
///
/// **It deliberately does not mirror the track serializer's separator rule.**
/// That one puts a blank between a section header and its first task, and can
/// tell a bare `## Done` from a header sitting above stranded content because
/// stranded content is anchored on the *task*, in `leading_lines`. An archive
/// has no such split: `header` is *defined* as everything above the first task
/// line, so a hand-written note directly above the tasks is indistinguishable
/// from a heading with no blank under it, and a separator rule here would edit
/// the note. `Archive::new` writes the blank, so an archive frame created never
/// welds; one trimmed by hand keeps exactly what it says.
pub fn serialize_archive(archive: &Archive) -> String {
    let mut lines = archive.header.clone();
    lines.extend(serialize_tasks(&archive.tasks, 0));
    lines.extend(archive.trailing.iter().cloned());

    // An archive with nothing in it at all stays an empty file rather than
    // becoming a lone newline.
    if lines.is_empty() {
        return String::new();
    }

    let mut out = lines.join("\n");
    out.push('\n');
    archive.eol.apply(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::{LineEnding, parse_archive};

    #[test]
    fn a_crlf_archive_comes_back_crlf() {
        let source = "# Archive — main\r\n\r\n- [x] `MAI-001` task\r\n";
        let archive = parse_archive(source);
        assert_eq!(archive.eol, LineEnding::Crlf);
        assert_eq!(serialize_archive(&archive), source);
    }

    /// The terminal newline is the writer's, as it is for tracks: a file without
    /// one gains it on the first write and does not grow another after that.
    #[test]
    fn a_write_leaves_exactly_one_terminal_newline() {
        let once = serialize_archive(&parse_archive("# Archive — m\n\n- [x] `M-1` t"));
        assert!(once.ends_with("- [x] `M-1` t\n"), "{once:?}");
        assert_eq!(serialize_archive(&parse_archive(&once)), once);
    }

    #[test]
    fn an_empty_archive_serializes_to_nothing() {
        assert_eq!(serialize_archive(&parse_archive("")), "");
    }

    /// The archive keeps its header exactly as it found it, including a
    /// heading with no blank under it — see the note on `serialize_archive`.
    /// This is the case the track serializer *does* separate, recorded here so
    /// the asymmetry is deliberate rather than an oversight someone later
    /// "fixes" into rewriting hand-written headers.
    #[test]
    fn an_archive_heading_is_never_separated_from_its_tasks() {
        let mut archive = parse_archive("# Archive \u{2014} main\n");
        assert!(archive.tasks.is_empty());
        archive.tasks = parse_archive("# A\n\n- [x] `M-1` t\n").tasks;
        assert_eq!(
            serialize_archive(&archive),
            "# Archive \u{2014} main\n- [x] `M-1` t\n"
        );

        let stranded = "# Archive \u{2014} main\n  - resolved: 2025-05-14\n- [x] `M-1` t\n";
        assert_eq!(serialize_archive(&parse_archive(stranded)), stranded);
    }

    /// A dirty task takes the canonical path; everything around it is still
    /// carried verbatim.
    #[test]
    fn a_dirty_task_does_not_disturb_the_header_or_the_tail() {
        let source = "\
# Archive — main
> hand-written note in the header

- [x] `MAI-001` task
  - resolved: 2026-08-05

<!-- end -->
";
        let mut archive = parse_archive(source);
        archive.tasks[0].dirty = true;
        archive.tasks[0].title = "renamed".to_string();

        let out = serialize_archive(&archive);
        assert!(
            out.starts_with("# Archive — main\n> hand-written note"),
            "{out:?}"
        );
        assert!(out.contains("`MAI-001` renamed"), "{out:?}");
        assert!(out.ends_with("<!-- end -->\n"), "{out:?}");
    }
}
