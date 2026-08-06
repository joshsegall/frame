//! Property tests for the markdown parse/serialize pair.
//!
//! ## Why the obvious test would prove nothing
//!
//! `serialize_task` emits a **non-dirty** task verbatim from its `source_text`
//! (see `src/parse/task_serializer.rs`). So `serialize(parse(x)) == x` holds by
//! construction for any input frame can read at all — it echoes the bytes back
//! and says nothing about whether the parser understood them. The three
//! round-trip unit tests beside the serializers are that shape; they are worth
//! keeping as format pins, but they cannot catch a comprehension bug.
//!
//! That is exactly how the defect fixed in `eff5ec0` stayed invisible. A note
//! with an unclosed code fence absorbed the rest of the file — sibling tasks,
//! the `## Parked` and `## Done` headers, every completed task — into one
//! task's `source_text`. Serializing echoed it back byte-identical, so a naive
//! round-trip passed green while `fr show` reported `task not found` for a task
//! `grep` could still see. The damage only materialized on the *next* write,
//! when the task went dirty and took the canonical path, dropping the swallowed
//! tail.
//!
//! Every property here is therefore built to defeat the verbatim path.
//!
//! ## The properties
//!
//! - **P1 — parse never panics.** Over generated markdown-ish soup and mutated
//!   fixtures. This is the class of the UTF-8 abort also fixed in `eff5ec0`:
//!   `strip_block_indent` sliced `line[4..]` inside a `§` and took the whole
//!   process down, so `fr list` panicked outright on an affected file.
//!
//! - **P2 — a canonical rewrite loses nothing.** Parse, mark every task dirty,
//!   serialize, reparse, and check that every task and every metadata entry
//!   still exists. Marking dirty bypasses `source_text` and forces the canonical
//!   path — it simulates the next write, the step that actually destroyed data.
//!   Stated as conservation rather than equality because a rewrite of malformed
//!   input may legitimately re-associate content; see `task_identities`.
//!
//! - **P3 — conservation against ground truth.** Build a `Track` *model*,
//!   serialize it, parse it back, compare to the model we started from. This is
//!   the only property here that can catch swallowing: when a task is absorbed
//!   into a note at parse time, both sides of P2 agree on the corrupted reading
//!   and pass. P3 knows the true task set because it constructed it.
//!
//! - **P4 — repeated rewrites converge.** `canon(x) = serialize(dirty(parse(x)))`
//!   must reach a fixpoint. For damaged input there is no ground truth to compare
//!   against, but a file that keeps changing every time it is written is a defect
//!   regardless. Bounded rather than one-step, because recovering malformed input
//!   normalizes it and normalization can take a second pass to settle — see
//!   `MAX_REWRITES_TO_SETTLE`.
//!
//! - **P5 — a plain write accounts for every line.** Parse, write back
//!   untouched, and require every non-blank line to return byte for byte. The
//!   other four compare *readings* of a file, so none of them can see a line the
//!   parser consumed and recorded nowhere — the reading that lost it is the only
//!   reading either side has. That blind spot is where the `fr clean` deletion
//!   lived; this property is the one that would have caught it, and does.
//!
//! ## Generator constraints
//!
//! P3's generators emit a deliberately well-behaved subset. Each exclusion below
//! is a genuine limit of what the format can represent, not a bug being papered
//! over — the wilder inputs are covered by P1/P2/P4, which need no ground truth.
//!
//! - **No flush-left lines inside a note.** `eff5ec0` made this boundary
//!   explicit: the serializer re-indents every note line to the block indent, so
//!   a less-indented line was never note content. It was lossy before and is
//!   refused now.
//! - **No trailing blank lines in a note.** `parse_note_block` trims them.
//! - **No whitespace-only note lines.** The serializer tests `is_empty()`, not
//!   `trim().is_empty()`, so `"   "` re-reads as `""`.
//! - **No `#` in titles** — it would re-parse as a tag.
//! - **No `", "` inside a `dep:`/`ref:` entry** — the serializer joins on it.
//! - **An inbox body may not open with a tag-only line.** `is_tag_only_line`
//!   claims those as tags, which is deliberate (`  #design` after a title is a
//!   tag, not body text).

use std::fs;
use std::path::PathBuf;

use frame::model::{Inbox, InboxItem, Metadata, SectionKind, Task, TaskState, Track, TrackNode};
use frame::parse::{LineEnding, parse_inbox, parse_track, serialize_inbox, serialize_track};
use proptest::prelude::*;

// ---------------------------------------------------------------------------
// Semantic projections
// ---------------------------------------------------------------------------

// `Task` already has a hand-written `PartialEq` (src/model/task.rs) that ignores
// `source_text`, `source_lines` and `dirty` — precisely the semantic comparison
// these properties want, so tasks are compared directly.
//
// `Track` and `TrackNode` have no `PartialEq` at all, and `InboxItem` derives one
// that *does* compare `source_text` and `dirty`. Both need a projection: after a
// canonical rewrite those fields legitimately differ.

/// The semantic content of a track: its title and its task sections, in order.
///
/// Literal nodes, header lines and trailing blank lines are formatting, and
/// canonical re-serialization may reflow them. What must survive is which tasks
/// are in which section, and in what order.
#[derive(Debug, PartialEq, Eq)]
struct TrackShape {
    title: String,
    sections: Vec<(SectionKind, Vec<Task>)>,
}

fn track_shape(track: &Track) -> TrackShape {
    TrackShape {
        title: track.title.clone(),
        sections: track
            .nodes
            .iter()
            .filter_map(|node| match node {
                TrackNode::Section { kind, tasks, .. } => Some((*kind, tasks.clone())),
                TrackNode::Literal(_) => None,
            })
            .collect(),
    }
}

/// Every task in the track, flattened, tagged with the section holding it.
///
/// P2 compares these as sets rather than comparing `TrackShape` outright. A
/// canonical rewrite of *malformed* input may legitimately re-associate content:
/// recovering an orphaned task normalizes its indent, and a metadata line that
/// sat at the wrong depth for the original indent becomes valid metadata for the
/// normalized one. That is the rewrite gaining fidelity, not losing it, and exact
/// equality cannot say so. What must never happen is content going missing, which
/// is what a subset check states directly.
#[derive(Debug, PartialEq, Eq, Clone)]
struct TaskIdentity {
    section: SectionKind,
    id: Option<String>,
    title: String,
}

