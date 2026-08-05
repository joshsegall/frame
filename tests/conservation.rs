//! P7 — conservation across **operation sequences**.
//!
//! # The gap this closes
//!
//! `tests/parse_properties.rs` states five properties about the parse/serialize
//! pair, and P5 is the strong one: a plain write accounts for every line. But
//! all of them stop at that pair. None says anything about `fr clean`,
//! `fr check --fix`, `fr triage` or a state change — each of which rewrites a
//! whole track file.
//!
//! That distinction is exactly where the worst defect so far reached the user.
//! `3447fb6` was a parser bug, but what made it *routine* was `fr clean`:
//! filling one task's missing `resolved:` date rewrites the entire track, so a
//! note line vanished from a done task in a track nobody had touched, inside a
//! large boring diff. The parser property caught the bug once it was written;
//! nothing was watching the operation that delivered it.
//!
//! So this generates a random sequence of **real operations** against a real
//! project on disk, reloading from disk between steps so every step goes
//! through parse → operate → serialize → write, and asserts after each one:
//!
//! 1. **No title disappears.** Every task title that existed still exists
//!    somewhere under `frame/` — a track or an archive. Titles are what the
//!    user typed; no operation here is licensed to change one except
//!    `EditTitle`, and none is licensed to lose one.
//! 2. **No ID disappears**, under the same rule.
//! 3. **Unowned content survives.** Lines frame preserves but does not
//!    understand — a stranded line, an orphaned task line, content indented
//!    past its metadata — must still be there. This is P5's claim, moved up to
//!    the operation layer, and it is the one that would have caught `3447fb6`
//!    where it actually hurt.
//! 4. **Every file frame writes is already settled.** Re-serializing a file
//!    frame just wrote must not change it. An operation that leaves a file one
//!    rewrite away from its own fixpoint is churn — `f1a4ff5` and `0dfa9d1`
//!    were both that, found only after the diff showed up in someone's
//!    `git status`. P4 asserts convergence for the parse/serialize pair; this
//!    asserts operations land *on* the fixpoint rather than near it.
//!
//! # Licensed removal
//!
//! Two operations are allowed to take something away, and the harness tracks
//! what: `Delete` removes one task's subtree, and `EditTitle` retires the old
//! title. Everything else must conserve. Cross-track moves are deliberately
//! **not** in the op set — they re-mint the task's ID by design, so ID
//! conservation would need a rename map, and `tests/merge_simulation.rs`
//! already owns namespace behaviour.
//!
//! # In-process, against the real ops layer
//!
//! Operations go through `ops::` and `io::project_io` rather than the `fr`
//! binary. A subprocess per step would put thousands of process spawns in a
//! proptest run; the code under test — parse, ops, serialize, `atomic_write` —
//! is the same either way. `tests/cli_integration.rs` covers the CLI layer.

use std::collections::BTreeSet;
use std::path::Path;

use frame::io::actors::IdScope;
use frame::io::project_io;
use frame::model::project::Project;
use frame::model::task::{Task, TaskState};
use frame::model::track::{Track, TrackNode};
use frame::ops::ids::Mint;
use frame::ops::task_ops::{self, InsertPosition};
use frame::ops::{check, clean, fix, inbox_ops};
use frame::parse::{parse_track, serialize_track};
use proptest::prelude::*;

// ---------------------------------------------------------------------------
// The base project
// ---------------------------------------------------------------------------

/// A track carrying the shapes frame preserves but does not own, alongside
/// ordinary tasks.
///
/// Two shapes, both from real defects: the stray line between tasks is
/// `3447fb6`, and the line indented past a subtask's metadata is `e89450d`. The
/// second was taken out for one commit after this property found F12 in it, and
/// is back now that `trailing_lines` anchors it to the task it sits under.
const TRACK_A: &str = "\
# Alpha

> A track with awkward corners.

## Backlog

- [ ] `A-001` First task #core
  - added: 2026-01-01
  a stray line between two tasks
- [ ] `A-002` Second task
  - added: 2026-01-01
  - note: a note that says something
  - [ ] `A-002.1` A subtask
    - added: 2026-01-01
      content indented past its metadata
- [ ] `A-003` Third task
  - added: 2026-01-01

## Parked

- [~] `A-010` Parked idea
  - added: 2026-01-01

## Done

- [x] `A-020` Already finished
  - added: 2026-01-01
  - resolved: 2026-01-02
";

const TRACK_B: &str = "\
# Beta

## Backlog

- [ ] `B-001` Beta work
  - added: 2026-01-01

