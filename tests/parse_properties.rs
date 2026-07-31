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
//! - **P2 — canonical re-serialization preserves what was parsed.** Parse, mark
//!   every task dirty, serialize, reparse, compare. Marking dirty bypasses
//!   `source_text` and forces the canonical path — it simulates the next write,
//!   the step that actually destroyed data.
//!
//! - **P3 — conservation against ground truth.** Build a `Track` *model*,
//!   serialize it, parse it back, compare to the model we started from. This is
//!   the only property here that can catch swallowing: when a task is absorbed
//!   into a note at parse time, both sides of P2 agree on the corrupted reading
//!   and pass. P3 knows the true task set because it constructed it.
//!
//! - **P4 — canonical form is a fixpoint.** `canon(canon(x)) == canon(x)` where
//!   `canon(x) = serialize(dirty(parse(x)))`. For damaged input there is no
//!   ground truth to compare against, but one normalization pass may not be
//!   followed by a second that changes anything.
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
use frame::parse::{parse_inbox, parse_track, serialize_inbox, serialize_track};
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

/// Trailing newlines are excluded from the P4 fixpoint comparison because the
/// round trip is **known to be unstable** in exactly that byte, and the churn
/// would mask every structural instability the property is here to find.
///
/// The cause: `serialize_track` ends with `lines.join("\n")` while `parse_track`
/// reads with `str::lines()`, and `"a\n".lines()` is `["a"]` — a terminal newline
/// has no representation in the model. Each round trip therefore drops one, so a
/// file ending in N blank lines needs N-1 writes to settle, emitting a one-line
/// git diff each time. No data is lost and every checked-in track and fixture
/// already ends without a trailing newline, so this is cosmetic churn rather
/// than corruption — but it is a real instability, found by this property before
/// it was scoped out.
fn trim_trailing_newlines(s: &str) -> &str {
    s.trim_end_matches('\n')
}

// ---------------------------------------------------------------------------
// Corpus: the checked-in fixtures, as mutation seeds
// ---------------------------------------------------------------------------

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn fixture_sources() -> Vec<String> {
    let mut out = Vec::new();
    for entry in fs::read_dir(fixture_dir()).expect("fixtures dir readable") {
        let path = entry.expect("readable dir entry").path();
        if path.extension().is_some_and(|e| e == "md") {
            out.push(fs::read_to_string(&path).expect("fixture readable"));
        }
    }
    assert!(!out.is_empty(), "no .md fixtures found");
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
    "    ```",
    "    ```rust",
    "    ```lace",
    "    § multi-byte at the slice boundary",
    // Indent 1, but byte 4 falls *inside* the second '§' (each is 2 bytes:
    // [0]=' ', [1..3]='§', [3..5]='§'). A note block indent of 4 slicing on byte
    // length rather than indent splits the character and aborts the process.
    " §§ dedented mid-character",
    "    body text",
    // Note lines that are *more* indented than the block are absent for the same
    // reason as the orphan lines above: on their own they reduce to a single-line
    // note with leading whitespace, which is a known-unstable shape. See
    // `single_line_note_loses_leading_whitespace`. Multi-line notes keep their
    // relative indentation and are covered by `arb_note` in P3.
    "- [ ] `T-003` trailing",
    // Bare indented task lines are deliberately absent from this pool. Lines are
    // drawn independently, so they would usually land with no parent above them —
    // and an orphaned subtask is silently dropped by the parser today. See
    // `orphaned_subtask_is_silently_dropped`, which pins that defect. Leaving
    // them in would make P1/P4 fail on the known bug on most runs and drown out
    // everything else they are meant to find. Nesting is still covered, with real
    // parents, by `arb_task_tree` in P3.
    "not a task line at all",
    "```",
    "§",
];