fn task_identities(track: &Track) -> Vec<TaskIdentity> {
    fn walk(section: SectionKind, tasks: &[Task], out: &mut Vec<TaskIdentity>) {
        for task in tasks {
            out.push(TaskIdentity {
                section,
                id: task.id.as_deref().map(str::to_string),
                title: task.title.clone(),
            });
            walk(section, &task.subtasks, out);
        }
    }
    let mut out = Vec::new();
    for node in &track.nodes {
        if let TrackNode::Section { kind, tasks, .. } = node {
            walk(*kind, tasks, &mut out);
        }
    }
    out
}

/// Every metadata entry in the track, flattened. Compared as a set for the same
/// reason as `task_identities`.
fn metadata_entries(track: &Track) -> Vec<Metadata> {
    fn walk(tasks: &[Task], out: &mut Vec<Metadata>) {
        for task in tasks {
            out.extend(task.metadata.iter().cloned());
            walk(&task.subtasks, out);
        }
    }
    let mut out = Vec::new();
    for node in &track.nodes {
        if let TrackNode::Section { tasks, .. } = node {
            walk(tasks, &mut out);
        }
    }
    out
}

/// Every stranded line the parser attached to a task, flattened. Compared as a
/// set for the same reason as `task_identities`.
///
/// Where the line ends up attached is not fixed: a rewrite that normalizes an
/// orphaned task's indent can hand its stranded neighbour to a different task,
/// and one that recovers a metadata key can stop the line being stranded at all.
/// Either is fine. Vanishing is not.
fn stranded_lines(track: &Track) -> Vec<String> {
    fn walk(tasks: &[Task], out: &mut Vec<String>) {
        for task in tasks {
            out.extend(task.leading_lines.iter().cloned());
            walk(&task.subtasks, out);
        }
    }
    let mut out = Vec::new();
    for node in &track.nodes {
        if let TrackNode::Section { tasks, .. } = node {
            walk(tasks, &mut out);
        }
    }
    out
}

/// The semantic content of an inbox item, minus source tracking.
#[derive(Debug, PartialEq, Eq)]
struct ItemShape {
    title: String,
    tags: Vec<String>,
    body: Option<String>,
}