## Done
";

const INBOX: &str = "\
# Inbox

- something captured earlier #idea
- another thought
";

const PROJECT_TOML: &str = "\
[project]
name = \"conservation\"

[clean]
auto_clean = true
done_threshold = 2
done_retain = 0

[[tracks]]
id = \"alpha\"
name = \"Alpha\"
state = \"active\"
file = \"tracks/alpha.md\"

[[tracks]]
id = \"beta\"
name = \"Beta\"
state = \"active\"
file = \"tracks/beta.md\"

[ids.prefixes]
alpha = \"A\"
beta = \"B\"
";

fn build_project(root: &Path) {
    let frame_dir = root.join("frame");
    std::fs::create_dir_all(frame_dir.join("tracks")).unwrap();
    std::fs::write(frame_dir.join(".actor"), "null\n").unwrap();
    std::fs::write(frame_dir.join("project.toml"), PROJECT_TOML).unwrap();
    std::fs::write(frame_dir.join("tracks/alpha.md"), TRACK_A).unwrap();
    std::fs::write(frame_dir.join("tracks/beta.md"), TRACK_B).unwrap();
    std::fs::write(frame_dir.join("inbox.md"), INBOX).unwrap();
}

// ---------------------------------------------------------------------------
// What must be conserved
// ---------------------------------------------------------------------------

/// The lines frame keeps but does not model: a stray line, an orphan, content
/// past its metadata. Held verbatim so a rewrite that drops one is visible.
///
/// Taken from the base track rather than derived, because deriving them from
/// the parse would ask the parser what it kept — and a line the parser dropped
/// is missing from that answer too. Same reasoning as P5.
const UNOWNED_LINES: &[&str] = &[
    "a stray line between two tasks",
    "content indented past its metadata",
];

/// Every `.md` under `frame/`, so conservation is judged across tracks and
/// archives together — a task moving into the archive is not a loss.
fn all_markdown(frame_dir: &Path) -> Vec<(std::path::PathBuf, String)> {
    let mut out = Vec::new();
    let mut stack = vec![frame_dir.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "md")
                && let Ok(text) = std::fs::read_to_string(&path)
            {
                out.push((path, text));
            }
        }
    }
    out.sort();
    out
}

fn walk<'a>(task: &'a Task, out: &mut Vec<&'a Task>) {
    out.push(task);
    for sub in &task.subtasks {
        walk(sub, out);
    }
}

fn tasks_of(track: &Track) -> Vec<&Task> {
    let mut out = Vec::new();
    for node in &track.nodes {
        if let TrackNode::Section { tasks, .. } = node {
            for task in tasks {
                walk(task, &mut out);
            }
        }
    }
    out
}

/// Titles and IDs present anywhere under `frame/`.
///
/// Tracks and archives are read differently because they *are* different: an
/// archive file is a flat task list under a `# Archive — <id>` heading, with no
/// `## Section` headers, so walking sections finds nothing in one. That is what
/// `project_io::load_archives` is for, and using it here means the harness
/// agrees with the code under test about where an archived task lives rather
/// than inventing a second reading of the format.
fn present(frame_dir: &Path) -> (BTreeSet<String>, BTreeSet<String>) {
    let mut titles = BTreeSet::new();
    let mut ids = BTreeSet::new();
    let mut record = |task: &Task| {
        if !task.title.trim().is_empty() {
            titles.insert(task.title.clone());
        }
        if let Some(id) = &task.id {
            ids.insert(id.to_string());
        }
    };

    for (path, text) in all_markdown(frame_dir) {
        if path.components().any(|c| c.as_os_str() == "archive") {
            continue;
        }
        let track = parse_track(&text);
        for task in tasks_of(&track) {
            record(task);
        }
    }

    for (_, tasks) in project_io::load_archives(frame_dir).unwrap_or_default() {
        let mut flat = Vec::new();
        for task in &tasks {
            walk(task, &mut flat);
        }
        for task in flat {
            record(task);
        }
    }

    // `archive/_tracks/` holds whole archived track files, which `load_archives`
    // skips. They are still part of the project's content.
    let whole = frame_dir.join("archive").join("_tracks");
    if let Ok(entries) = std::fs::read_dir(&whole) {
        for entry in entries.flatten() {
            if entry.path().extension().is_some_and(|e| e == "md")
                && let Ok(text) = std::fs::read_to_string(entry.path())
            {
                let track = parse_track(&text);
                for task in tasks_of(&track) {
                    record(task);
                }
            }
        }
    }

    (titles, ids)
}

