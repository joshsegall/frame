pub mod inbox_parser;
pub mod inbox_serializer;
pub mod span;
pub mod task_parser;
pub mod task_serializer;
pub mod track_parser;
pub mod track_serializer;

/// Check if content continues at or beyond `min_indent` after blank lines.
/// Used by both the task note parser and inbox body parser to decide whether
/// a blank line is internal (separating paragraphs) or terminal (ending the block).
pub(crate) fn has_continuation_at_indent(
    lines: &[String],
    after_blank: usize,
    min_indent: usize,
) -> bool {
    for line in lines.iter().skip(after_blank) {
        if line.trim().is_empty() {
            continue;
        }
        return count_indent(line) >= min_indent;
    }
    false
}

/// Count leading spaces
pub(crate) fn count_indent(line: &str) -> usize {
    line.len() - line.trim_start_matches(' ').len()
}

/// Drop blank lines from the end of a file's serialized lines.
///
/// Only for use when the last thing in the file is an emptied container — a
/// section whose tasks are all archived, an inbox with no items left. The blank
/// line under a header lives in that header's own lines, so draining the
/// container leaves the blank stranded at end of file with nothing left to
/// separate: an extra blank row that comes back on every write. Trailing blanks
/// that follow real content are the user's formatting and must survive.
pub(crate) fn pop_trailing_blanks(lines: &mut Vec<String>) {
    while lines.last().is_some_and(|l| l.trim().is_empty()) {
        lines.pop();
    }
}

pub use inbox_parser::parse_inbox;
pub use inbox_serializer::serialize_inbox;
pub use task_parser::{parse_tasks, parse_title_and_tags};
pub use task_serializer::serialize_tasks;
pub use track_parser::parse_track;
pub use track_serializer::serialize_track;