fn inbox_shape(inbox: &Inbox) -> Vec<ItemShape> {
    inbox
        .items
        .iter()
        .map(|item| ItemShape {
            title: item.title.clone(),
            tags: item.tags.clone(),
            body: item.body.clone(),
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Forcing the canonical path
// ---------------------------------------------------------------------------

fn dirty_task(task: &mut Task) {
    task.dirty = true;
    task.source_text = None;
    for sub in &mut task.subtasks {
        dirty_task(sub);
    }
}

fn dirty_track(track: &mut Track) {
    for node in &mut track.nodes {
        if let TrackNode::Section { tasks, .. } = node {
            for task in tasks.iter_mut() {
                dirty_task(task);
            }
        }
    }
}

fn dirty_inbox(inbox: &mut Inbox) {
    for item in &mut inbox.items {
        item.dirty = true;
        item.source_text = None;
    }
}

/// `canon(x)` — the text a full rewrite of `x` would produce. This is the shape
/// the verbatim `source_text` path never exercises.
fn canon_track(source: &str) -> String {
    let mut track = parse_track(source);
    dirty_track(&mut track);
    serialize_track(&track)
}

fn canon_inbox(source: &str) -> String {
    let (mut inbox, _) = parse_inbox(source);
    dirty_inbox(&mut inbox);
    serialize_inbox(&inbox)
}

/// How many rewrites P4 allows before the text must stop changing.
///
/// One is not enough, and the reason is structural rather than a defect. A
/// rewrite of malformed input *normalizes*: an orphaned task recovered at the
/// section's indent is re-emitted at the canonical indent, and only on the next
/// parse does a metadata line below it sit at the right depth to be adopted. That
/// is two passes to settle, by construction.
///
/// What must never happen is oscillation or unbounded drift — a file that keeps
/// changing every time it is written. Convergence within a small bound states
/// that directly, and is the property users actually depend on.
const MAX_REWRITES_TO_SETTLE: usize = 4;

/// Rewrite repeatedly until the text stops changing. Returns the settled text and
/// how many rewrites it took, or `Err` with the last two forms if it never
/// settled within [`MAX_REWRITES_TO_SETTLE`].
fn settle(source: &str, rewrite: fn(&str) -> String) -> Result<(String, usize), (String, String)> {
    let mut current = rewrite(source);
    for pass in 1..=MAX_REWRITES_TO_SETTLE {
        let next = rewrite(&current);
        if next == current {
            return Ok((current, pass));
        }
        current = next;
    }
    Err((current.clone(), rewrite(&current)))
}

// ---------------------------------------------------------------------------
// Corpus: the checked-in fixtures, as mutation seeds
// ---------------------------------------------------------------------------

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

/// Every fixture, plus a CRLF copy of each.
///
/// The files on disk are LF and should stay that way — they are read by the
/// round-trip tests too, and a CRLF file in git is its own kind of trouble. The
/// copies are made here instead, which doubles the corpus for the properties
/// that mutate it without putting a `\r` in the repository.
fn fixture_sources() -> Vec<String> {
    let mut out = Vec::new();
    for entry in fs::read_dir(fixture_dir()).expect("fixtures dir readable") {
        let path = entry.expect("readable dir entry").path();
        if path.extension().is_some_and(|e| e == "md") {
            out.push(fs::read_to_string(&path).expect("fixture readable"));
        }
    }
    assert!(!out.is_empty(), "no .md fixtures found");
    let crlf: Vec<String> = out.iter().map(|s| s.replace('\n', "\r\n")).collect();
    out.extend(crlf);
    out
}

// ---------------------------------------------------------------------------
// Generators: markdown-ish soup (P1, P4)
// ---------------------------------------------------------------------------

/// Lines chosen to collide with every structural decision the parser makes:
/// checkbox forms, section headers, fence openers and closers (with and without
/// an info string), metadata keys, indentation steps, and multi-byte characters
/// at offsets the old byte-slicing code would have split.
const INTERESTING_LINES: &[&str] = &[
    "",
    " ",
    "   ",
    "\t",
    "# Title",
    "## Backlog",
    "## Parked",
    "## Done",
    "> a description",
    "- [ ] plain task",
    "- [x] `T-001` done task",
    "- [>] `T-a1` active #tag",
    "- [~] parked",
    "- [-] blocked",
    "- [ ] `T-002` café ünïcode §§§ title",
    "  - added: 2025-05-14",
    "  - resolved: 2025-05-14",
    "  - dep: T-001, T-002",
    "  - ref: src/a.rs",
    "  - spec: doc/s.md#x",
    "  - note: single line",
    "  - note:",
    // Whole note blocks, as single multi-line entries. Drawing the `- note:` key
    // and its indented body independently would usually strand the body as
    // orphaned deep content, which is a *different* known defect (see
    // `DEEP_CONTENT_LINES`). Keeping them together covers fenced, multi-byte and
    // extra-indented note bodies in the shape they actually occur.
    "  - note:\n    ```rust\n    let x = 1;\n    ```",
    "  - note:\n    ```lace\n    let x = perform Ask()",
    "  - note:\n    § multi-byte at the slice boundary\n    more note text",
    "  - note:\n      deeper body\n        deeper still",
    "  - note:\n    body text",
    "- [ ] `T-003` trailing",
    "not a task line at all",
    "```",
    "§",
];

/// Indented content that is not metadata and not attached to a `- note:` key,
/// plus orphaned task lines. Drawn independently, these land in exactly the
/// malformed positions the recovery paths exist for: content deeper than the
/// metadata indent, and subtasks with no parent.
const DEEP_CONTENT_LINES: &[&str] = &[
    "    ```",
    "    ```rust",
    "    ```lace",
    "    § multi-byte at the slice boundary",
    "    body text",
    "      deeper body",
    "        deeper still",
    "  - [ ] `T-003.1` subtask",
    "    - [ ] `T-003.1.1` deep subtask",
    "      - [ ] `T-003.1.1.1` past MAX_DEPTH",
    // Indent 1, but byte 4 falls *inside* the second '§' (each is 2 bytes:
    // [0]=' ', [1..3]='§', [3..5]='§'). A note block indent of 4 slicing on byte
    // length rather than indent splits the character and aborts the process.
    " §§ dedented mid-character",
];

fn arb_soup() -> impl Strategy<Value = String> {
    let pool: Vec<&'static str> = INTERESTING_LINES
        .iter()
        .chain(DEEP_CONTENT_LINES)
        .copied()
        .collect();
    // Both endings. Every generator here used to `join("\n")`, so no case in
    // the whole suite carried a `\r` and the CRLF-to-LF rewrite was invisible
    // to all five properties at once.
    (
        prop::collection::vec(prop::sample::select(pool).prop_map(str::to_string), 0..40),
        prop::bool::ANY,
    )
        .prop_map(|(lines, crlf)| lines.join(if crlf { "\r\n" } else { "\n" }))
}

/// A mutation applied to one line of a real fixture.
#[derive(Debug, Clone)]
enum Mutation {
    Delete,
    Duplicate,
    Truncate,
    Indent(usize),
    Dedent,
    Replace(usize),
    AppendMultibyte,
}

fn arb_mutation() -> impl Strategy<Value = Mutation> {
    prop_oneof![
        Just(Mutation::Delete),
        Just(Mutation::Duplicate),
        Just(Mutation::Truncate),
        (1usize..8).prop_map(Mutation::Indent),
        Just(Mutation::Dedent),
        (0usize..INTERESTING_LINES.len()).prop_map(Mutation::Replace),
        Just(Mutation::AppendMultibyte),
    ]
}

/// Apply `mutation` at `idx % len`. Truncation and the multi-byte append are the
/// ones aimed squarely at the old `line[block_indent..]` slice: both can leave a
/// byte index pointing into the middle of a character.
fn mutate(source: &str, idx: usize, mutation: &Mutation) -> String {
    // `split` rather than `lines`, so a CRLF source stays CRLF through the
    // mutation: the `\r` rides along at the end of each line's content.
    let mut lines: Vec<String> = source.split('\n').map(str::to_string).collect();
    if lines.is_empty() {
        return source.to_string();
    }
    let i = idx % lines.len();
    match mutation {
        Mutation::Delete => {
            lines.remove(i);
        }
        Mutation::Duplicate => {
            lines.insert(i, lines[i].clone());
        }
        Mutation::Truncate => {
            // Cut at a char boundary chosen from the middle of the line, so the
            // *content* is ragged even though the string stays valid UTF-8.
            let line = &lines[i];
            let cut = line
                .char_indices()
                .nth(line.chars().count() / 2)
                .map(|(b, _)| b)
                .unwrap_or(0);
            lines[i] = line[..cut].to_string();
        }
        Mutation::Indent(n) => {
            lines[i] = format!("{}{}", " ".repeat(*n), lines[i]);
        }
        Mutation::Dedent => {
            lines[i] = lines[i].trim_start().to_string();
        }
        Mutation::Replace(n) => {
            lines[i] = INTERESTING_LINES[*n].to_string();
        }
        Mutation::AppendMultibyte => {
            lines[i] = format!("{}§é→", lines[i]);
        }
    }
    lines.join("\n")
}

// ---------------------------------------------------------------------------
// Generators: well-formed models (P3)
// ---------------------------------------------------------------------------

fn arb_title() -> impl Strategy<Value = String> {
    // No '#' (re-parses as a tag), no backtick (re-parses as an ID delimiter),
    // no leading/trailing space (trimmed on read). Multi-byte content is very
    // much wanted here.
    prop::sample::select(
        [
            "plain title",
            "café ünïcode",
            "§ symbol lead",
            "with - dashes",
            "with: a colon",
            "trailing punctuation!",
            "a",
        ]
        .as_slice(),
    )
    .prop_map(str::to_string)
}

fn arb_tags() -> impl Strategy<Value = Vec<String>> {
    prop::collection::vec(
        prop::sample::select(["cc", "bug", "design", "cc-added"].as_slice())
            .prop_map(str::to_string),
        0..3,
    )
}

fn arb_note() -> impl Strategy<Value = String> {
    // Lines are non-blank and never flush-left-ambiguous; no trailing blank line.
    prop::collection::vec(
        prop::sample::select(
            [
                "a note line",
                "```rust",
                "let x = 1;",
                "```",
                "§ unicode in a note",
                "- not a task, just prose",
            ]
            .as_slice(),
        )
        .prop_map(str::to_string),
        1..5,
    )
    .prop_map(|lines| lines.join("\n"))
}

fn arb_metadata() -> impl Strategy<Value = Vec<Metadata>> {
    // At most one of each key, in the serializer's own emission order, so the
    // comparison is about content rather than ordering policy.
    (
        prop::option::of(Just("2025-05-14".to_string())),
        prop::option::of(Just("2025-06-01".to_string())),
        prop::option::of(prop::collection::vec(
            prop::sample::select(["T-001", "T-002", "EFF-a3"].as_slice()).prop_map(str::to_string),
            1..3,
        )),
        prop::option::of(prop::collection::vec(
            prop::sample::select(["src/a.rs", "doc/b.md"].as_slice()).prop_map(str::to_string),
            1..3,
        )),
        prop::option::of(Just("doc/spec.md#section".to_string())),
        prop::option::of(arb_note()),
    )
        .prop_map(|(added, resolved, dep, refs, spec, note)| {
            let mut out = Vec::new();
            if let Some(v) = added {
                out.push(Metadata::Added(v));
            }
            if let Some(v) = resolved {
                out.push(Metadata::Resolved(v));
            }
            if let Some(v) = dep {
                out.push(Metadata::Dep(v));
            }
            if let Some(v) = refs {
                out.push(Metadata::Ref(v));
            }
            if let Some(v) = spec {
                out.push(Metadata::Spec(v));
            }
            // Note last: a multiline note runs to the end of the task's own
            // block, so anything emitted after it would be swallowed. That is
            // the format, not a defect.
            if let Some(v) = note {
                out.push(Metadata::Note(v));
            }
            out
        })
}

/// A task with no subtasks, at the given nesting depth.
fn arb_leaf(depth: usize) -> impl Strategy<Value = Task> {
    (
        prop::sample::select(
            [
                TaskState::Todo,
                TaskState::Active,
                TaskState::Blocked,
                TaskState::Done,
                TaskState::Parked,
            ]
            .as_slice(),
        ),
        prop::option::of(prop::sample::select(
            ["T-001", "T-002", "EFF-a14", "T-001.1"].as_slice(),
        )),
        arb_title(),
        arb_tags(),
        arb_metadata(),
    )
        .prop_map(move |(state, id, title, tags, metadata)| {
            let mut task = Task::new(state, id.map(Into::into), title);
            task.tags = tags;
            task.metadata = metadata;
            task.depth = depth;
            task
        })
}

/// Tasks nested up to the format's 3-level limit, with `depth` set to match the
/// nesting (`Task`'s `PartialEq` compares it).
fn arb_task_tree() -> impl Strategy<Value = Task> {
    (
        arb_leaf(0),
        prop::collection::vec(
            (arb_leaf(1), prop::collection::vec(arb_leaf(2), 0..2)),
            0..3,
        ),
    )
        .prop_map(|(mut root, children)| {
            root.subtasks = children
                .into_iter()
                .map(|(mut child, grandchildren)| {
                    child.subtasks = grandchildren;
                    child
                })
                .collect();
            root
        })
}

fn section(kind: SectionKind, tasks: Vec<Task>, last: bool) -> TrackNode {
    let header = match kind {
        SectionKind::Backlog => "## Backlog",
        SectionKind::Parked => "## Parked",
        SectionKind::Done => "## Done",
    };
    TrackNode::Section {
        kind,
        header_lines: vec![header.to_string(), String::new()],
        tasks,
        trailing_lines: if last {
            Vec::new()
        } else {
            vec![String::new()]
        },
    }
}

/// Both line endings, so P3's ground-truth comparison runs against each. A
/// model that says CRLF must serialize to a CRLF file and read back as the same
/// model — which is the whole claim `LineEnding` makes.
fn arb_eol() -> impl Strategy<Value = LineEnding> {
    prop_oneof![Just(LineEnding::Lf), Just(LineEnding::Crlf)]
}

fn arb_track_model() -> impl Strategy<Value = Track> {
    (
        prop::collection::vec(arb_task_tree(), 0..3),
        prop::collection::vec(arb_task_tree(), 0..2),
        prop::collection::vec(arb_task_tree(), 0..2),
        arb_eol(),
    )
        .prop_map(|(backlog, parked, done, eol)| Track {
            title: "Generated Track".to_string(),
            description: None,
            nodes: vec![
                TrackNode::Literal(vec!["# Generated Track".to_string(), String::new()]),
                section(SectionKind::Backlog, backlog, false),
                section(SectionKind::Parked, parked, false),
                section(SectionKind::Done, done, true),
            ],
            source_lines: Vec::new(),
            eol,
        })
}

fn arb_inbox_model() -> impl Strategy<Value = Inbox> {
    (
        prop::collection::vec(
            (
                arb_title(),
                arb_tags(),
                prop::option::of(prop::collection::vec(
                    // First line must not be tag-only; `is_tag_only_line` would
                    // claim it as tags. Every candidate here starts with prose.
                    prop::sample::select(
                        [
                            "body line",
                            "more detail here",
                            "§ unicode body",
                            "```lace",
                            "let x = 1",
                            "```",
                        ]
                        .as_slice(),
                    )
                    .prop_map(str::to_string),
                    1..4,
                )),
            ),
            0..4,
        ),
        arb_eol(),
    )
        .prop_map(|(items, eol)| Inbox {
            header_lines: vec!["# Inbox".to_string(), String::new()],
            items: items
                .into_iter()
                .map(|(title, tags, body)| {
                    let mut item = InboxItem::new(title);
                    item.tags = tags;
                    item.body = body.map(|lines| lines.join("\n"));
                    item
                })
                .collect(),
            source_lines: Vec::new(),
            eol,
        })
}

// ---------------------------------------------------------------------------
// P1 — parse never panics
// ---------------------------------------------------------------------------

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// The UTF-8 abort fixed in `eff5ec0` took the whole process down, so this
    /// property is about liveness, not correctness: whatever the input, parsing
    /// and serializing must return.
    #[test]
    fn p1_track_parse_never_panics(source in arb_soup()) {
        let track = parse_track(&source);
        let _ = serialize_track(&track);
    }

    #[test]
    fn p1_inbox_parse_never_panics(source in arb_soup()) {
        let (inbox, _) = parse_inbox(&source);
        let _ = serialize_inbox(&inbox);
    }

    /// Same, but starting from real fixtures and damaging them one edit at a
    /// time — closer to the shapes a half-finished hand edit or a bad merge
    /// actually produces.
    #[test]
    fn p1_mutated_fixtures_never_panic(
        which in 0usize..64,
        idx in 0usize..512,
        mutation in arb_mutation(),
    ) {
        let corpus = fixture_sources();
        let source = &corpus[which % corpus.len()];
        let damaged = mutate(source, idx, &mutation);

        let track = parse_track(&damaged);
        let _ = serialize_track(&track);
        let (inbox, _) = parse_inbox(&damaged);
        let _ = serialize_inbox(&inbox);
    }
}

// ---------------------------------------------------------------------------
// P2 — canonical re-serialization preserves what was parsed
// ---------------------------------------------------------------------------

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    /// Parse, force every task through the canonical path, reparse. Whatever the
    /// first parse understood must survive being written back out. This is the
    /// step that destroyed data in `eff5ec0`.
    ///
    /// Stated as conservation rather than equality — see `task_identities` for
    /// why a rewrite of malformed input may legitimately re-associate content.
    #[test]
    fn p2_track_canonical_rewrite_preserves_content(
        which in 0usize..64,
        idx in 0usize..512,
        mutation in arb_mutation(),
    ) {
        let corpus = fixture_sources();
        let source = &corpus[which % corpus.len()];
        let damaged = mutate(source, idx, &mutation);

        let before = parse_track(&damaged);
        let rewritten = canon_track(&damaged);
        let after = parse_track(&rewritten);

        let after_tasks = task_identities(&after);
        for task in task_identities(&before) {
            prop_assert!(
                after_tasks.contains(&task),
                "task lost by the rewrite: {:?}\nsurvivors: {:?}",
                task,
                after_tasks
            );
        }

        let after_meta = metadata_entries(&after);
        for meta in metadata_entries(&before) {
            prop_assert!(
                after_meta.contains(&meta),
                "metadata lost by the rewrite: {:?}\nsurvivors: {:?}",
                meta,
                after_meta
            );
        }

        // Lines frame could not attribute to any task. Checked against the
        // rewritten *text* rather than against `stranded_lines(&after)`,
        // because a rewrite may legitimately stop a line being stranded — the
        // normalized indent can make it readable as metadata. Being understood
        // is not the same as being lost.
        for stranded in stranded_lines(&before) {
            prop_assert!(
                rewritten.lines().any(|l| l.trim() == stranded.trim()),
                "stranded line lost by the rewrite: {:?}\nrewritten: {:?}",
                stranded,
                rewritten
            );
        }
    }

    #[test]
    fn p2_inbox_canonical_rewrite_preserves_content(
        which in 0usize..64,
        idx in 0usize..512,
        mutation in arb_mutation(),
    ) {
        let corpus = fixture_sources();
        let source = &corpus[which % corpus.len()];
        let damaged = mutate(source, idx, &mutation);

        let (before, _) = parse_inbox(&damaged);
        let rewritten = canon_inbox(&damaged);
        let (after, _) = parse_inbox(&rewritten);

        prop_assert_eq!(inbox_shape(&before), inbox_shape(&after));
    }
}

// ---------------------------------------------------------------------------
// P3 — conservation against ground truth
// ---------------------------------------------------------------------------

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    /// The only property here that can catch swallowing. P2 compares two
    /// readings of the same text, so a task absorbed into a note at parse time
    /// makes both sides agree and pass. Here the model is the ground truth:
    /// every task, its section, its nesting and its metadata must come back.
    #[test]
    fn p3_track_model_survives_a_round_trip(model in arb_track_model()) {
        let text = serialize_track(&model);
        let parsed = parse_track(&text);

        prop_assert_eq!(track_shape(&model), track_shape(&parsed));
    }

    /// Task count is implied by the shape comparison above, but asserting it
    /// separately makes a swallowing regression report as "8 tasks became 3"
    /// rather than as a large structural diff.
    #[test]
    fn p3_no_task_is_lost(model in arb_track_model()) {
        fn count(tasks: &[Task]) -> usize {
            tasks.iter().map(|t| 1 + count(&t.subtasks)).sum()
        }
        let total = |t: &Track| -> usize {
            t.nodes
                .iter()
                .filter_map(|n| match n {
                    TrackNode::Section { tasks, .. } => Some(count(tasks)),
                    TrackNode::Literal(_) => None,
                })
                .sum()
        };

        let text = serialize_track(&model);
        let parsed = parse_track(&text);

        prop_assert_eq!(total(&model), total(&parsed));
    }

    #[test]
    fn p3_inbox_model_survives_a_round_trip(model in arb_inbox_model()) {
        let text = serialize_inbox(&model);
        let (parsed, _) = parse_inbox(&text);

        prop_assert_eq!(inbox_shape(&model), inbox_shape(&parsed));
    }
}