/// The whole of `frame/`'s markdown, for substring checks on unowned lines.
fn all_text(frame_dir: &Path) -> String {
    all_markdown(frame_dir)
        .into_iter()
        .map(|(_, t)| t)
        .collect::<Vec<_>>()
        .join("\n")
}

// ---------------------------------------------------------------------------
// Operations
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
enum Op {
    AddTask { track: usize, title: String },
    SetState { task: usize, state: TaskState },
    SetNote { task: usize, text: String },
    EditTitle { task: usize, title: String },
    AddTag { task: usize, tag: String },
    Delete { task: usize },
    Capture { text: String },
    Triage { item: usize, track: usize },
    Clean,
    CheckFix,
}

fn arb_title() -> impl Strategy<Value = String> {
    // No backtick (re-parses as an ID delimiter), no '#' (re-parses as a tag),
    // no leading/trailing space (trimmed on read). Multi-byte is wanted.
    prop::sample::select(
        [
            "a plain new task",
            "task with §unicode",
            "task: with punctuation",
            "a much longer title than the others so wrapping has something to do",
        ]
        .as_slice(),
    )
    .prop_map(str::to_string)
}

fn arb_op() -> impl Strategy<Value = Op> {
    prop_oneof![
        4 => (0usize..2, arb_title()).prop_map(|(track, title)| Op::AddTask { track, title }),
        4 => (0usize..12, prop::sample::select(
            [TaskState::Todo, TaskState::Active, TaskState::Done, TaskState::Parked, TaskState::Blocked].as_slice(),
        ))
            .prop_map(|(task, state)| Op::SetState { task, state }),
        3 => (0usize..12, prop::sample::select(
            ["a note", "a note\nwith two lines", "note with §unicode"].as_slice(),
        ))
            .prop_map(|(task, text)| Op::SetNote { task, text: text.to_string() }),
        2 => (0usize..12, arb_title()).prop_map(|(task, title)| Op::EditTitle { task, title }),
        2 => (0usize..12, prop::sample::select(["bug", "core", "later"].as_slice()))
            .prop_map(|(task, tag)| Op::AddTag { task, tag: tag.to_string() }),
        1 => (0usize..12).prop_map(|task| Op::Delete { task }),
        2 => arb_title().prop_map(|text| Op::Capture { text }),
        2 => (0usize..4, 0usize..2).prop_map(|(item, track)| Op::Triage { item, track }),
        3 => Just(Op::Clean),
        2 => Just(Op::CheckFix),
    ]
}

/// What an operation is licensed to remove. Everything else must conserve.
#[derive(Default)]
struct Licensed {
    titles: BTreeSet<String>,
    ids: BTreeSet<String>,
}

/// Every (track, id) pair in the project, in a stable order, for ops that
/// address a task by index across the whole project.
fn addressable(project: &Project) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for (track_id, track) in &project.tracks {
        for task in tasks_of(track) {
            if let Some(id) = &task.id {
                out.push((track_id.clone(), id.to_string()));
            }
        }
    }
    out
}

fn save_all(project: &Project) {
    for (track_id, track) in &project.tracks {
        let file = project
            .config
            .tracks
            .iter()
            .find(|t| &t.id == track_id)
            .map(|t| t.file.clone())
            .unwrap_or_else(|| format!("tracks/{track_id}.md"));
        project_io::save_track(&project.frame_dir, &file, track).unwrap();
    }
    if let Some(inbox) = &project.inbox {
        project_io::save_inbox(&project.frame_dir, inbox).unwrap();
    }
}

