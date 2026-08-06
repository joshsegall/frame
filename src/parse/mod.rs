pub mod archive_parser;
pub mod archive_serializer;
pub mod inbox_parser;
pub mod inbox_serializer;
pub mod span;
pub mod task_parser;
pub mod task_serializer;
pub mod track_parser;
pub mod track_serializer;

/// How a file separates its lines.
///
/// Every parser here reads with [`str::lines`], which strips `\r` along with
/// `\n`, and every serializer joins with `"\n"`. So a carriage return has
/// nowhere to live in the model, and a CRLF file used to come back LF — every
/// line of it rewritten by the first write frame made.
///
/// That is the `f1a4ff5` shape exactly: the terminal newline had nowhere to
/// live either, and the fix was to make it a property of the writer rather than
/// something the model has to carry per line. Same move here, one flag per
/// file. It matters because with `core.autocrlf` or a `text=auto` attribute git
/// re-applies CRLF on checkout and frame strips it on the next write, so the
/// two churn against each other forever with neither able to win — `1df7a69`
/// from a third direction.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum LineEnding {
    #[default]
    Lf,
    Crlf,
}

impl LineEnding {
    /// The ending a source uses, by majority.
    ///
    /// Only one flag is carried per file, so a file that mixes endings is
    /// normalized to whichever it uses more — the alternative is storing the
    /// `\r` per line, which puts it inside titles and tags where nothing wants
    /// it. Mixed files are malformed anyway; settling them on their own
    /// majority is the least surprising answer. A tie, which includes a file
    /// with no line breaks at all, reads as `Lf`.
    pub fn detect(source: &str) -> Self {
        let crlf = source.matches("\r\n").count();
        let total = source.matches('\n').count();
        if crlf * 2 > total {
            LineEnding::Crlf
        } else {
            LineEnding::Lf
        }
    }

    /// Apply this ending to text built with `\n`.
    ///
    /// One conversion at the end of serialization rather than a join everywhere:
    /// a serializer that assembles lines in six places can forget one, and every
    /// `\n` in a frame file — including inside a note body or a fenced block —
    /// is a line break that should carry the file's ending.
    pub fn apply(self, text: String) -> String {
        match self {
            LineEnding::Lf => text,
            LineEnding::Crlf => text.replace('\n', "\r\n"),
        }
    }
}

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

pub use archive_parser::parse_archive;
pub use archive_serializer::serialize_archive;
pub use inbox_parser::parse_inbox;
pub use inbox_serializer::serialize_inbox;
pub use task_parser::{parse_tasks, parse_title_and_tags};
pub use task_serializer::serialize_tasks;
pub use track_parser::parse_track;
pub use track_serializer::serialize_track;