// ---------------------------------------------------------------------------
// P4 — repeated rewrites converge
// ---------------------------------------------------------------------------

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// For damaged input there is no ground truth to compare against, but
    /// convergence is still checkable: repeated writes must stop changing the
    /// file. A parser that reads its own output differently shows up here even
    /// when nothing else can judge the result.
    ///
    /// Bounded rather than one-step — see `MAX_REWRITES_TO_SETTLE`. Compared
    /// byte for byte, trailing newlines included: they used to be excluded
    /// because the round trip ate one per write, which is now fixed.
    #[test]
    fn p4_track_rewrites_converge(source in arb_soup()) {
        if let Err((last, next)) = settle(&source, canon_track) {
            return Err(TestCaseError::fail(format!(
                "never settled in {MAX_REWRITES_TO_SETTLE} rewrites\nlast: {last:?}\nnext: {next:?}"
            )));
        }
    }

    #[test]
    fn p4_inbox_rewrites_converge(source in arb_soup()) {
        if let Err((last, next)) = settle(&source, canon_inbox) {
            return Err(TestCaseError::fail(format!(
                "never settled in {MAX_REWRITES_TO_SETTLE} rewrites\nlast: {last:?}\nnext: {next:?}"
            )));
        }
    }

    #[test]
    fn p4_mutated_fixture_rewrites_converge(
        which in 0usize..64,
        idx in 0usize..512,
        mutation in arb_mutation(),
    ) {
        let corpus = fixture_sources();
        let source = &corpus[which % corpus.len()];
        let damaged = mutate(source, idx, &mutation);

        if let Err((last, next)) = settle(&damaged, canon_track) {
            return Err(TestCaseError::fail(format!(
                "never settled in {MAX_REWRITES_TO_SETTLE} rewrites\nlast: {last:?}\nnext: {next:?}"
            )));
        }
    }
}