/// Apply one operation to a project loaded from disk, and write the result
/// back. Returns what the operation was licensed to remove.
fn apply_op(project: &mut Project, op: &Op) -> Licensed {
    let mut licensed = Licensed::default();
    let frame_dir = project.frame_dir.clone();

    match op {
        Op::AddTask { track, title } => {
            let idx = track % project.tracks.len();
            let (track_id, prefix) = {
                let (id, _) = &project.tracks[idx];
                let prefix = project.config.ids.prefixes.get(id).cloned();
                (id.clone(), prefix)
            };
            let Some(prefix) = prefix else {
                return licensed;
            };
            let mint = Mint::new(&frame_dir, &track_id, &prefix, None);
            let (_, track) = &mut project.tracks[idx];
            let _ = task_ops::add_task(track, title.clone(), InsertPosition::Bottom, mint);
        }
        Op::SetState { task, state } => {
            let all = addressable(project);
            if all.is_empty() {
                return licensed;
            }
            let (track_id, id) = all[task % all.len()].clone();
            if let Some((_, track)) = project.tracks.iter_mut().find(|(t, _)| *t == track_id)
                && let Some(t) = task_ops::find_task_mut_in_track(track, &id)
            {
                task_ops::set_state(t, *state);
            }
            // A state change can imply a section move; the ops layer owns that
            // rule and `reconcile_sections` is how every caller applies it.
            clean::reconcile_sections(project);
        }
        Op::SetNote { task, text } => {
            let all = addressable(project);
            if all.is_empty() {
                return licensed;
            }
            let (track_id, id) = all[task % all.len()].clone();
            if let Some((_, track)) = project.tracks.iter_mut().find(|(t, _)| *t == track_id) {
                let _ = task_ops::set_note(track, &id, text.clone());
            }
        }
        Op::EditTitle { task, title } => {
            let all = addressable(project);
            if all.is_empty() {
                return licensed;
            }
            let (track_id, id) = all[task % all.len()].clone();
            if let Some((_, track)) = project.tracks.iter_mut().find(|(t, _)| *t == track_id) {
                // The old title is retired by this operation, and only this one.
                if let Some(t) = task_ops::find_task_in_track(track, &id) {
                    licensed.titles.insert(t.title.clone());
                }
                let _ = task_ops::edit_title(track, &id, title.clone());
            }
        }
        Op::AddTag { task, tag } => {
            let all = addressable(project);
            if all.is_empty() {
                return licensed;
            }
            let (track_id, id) = all[task % all.len()].clone();
            if let Some((_, track)) = project.tracks.iter_mut().find(|(t, _)| *t == track_id) {
                let _ = task_ops::add_tag(track, &id, tag);
            }
        }
        Op::Delete { task } => {
            let all = addressable(project);
            if all.is_empty() {
                return licensed;
            }
            let (track_id, id) = all[task % all.len()].clone();
            if let Some((_, track)) = project.tracks.iter_mut().find(|(t, _)| *t == track_id) {
                // The subtree goes with it, so everything under it is licensed.
                if let Some(t) = task_ops::find_task_in_track(track, &id) {
                    let mut subtree = Vec::new();
                    walk(t, &mut subtree);
                    for s in subtree {
                        licensed.titles.insert(s.title.clone());
                        if let Some(sid) = &s.id {
                            licensed.ids.insert(sid.to_string());
                        }
                    }
                }
                let _ = task_ops::delete_task(track, &id);
            }
        }
        Op::Capture { text } => {
            if let Some(inbox) = project.inbox.as_mut() {
                inbox_ops::add_inbox_item(inbox, text.clone(), Vec::new(), None);
            }
        }
        Op::Triage { item, track } => {
            let idx = track % project.tracks.len();
            let (track_id, prefix) = {
                let (id, _) = &project.tracks[idx];
                (id.clone(), project.config.ids.prefixes.get(id).cloned())
            };
            let Some(prefix) = prefix else {
                return licensed;
            };
            let Some(inbox) = project.inbox.as_mut() else {
                return licensed;
            };
            if inbox.items.is_empty() {
                return licensed;
            }
            let i = item % inbox.items.len();
            let mint = Mint::new(&frame_dir, &track_id, &prefix, None);
            let (_, track) = &mut project.tracks[idx];
            let _ = inbox_ops::triage(inbox, i, track, InsertPosition::Bottom, mint);
        }
        Op::Clean => {
            clean::clean_project(project, IdScope::Mint(None));
        }
        Op::CheckFix => {
            let plan = fix::plan(&check::check_project(project));
            fix::apply(project, &plan);
        }
    }

    save_all(project);
    licensed
}

// ---------------------------------------------------------------------------
// The property
// ---------------------------------------------------------------------------

