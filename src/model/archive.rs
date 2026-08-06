use crate::model::task::Task;
use crate::parse::LineEnding;

/// A done-task archive: `frame/archive/<track>.md`.
///
/// **An archive is not a track.** `fr clean` writes a `# Archive — <track>`
/// heading and then bare task lines, with no `## Section` headers at all, so
/// walking `TrackNode::Section` finds nothing in one and `parse_track` reads the
/// whole task list as literal text. That mistake has been made twice in this
/// codebase — in `ops/fix.rs`, where the repair reported success and changed
/// nothing, and in `ops/track_ops.rs`, where a prefix rename silently skipped
/// every archived id — which is why the format now has a parse/serialize pair of
/// its own instead of four hand-rolled readers.
///
/// `frame/archive/_tracks/<track>.md` is a *different* file: a whole track that
/// `fr track archive` moved there intact, sections and all. It is a [`Track`],
/// parses with `parse_track`, and must not come near this type.
///
/// [`Track`]: crate::model::track::Track
#[derive(Debug, Clone)]
pub struct Archive {
    /// Everything above the first task line, verbatim: the heading, the blank
    /// under it, and anything a person wrote there. Frame does not understand
    /// this and does not try to — it is carried through a rewrite unchanged.
    pub header: Vec<String>,
    /// The archived tasks, as a flat list.
    pub tasks: Vec<Task>,
    /// Lines after the last task that the task parser would not claim — a note
    /// someone left at the bottom, a rule, an HTML comment.
    ///
    /// Every reader of this format used to discard the index `parse_tasks`
    /// stopped at, so a rewrite was header-plus-tasks and anything down here was
    /// in neither piece. It is carried for the same reason the header is.
    pub trailing: Vec<String>,
    /// The line ending the file used, re-applied when it is written back.
    /// See [`crate::parse::LineEnding`].
    pub eol: LineEnding,
}

impl Archive {
    /// An empty archive with the standard heading for `track_id`, as `fr clean`
    /// writes it for a track that has none yet.
    pub fn new(track_id: &str) -> Self {
        Archive {
            header: vec![format!("# Archive — {}", track_id), String::new()],
            tasks: Vec::new(),
            trailing: Vec::new(),
            eol: LineEnding::default(),
        }
    }
}