// ---------------------------------------------------------------------------
// P5 — a plain write accounts for every line
// ---------------------------------------------------------------------------

/// The non-blank lines of a file, in order. Blank lines are excluded because the
/// parser genuinely normalizes them: a blank between two tasks is formatting, it
/// belongs to no node, and losing one is cosmetic. A non-blank line is content.
///
/// Line *terminators* are not content and are not compared here — P6 owns them.
/// Keeping them out is what lets this property ignore the terminal newline
/// frame deliberately adds (`f1a4ff5`), which is a difference in the file's
/// ending rather than in its lines.
fn nonblank_lines(source: &str) -> Vec<&str> {
    source.lines().filter(|l| !l.trim().is_empty()).collect()
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    /// Parse and write back without touching anything: every non-blank line must
    /// come back, byte for byte, in the same order.
    ///
    /// P1–P4 all compare *readings* of a file against each other, or against a
    /// well-formed model. None of them can see a line the parser consumed and
    /// recorded nowhere: both sides of P2 agree it does not exist, P3's
    /// generators never produce one, and P4 converges happily because the line
    /// is gone after the first write and stays gone. That is the blind spot
    /// `parse_tasks` sat in — it advanced past unrecognized indented content
    /// with `idx += 1`, so the line was absent from the model, invisible to
    /// `fr check`, and deleted by the next write of the file.
    ///
    /// This property closes it from the other side: not "did the model survive
    /// the write" but "did the file". It needs no ground truth and no
    /// interpretation, which is why it can catch content frame does not
    /// understand — the only content at risk of being dropped for not being
    /// understood.
    #[test]
    fn p5_a_plain_write_keeps_every_line(source in arb_soup()) {
        let written = serialize_track(&parse_track(&source));
        prop_assert_eq!(nonblank_lines(&source), nonblank_lines(&written));
    }

    /// Same property against damaged real fixtures, which is where the shapes
    /// that trip it actually come from: a half-finished hand edit, a merge that
    /// left a line at the wrong indent, a metadata key that lost its colon.
    #[test]
    fn p5_a_plain_write_keeps_every_line_in_a_damaged_fixture(
        which in 0usize..64,
        idx in 0usize..512,
        mutation in arb_mutation(),
    ) {
        let corpus = fixture_sources();
        let source = &corpus[which % corpus.len()];
        let damaged = mutate(source, idx, &mutation);

        let written = serialize_track(&parse_track(&damaged));
        prop_assert_eq!(nonblank_lines(&damaged), nonblank_lines(&written));
    }
}