/// Assert every file frame just wrote is a fixpoint of the parse/serialize
/// pair. An operation that leaves a file one rewrite from settling produces a
/// diff nobody asked for on the next unrelated command.
fn assert_settled(frame_dir: &Path, step: usize, op: &Op) -> Result<(), TestCaseError> {
    for (path, text) in all_markdown(frame_dir) {
        // Archives are appended to, not round-tripped, and `_tracks/` holds
        // whole files moved verbatim. Both are checked for content below.
        if path.components().any(|c| c.as_os_str() == "archive") {
            continue;
        }
        let rewritten = serialize_track(&parse_track(&text));
        if rewritten != text {
            return Err(TestCaseError::fail(format!(
                "step {step} ({op:?}) left {} unsettled\nwrote:     {text:?}\nrewrites to: {rewritten:?}",
                path.display()
            )));
        }
    }
    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(96))]

    /// The capstone: a random sequence of real operations conserves every title,
    /// every ID, and every line frame does not own — and settles each file it
    /// writes.
    #[test]
    fn p7_an_operation_sequence_conserves_content(ops in prop::collection::vec(arb_op(), 1..9)) {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();
        build_project(root);
        let frame_dir = root.join("frame");

        let (mut titles, mut ids) = present(&frame_dir);
        let mut expected_unowned: Vec<&str> = UNOWNED_LINES.to_vec();

        for (step, op) in ops.iter().enumerate() {
            let mut project = project_io::load_project(root).expect("project loads");
            let licensed = apply_op(&mut project, op);

            for t in &licensed.titles {
                titles.remove(t);
            }
            for i in &licensed.ids {
                ids.remove(i);
            }

            let (now_titles, now_ids) = present(&frame_dir);

            let lost_titles: Vec<_> = titles.difference(&now_titles).cloned().collect();
            prop_assert!(
                lost_titles.is_empty(),
                "step {step} ({op:?}) lost titles: {lost_titles:?}"
            );

            let lost_ids: Vec<_> = ids.difference(&now_ids).cloned().collect();
            prop_assert!(
                lost_ids.is_empty(),
                "step {step} ({op:?}) lost ids: {lost_ids:?}"
            );

            let text = all_text(&frame_dir);
            if matches!(op, Op::SetNote { .. }) {
                // A note's extent is set by indentation, so unowned content
                // indented under a task becomes part of that task's note the
                // moment one exists — and replacing the note replaces it.
                //
                // That is licensed, not a loss: the content renders inside the
                // note, on the task the user named, and they are replacing what
                // they can see. It is the *cross-task* version that was the bug
                // (F12) — content absorbed into a neighbour's note, deleted by
                // an edit to a task it never belonged to — and
                // `deep_content_survives_a_section_move` pins that separately.
                //
                // So re-baseline rather than assert, and anything consumed here
                // stops being expected for the rest of the run.
                expected_unowned.retain(|line| text.contains(line));
            } else {
                for line in &expected_unowned {
                    prop_assert!(
                        text.contains(line),
                        "step {step} ({op:?}) dropped a line frame does not own: {line:?}"
                    );
                }
            }

            assert_settled(&frame_dir, step, op)?;

            // Anything the operation added joins the set that must survive from
            // here on, so a later step cannot quietly drop it either.
            titles = now_titles;
            ids = now_ids;
        }
    }
}

// ---------------------------------------------------------------------------
// Fixed cases
// ---------------------------------------------------------------------------

/// The `3447fb6` shape, as a named case: `fr clean` rewrites a whole track to
/// fill one missing date, and a line it does not understand must ride through.
#[test]
fn clean_does_not_drop_a_line_it_does_not_understand() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path();
    build_project(root);
    let frame_dir = root.join("frame");

    let mut project = project_io::load_project(root).unwrap();
    clean::clean_project(&mut project, IdScope::Mint(None));
    save_all(&project);

    let text = all_text(&frame_dir);
    for line in UNOWNED_LINES {
        assert!(text.contains(line), "clean dropped {line:?}:\n{text}");
    }
}

/// A task archived by `fr clean` leaves the track and must arrive in the
/// archive — conservation across two files, which is the property the
/// append-before-remove ordering exists to keep.
#[test]
fn a_cleaned_task_moves_to_the_archive_rather_than_away() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path();
    build_project(root);
    let frame_dir = root.join("frame");

    // Two more done tasks, over the threshold of 2.
    let path = frame_dir.join("tracks/alpha.md");
    let mut body = std::fs::read_to_string(&path).unwrap();
    body.push_str(
        "- [x] `A-021` Second finished\n  - added: 2026-01-01\n  - resolved: 2026-01-02\n\
         - [x] `A-022` Third finished\n  - added: 2026-01-01\n  - resolved: 2026-01-02\n",
    );
    std::fs::write(&path, body).unwrap();

    let before = present(&frame_dir);
    let mut project = project_io::load_project(root).unwrap();
    clean::clean_project(&mut project, IdScope::Mint(None));
    save_all(&project);

    let after = present(&frame_dir);
    assert!(
        before.0.is_subset(&after.0),
        "titles left the project: {:?}",
        before.0.difference(&after.0).collect::<Vec<_>>()
    );
    assert!(
        before.1.is_subset(&after.1),
        "ids left the project: {:?}",
        before.1.difference(&after.1).collect::<Vec<_>>()
    );
    assert!(
        frame_dir.join("archive/alpha.md").exists(),
        "and the archive is where they went"
    );
}