fn arb_soup() -> impl Strategy<Value = String> {
    prop::collection::vec(
        prop::sample::select(INTERESTING_LINES).prop_map(str::to_string),
        0..40,
    )
    .prop_map(|lines| lines.join("\n"))
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
    let mut lines: Vec<String> = source.lines().map(str::to_string).collect();
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

fn arb_track_model() -> impl Strategy<Value = Track> {
    (
        prop::collection::vec(arb_task_tree(), 0..3),
        prop::collection::vec(arb_task_tree(), 0..2),
        prop::collection::vec(arb_task_tree(), 0..2),
    )
        .prop_map(|(backlog, parked, done)| Track {
            title: "Generated Track".to_string(),
            description: None,
            nodes: vec![
                TrackNode::Literal(vec!["# Generated Track".to_string(), String::new()]),
                section(SectionKind::Backlog, backlog, false),
                section(SectionKind::Parked, parked, false),
                section(SectionKind::Done, done, true),
            ],
            source_lines: Vec::new(),
        })
}

fn arb_inbox_model() -> impl Strategy<Value = Inbox> {
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
    )
    .prop_map(|items| Inbox {
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

        prop_assert_eq!(track_shape(&before), track_shape(&after));
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
// P4 — canonical form is a fixpoint
// ---------------------------------------------------------------------------

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// For damaged input there is no ground truth to compare against, but
    /// stability is still checkable: one rewrite may normalize, a second may not
    /// change anything. A parser that reads its own output differently shows up
    /// here even when nothing else can judge the result.
    ///
    /// Trailing newlines are excluded — see `trim_trailing_newlines`.
    #[test]
    fn p4_track_canonical_form_is_stable(source in arb_soup()) {
        let once = canon_track(&source);
        let twice = canon_track(&once);
        prop_assert_eq!(trim_trailing_newlines(&once), trim_trailing_newlines(&twice));
    }

    #[test]
    fn p4_inbox_canonical_form_is_stable(source in arb_soup()) {
        let once = canon_inbox(&source);
        let twice = canon_inbox(&once);
        prop_assert_eq!(trim_trailing_newlines(&once), trim_trailing_newlines(&twice));
    }

    #[test]
    fn p4_mutated_fixture_canonical_form_is_stable(
        which in 0usize..64,
        idx in 0usize..512,
        mutation in arb_mutation(),
    ) {
        let corpus = fixture_sources();
        let source = &corpus[which % corpus.len()];
        let damaged = mutate(source, idx, &mutation);

        let once = canon_track(&damaged);
        let twice = canon_track(&once);
        prop_assert_eq!(trim_trailing_newlines(&once), trim_trailing_newlines(&twice));
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

/// **Known defect, found by `p4_track_canonical_form_is_stable`.** An indented
/// task line with no parent above it is silently discarded: it becomes neither a
/// task in the section nor a literal node, so the next write deletes it from the
/// file. `fr check` cannot see it either — by the time check runs, the task is
/// already gone from the parsed model.
///
/// Shapes confirmed, all of which lose the task:
///
/// - `## Backlog` / `  - [ ] A`            → section with 0 tasks, no literal
/// - `## Backlog` / `` / `  - [ ] A`       → same, blank absorbed into the header
/// - two orphans split by a blank line     → the first is dropped, the second
///   survives only as a literal
///
/// This is the `eff5ec0` class again: the parser silently drops content, nothing
/// warns, and the next write commits the loss. It is reachable from a hand edit
/// that removes a parent task, and from a three-way merge that keeps a subtask
/// whose parent went away.
///
/// Ignored so it does not block CI. Un-ignore when the parser preserves the
/// line — as a task, promoted or not, or at minimum as a literal.
#[test]
#[ignore = "known defect: an orphaned subtask line is silently dropped by the parser"]
fn orphaned_subtask_is_silently_dropped() {
    let source = "\
# Track

## Backlog

  - [ ] `T-001` Orphaned subtask with no parent";

    let track = parse_track(source);
    let rewritten = serialize_track(&track);

    assert!(
        rewritten.contains("T-001"),
        "orphaned subtask was deleted by the round trip: {rewritten:?}"
    );
}

/// **Known defect, found by `p4_track_canonical_form_is_stable`.** A note that
/// reduces to a single line with leading whitespace has no faithful single-line
/// representation, so the indentation is lost on the *second* write.
///
/// `Note("  deeper body")` serializes to `- note:   deeper body`, which re-parses
/// as `Note("deeper body")` — the parser trims after the key, and there is no
/// way to tell the two apart. Multi-line notes are unaffected: they take the
/// block form, which preserves relative indentation and is stable.
///
/// Minor next to the orphan-drop above — indentation on a one-line note, not a
/// task — but it is the same failure mode: the file changes under a write that
/// was not asked to change it. The fix is on the serializer side: use the block
/// form whenever the note has leading whitespace.
///
/// Ignored so it does not block CI. Un-ignore when the round trip is stable.
#[test]
#[ignore = "known defect: a single-line note loses leading whitespace on rewrite"]
fn single_line_note_loses_leading_whitespace() {
    let source = "\
# Track

## Backlog

- [ ] `T-001` Task
  - note:
      indented one-liner";

    let once = canon_track(source);
    let twice = canon_track(&once);

    assert_eq!(once, twice, "note indentation changed on the second write");
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
- [ ] `T-002` Sibling";

    let track = parse_track(source);
    let _ = serialize_track(&track);
    assert_eq!(track.backlog().len(), 2);
}