// ---------------------------------------------------------------------------
// P6 — a write keeps the file's line ending
// ---------------------------------------------------------------------------

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// A CRLF file must come back CRLF.
    ///
    /// Every parser reads with `str::lines`, which strips `\r` along with `\n`,
    /// and every serializer joined with `"\n"` — so the carriage returns had
    /// nowhere to live and the first write rewrote every line in the file. No
    /// content was lost, which is why P1-P5 all pass on it: P2 and P3 compare
    /// readings that agree, P5 compares lines whose terminators it strips, and
    /// P4 converges happily because the file is stable *after* the one
    /// destructive rewrite.
    ///
    /// It matters because of what else writes these files. With `core.autocrlf`
    /// or a `text=auto` attribute, git re-applies CRLF on checkout and frame
    /// strips it on the next write — the two churn against each other forever
    /// with neither able to win, which is `1df7a69` from a third direction. And
    /// a whole-file diff is where a one-line deletion hides, which is how
    /// `3447fb6` went unnoticed for as long as it did.
    #[test]
    fn p6_a_write_keeps_the_line_ending(source in arb_soup()) {
        let written = serialize_track(&parse_track(&source));
        prop_assert_eq!(
            LineEnding::detect(&source),
            LineEnding::detect(&written),
            "source: {:?}\nwritten: {:?}", source, written
        );
    }

    #[test]
    fn p6_an_inbox_write_keeps_the_line_ending(source in arb_soup()) {
        let written = serialize_inbox(&parse_inbox(&source).0);
        prop_assert_eq!(
            LineEnding::detect(&source),
            LineEnding::detect(&written),
            "source: {:?}\nwritten: {:?}", source, written
        );
    }

    /// And it survives repeated writes — a fixpoint, not a one-step accident.
    /// The churn this prevents is precisely a file that never settles.
    #[test]
    fn p6_the_line_ending_is_a_fixpoint(source in arb_soup()) {
        let once = serialize_track(&parse_track(&source));
        let twice = serialize_track(&parse_track(&once));
        prop_assert_eq!(once, twice);
    }
}

// ---------------------------------------------------------------------------
// Regression pins — the concrete shapes behind the properties above
// ---------------------------------------------------------------------------