/// The F12 regression, kept as a named case beside the property that found it.
///
/// `leading_lines` hold a line the parser could not attribute, on the *next*
/// task — so where the line lands in the written file depends on what its
/// neighbours are. Move the task in between away and it lands somewhere else
/// entirely, and here "somewhere else" is inside another task's note:
///
/// 1. `A-002.1`'s over-indented content was carried on the following task,
///    `A-003`.
/// 2. Marking `A-002` done moved it and its subtree to `## Done`. `A-003` stayed
///    put, and the line it carried then rendered straight after `A-001`'s note
///    block — at an indent that made it part of that note.
/// 3. `fr note A-001 ...`, an ordinary edit of an unrelated task, replaced that
///    note. The line went with it.
///
/// Worse than a plain drop, because the content crosses tasks before it dies:
/// the user editing `A-001` is deleting something that belonged to `A-002.1`,
/// with nothing on screen to say so.
///
/// Fixed by anchoring such content to the task it sits *under*
/// (`Task::trailing_lines`), so it travels with that task instead of being left
/// behind for a neighbour to absorb.
#[test]
fn deep_content_survives_a_section_move() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path();
    build_project(root);
    let frame_dir = root.join("frame");

    // Put the shape back for this test only.
    let path = frame_dir.join("tracks/alpha.md");
    let body = std::fs::read_to_string(&path).unwrap().replace(
        "  - [ ] `A-002.1` A subtask\n    - added: 2026-01-01\n",
        "  - [ ] `A-002.1` A subtask\n    - added: 2026-01-01\n      deep content\n",
    );
    std::fs::write(&path, body).unwrap();

    let mut project = project_io::load_project(root).unwrap();
    apply_op(
        &mut project,
        &Op::SetNote {
            task: 0,
            text: "a note\nwith two lines".into(),
        },
    );

    let mut project = project_io::load_project(root).unwrap();
    apply_op(
        &mut project,
        &Op::SetState {
            task: 1,
            state: TaskState::Done,
        },
    );

    let mut project = project_io::load_project(root).unwrap();
    apply_op(
        &mut project,
        &Op::SetNote {
            task: 0,
            text: "a note".into(),
        },
    );

    assert!(
        all_text(&frame_dir).contains("deep content"),
        "an edit to A-001 must not delete content that belonged to A-002.1"
    );
}

/// The other half of the F12 story, pinned so it stays a decision rather than a
/// gap: a note absorbs unowned content indented under **its own** task.
///
/// `- note:` takes its extent from indentation (`doc/format.md`), and
/// `trailing_lines` must be emitted after all metadata — anything after a
/// stranded run stops being collected as metadata (`e89450d`), so putting them
/// earlier would lose the note itself. A note written in **block** form
/// therefore ends at the same indent the stranded run sits at, and claims it;
/// from then on the content is note text. A single-line `- note: x` does not,
/// which is why only the block form is pinned here.
///
/// Licensed, and materially different from F12: the content renders inside the
/// note of the task the user named, so replacing that note replaces what they
/// can see. F12 was the same absorption by a *neighbouring* task's note, where
/// the user had no reason to connect the edit to the content it destroyed.
#[test]
fn a_note_absorbs_unowned_content_under_its_own_task() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path();
    build_project(root);
    let frame_dir = root.join("frame");

    let mut project = project_io::load_project(root).unwrap();
    apply_op(
        &mut project,
        &Op::SetNote {
            task: 2, // A-002.1, the task carrying the deep content
            text: "a note\nwith two lines".into(),
        },
    );

    // Still in the file, now as part of that task's note.
    assert!(all_text(&frame_dir).contains("content indented past its metadata"));

    let project = project_io::load_project(root).unwrap();
    let sub = task_ops::find_task_in_track(&project.tracks[0].1, "A-002.1").unwrap();
    assert!(
        sub.trailing_lines.is_empty(),
        "the note claimed it, so it is no longer unowned: {:?}",
        sub.trailing_lines
    );
    assert!(
        sub.metadata.iter().any(|m| matches!(
            m,
            frame::model::task::Metadata::Note(n) if n.contains("content indented")
        )),
        "and it is note text now: {:?}",
        sub.metadata
    );
}