/// The exact `eff5ec0` failure, as a fixed case: an unclosed fence in a note
/// must not swallow the sibling task, the later section headers, or the done
/// task. Kept alongside the properties so a shrink failure has something
/// human-readable to sit next to.
#[test]
fn unclosed_fence_does_not_swallow_the_rest_of_the_file() {
    let source = "\
# Track

## Backlog

- [ ] `T-001` Has an unclosed fence
  - note:
    ```rust
    let x = 1;
- [ ] `T-002` Sibling that must survive

## Done

- [x] `T-003` Done task that must survive";

    let track = parse_track(source);

    assert_eq!(track.backlog().len(), 2, "sibling task was swallowed");
    assert_eq!(track.done().len(), 1, "done task was swallowed");
    assert_eq!(track.backlog()[1].id.as_deref(), Some("T-002"));
    assert_eq!(track.done()[0].id.as_deref(), Some("T-003"));
}

/// Regression: an orphaned task line must survive a write.
///
/// An indented task line with no parent above it used to be consumed by
/// `parse_tasks` without being recorded anywhere — absent from the model, so the
/// next write deleted it from the file, and `fr check` could not see it either
/// because the task was already gone by the time check ran. It is reachable from
/// a hand edit that removes a parent, from a three-way merge that keeps a subtask
/// whose parent went away, and from nesting one level past `MAX_DEPTH`.
///
/// It is now parsed at the level it was found in: read at its real indent, but
/// recorded at the enclosing depth, so a rewrite re-emits it somewhere that
/// parses back the same way. Over-deep nesting is flattened rather than dropped.
///
/// Every shape below deleted a task before the fix.
#[test]
fn orphaned_task_lines_survive_a_write() {
    let cases: &[(&str, &str, &str)] = &[
        (
            "orphan directly after the section header",
            "# Track\n\n## Backlog\n\n  - [ ] `T-001` Orphan",
            "T-001",
        ),
        (
            "orphan with no blank line after the header",
            "# Track\n\n## Backlog\n  - [ ] `T-001` Orphan",
            "T-001",
        ),
        (
            "orphan followed by another section",
            "# Track\n\n## Backlog\n\n  - [ ] `T-001` Orphan\n\n## Done\n\n- [x] `T-002` Done",
            "T-001",
        ),
        (
            "nesting one level past MAX_DEPTH, at the end",
            "# Track\n\n## Backlog\n\n- [ ] `A` a\n  - [ ] `B` b\n    - [ ] `C` c\n      - [ ] `D` d",
            "`D`",
        ),
        (
            "nesting past MAX_DEPTH with a task following at top level",
            "# Track\n\n## Backlog\n\n- [ ] `A` a\n  - [ ] `B` b\n    - [ ] `C` c\n      - [ ] `D` d\n- [ ] `E` e",
            "`D`",
        ),
    ];

    for (label, source, needle) in cases {
        let clean = serialize_track(&parse_track(source));
        assert!(
            clean.contains(needle),
            "{label}: {needle} deleted by a verbatim write: {clean:?}"
        );

        let once = canon_track(source);
        assert!(
            once.contains(needle),
            "{label}: {needle} deleted by a canonical rewrite: {once:?}"
        );

        let twice = canon_track(&once);
        assert_eq!(once, twice, "{label}: canonical form is not stable");
    }
}

/// The last of those shapes must also keep the *other* tasks — the earlier
/// candidate fix (stop parsing and let the line fall through as literal text)
/// preserved the orphan but truncated the section, dropping `E`.
#[test]
fn recovering_an_orphan_does_not_truncate_the_section() {
    let source = "\
# Track

## Backlog

- [ ] `A` a
  - [ ] `B` b
    - [ ] `C` c
      - [ ] `D` d
- [ ] `E` e";

    let rewritten = canon_track(source);
    for id in ["`A`", "`B`", "`C`", "`D`", "`E`"] {
        assert!(rewritten.contains(id), "{id} lost: {rewritten:?}");
    }
}

/// Regression: content indented deeper than the metadata indent, which is not
/// recognized metadata and not attached to a `- note:` key, must survive a write.
///
/// `parse_single_task`'s metadata loop used to advance past such a line with
/// `idx += 1; continue`, and `own_end_idx` then included it — so it survived a
/// verbatim write but disappeared the moment the task went dirty, because the
/// canonical path rebuilds the task from its fields and the line is in none of
/// them. The loop now stops and hands the line back to `parse_tasks`, which keeps
/// it as a task or as literal text on the track.
///
/// Pre-existing and independent of the orphaned-task recovery — confirmed
/// identical before and after that change. It surfaced only because orphaned task
/// lines are no longer deleted first.
#[test]
fn deep_content_after_metadata_survives_a_write() {
    let source = "\
# Track

## Backlog

- [ ] `T-001` Task
  - added: 2025-05-14
    stray deep content";

    let rewritten = canon_track(source);

    assert!(
        rewritten.contains("stray deep content"),
        "deep content deleted by a canonical rewrite: {rewritten:?}"
    );
}

/// Regression: a note that reduces to a single line with leading or trailing
/// whitespace must keep it.
///
/// `parse_metadata` trims the value after the key, so `- note:   x` re-reads as
/// `Note("x")` — the single-line form cannot carry the whitespace, and the
/// indentation was lost on the *second* write. The serializer now routes any
/// untrimmed note to the indented block form, which `strip_block_indent` reverses
/// exactly.
#[test]
fn single_line_note_keeps_surrounding_whitespace() {
    let source = "\
# Track

## Backlog

- [ ] `T-001` Task
  - note:
      indented one-liner";

    let once = canon_track(source);
    let twice = canon_track(&once);
    assert_eq!(once, twice, "note indentation changed on the second write");

    let parsed = parse_track(&once);
    let Some(Metadata::Note(note)) = parsed.backlog()[0].metadata.first() else {
        panic!("expected a note, got {:?}", parsed.backlog()[0].metadata);
    };
    assert_eq!(
        note, "  indented one-liner",
        "leading whitespace was dropped"
    );
}

/// Multi-byte content at and around the note boundary, including a dedented line
/// whose byte `block_indent` falls *inside* a character.
///
/// This is the shape of the UTF-8 abort in `eff5ec0` but it no longer pins that
/// panic, and the distinction is worth recording. Two guards were added there:
/// the indent bound in `parse_note_block` and the indent-based guard in
/// `strip_block_indent`. The first makes the second unreachable — a dedented line
/// breaks out of the note before any slicing happens — so `strip_block_indent`'s
/// non-slicing branches are now dead in production, reached only by its own unit
/// tests. Reverting the slice guard alone does not reproduce the crash; it shows
/// up as note-indentation corruption caught by P4 instead.
#[test]
fn multibyte_at_the_indent_boundary_does_not_panic() {
    let source = "\
# Track

## Backlog

- [ ] `T-001` Task
  - note:
    §ünïcode at the slice boundary
 §§ dedented mid-character
- [ ] `T-002` Sibling
";

    let track = parse_track(source);
    let _ = serialize_track(&track);
    assert_eq!(track.backlog().len(), 2);
}

/// Regression: the `fr clean` incident. A mis-indented prose line below a
/// `resolved:` key, in a done task nobody was working on, deleted by a write of
/// the track — no diff to read it in, because the same run was filling in
/// `resolved:` dates on other tasks and the file was already churning.
///
/// The line satisfied every condition of the drop at once: deeper than the
/// metadata indent (so `parse_single_task` stopped before it), not a task line
/// (so `parse_tasks` would not adopt it), and followed by another task at the
/// same level (so `parse_tasks` consumed it rather than leaving it to the track
/// parser as literal text). `parse_tasks` advanced past it with `idx += 1` and
/// recorded it nowhere, which is why nothing warned: the line was not in the
/// model for `fr check` to inspect, and the next write simply did not emit it.
///
/// Asserted on the plain write as well as the canonical one. The plain path is
/// the one that actually ran — `fr clean` rewrites a whole track file to fill in
/// one task's date, and every clean task on it goes out verbatim — and it was
/// losing the line too.
#[test]
fn a_stray_line_between_two_done_tasks_survives_a_write() {
    let source = "\
# Main

## Done

- [x] `MAI-012` Sharded map lowering
  - added: 2026-07-01
  - resolved: 2026-07-20
    **Shape.** A sharded `||> Vec.map` whose callback produces a per-row output.
- [x] `MAI-013` Unrelated finished work
  - resolved: 2026-07-21
";

    let plain = serialize_track(&parse_track(source));
    assert_eq!(plain, source, "a plain write changed the file at all");

    let rewritten = canon_track(source);
    assert!(
        rewritten.contains("**Shape.**"),
        "stray line deleted by a canonical rewrite: {rewritten:?}"
    );
    assert_eq!(
        rewritten,
        canon_track(&rewritten),
        "the recovered line did not settle"
    );
}

/// The same drop one level down, and with no `- note:` key anywhere near it:
/// content stranded under a subtask, with a top-level task following. It was
/// consumed by the outer `parse_tasks`, not the inner one, so a fix that only
/// looked at the metadata loop would have left it.
#[test]
fn a_stray_line_under_a_subtask_survives_a_write() {
    let source = "\
# Track

## Backlog

- [ ] `T-001` Parent
  - [ ] `T-001.1` Subtask
      stranded under the subtask
- [ ] `T-002` Sibling
";

    assert_eq!(serialize_track(&parse_track(source)), source);
    assert!(canon_track(source).contains("stranded under the subtask"));
}

/// A write ends the file with exactly one newline, and a second write does not
/// change it.
///
/// Frame used to end files with no terminal newline at all: `serialize_track`
/// finishes with `lines.join("\n")` while `parse_track` reads with
/// `str::lines()`, and `"a\n".lines()` is `["a"]`, so the byte had nowhere to
/// live in the model. Every write dropped one. A file ending in N blank lines
/// took N-1 writes to settle, and — the part that actually bit — an editor save
/// adding the customary final newline was undone by the next frame write, so the
/// two churned against each other indefinitely. This is the `1df7a69` shape:
/// frame and another writer disagreeing about a file forever.
/// The concrete `LineEnding` claim, as fixed cases beside the property.
#[test]
fn a_crlf_track_comes_back_crlf() {
    let source = "# Main\r\n\r\n## Backlog\r\n\r\n\
                  - [ ] `M-001` One\r\n  - added: 2025-05-01\r\n\r\n## Done\r\n";
    let written = serialize_track(&parse_track(source));
    assert_eq!(
        written, source,
        "a CRLF file must survive a write unchanged"
    );
}

#[test]
fn a_crlf_inbox_comes_back_crlf() {
    let source = "# Inbox\r\n\r\n- captured thing #tag\r\n";
    let written = serialize_inbox(&parse_inbox(source).0);
    assert_eq!(written, source);
}

/// An LF file must not acquire carriage returns because one line had one.
#[test]
fn a_mostly_lf_file_stays_lf() {
    let source = "# Main\n\n## Backlog\r\n\n- [ ] `M-001` One\n\n## Done\n";
    let written = serialize_track(&parse_track(source));
    assert!(
        !written.contains('\r'),
        "the majority ending wins, and it is LF here: {written:?}"
    );
}

/// A CRLF file that frame *edits* keeps its ending too — the canonical path
/// rebuilds a task from its fields, so it is a different code path from the
/// verbatim one the cases above take.
#[test]
fn a_dirtied_crlf_track_still_writes_crlf() {
    let source = "# Main\r\n\r\n## Backlog\r\n\r\n- [ ] `M-001` One\r\n\r\n## Done\r\n";
    let mut track = parse_track(source);
    dirty_track(&mut track);
    let written = serialize_track(&track);
    assert_eq!(
        LineEnding::detect(&written),
        LineEnding::Crlf,
        "{written:?}"
    );
    assert!(
        !written.contains("\n\n\r"),
        "no stray bare newlines: {written:?}"
    );
}

#[test]
fn a_write_leaves_exactly_one_terminal_newline() {
    for source in [
        "# T\n\n## Backlog\n\n- [ ] `T-001` One",
        "# T\n\n## Backlog\n\n- [ ] `T-001` One\n",
        "# T\n\n## Backlog\n\n- [ ] `T-001` One\n\n\n",
    ] {
        let once = canon_track(source);
        let twice = canon_track(&once);
        assert_eq!(
            once, twice,
            "a second write must change nothing: {source:?}"
        );
        assert!(once.ends_with('\n'), "{once:?}");
    }

    // Erosion, specifically: trailing blank lines the user wrote are kept, not
    // shaved off one per write. Preserving them is correct — they are the user's
    // formatting — and it is the *losing* of them that was the defect.
    let padded = "# T\n\n## Backlog\n\n- [ ] `T-001` One\n\n\n";
    assert_eq!(
        canon_track(padded).matches('\n').count(),
        padded.matches('\n').count(),
        "trailing blank lines must survive a write intact"
    );
}

/// The same for the inbox, which has its own serializer and could drift.
#[test]
fn an_inbox_write_leaves_exactly_one_terminal_newline() {
    for source in [
        "# Inbox\n\n- one",
        "# Inbox\n\n- one\n",
        "# Inbox\n\n- one\n\n",
    ] {
        let once = canon_inbox(source);
        let twice = canon_inbox(&once);
        assert_eq!(
            once, twice,
            "a second write must change nothing: {source:?}"
        );
        assert!(once.ends_with('\n'), "{once:?}");
    }
}

/// The same drop with nothing before it: a stranded line at the top of a
/// section, above the first task. Shrunk out of P5 by proptest, and the case
/// that decided where stranded lines get attached — there is no preceding task
/// to hang this one on, but there is always a following one, which is why
/// `leading_lines` sits on the successor.
#[test]
fn a_stray_line_above_the_first_task_survives_a_write() {
    let source = "\
## Backlog
  - added: 2025-05-14
- [ ] plain task
";

    assert_eq!(serialize_track(&parse_track(source)), source);
    assert!(canon_track(source).contains("- added: 2025-05-14"));
}

/// Stranded content has two anchors — `trailing_lines` on the task above for a
/// line indented past that task's metadata, `leading_lines` on the task below
/// for anything else — and both are reachable from one run of stranded lines.
/// The blank separating the run from the task below it must die at both, or the
/// file changes on two successive writes: the first emitted the blank, the
/// second re-read the run under the *other* anchor and dropped it.
///
/// Shrunk out of P6 by proptest, twice: fixing the first shape (blank directly
/// above the task) surfaced the second (blank between two held lines that
/// different anchors claim). Nothing was ever lost here — this is the
/// convergence claim, not conservation.
#[test]
fn a_stranded_run_settles_in_one_write() {
    for source in [
        "## Backlog\n- [ ] plain task\n\n    ```\n\n- [ ] plain task\n",
        "## Backlog\n- [ ] plain task\n\n    ```\n\n  - added: 2025-05-14\n- [ ] plain task\n",
    ] {
        let once = serialize_track(&parse_track(source));
        assert_eq!(
            once,
            serialize_track(&parse_track(&once)),
            "a second write changed the file again: {source:?}"
        );
        assert!(
            once.contains("```"),
            "the stranded line itself must survive: {once:?}"
        );
    }
}
