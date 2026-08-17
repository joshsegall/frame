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
//! 5. **No dependency is broken.** Every `dep:` that resolved before an
//!    operation names the same task after it. See below — this one is stated
//!    in titles, not ids, and the reason is the whole point of it.
//!
//! # Claim 5, and why it is not about dangling deps
//!
//! The obvious statement is "no operation increases the set of dangling deps",
//! and it is too weak to be worth writing. A dep rewritten to point at a
//! *different existing task* satisfies it, and two real defects have exactly
//! that shape: the chained rewrite `apply_id_map_to_deps` exists to prevent,
//! and the positional undo in
//! `cross_track_move_undo_restores_out_of_order_subtask_ids`, where re-keying
//! `.1 .3 .2` back by position handed two tasks each other's id. Every dep
//! still resolved in both.
//!
//! So the claim is about **identity**, and what identifies a task across a
//! renumbering is its title — which claims 1 and 2 already conserve.
//! `resolved_dep_pairs` reads every dep that resolves and names both ends by
//! title; the claim is that the set only shrinks where a title was licensed to
//! go. It is checked per step rather than against a run-wide baseline, so a dep
//! an earlier step created is covered too.
//!
//! **Titles have to be unique for that to mean anything**, which is what
//! `unique` is for — `arb_title` draws from four fixed strings and a repeat
//! over eight steps is close to certain. That also closes a smaller hole in
//! claims 1 and 2: `titles` is a set, so two tasks sharing a title collapse to
//! one entry and losing one of them is invisible.
//!
//! **What it does not catch**, stated so nobody assumes otherwise: a *chained*
//! rewrite needs a freshly minted id to equal an id the same map retired, which
//! a real mint does not produce. That guard is defended by the unit test
//! `an_id_map_matches_the_pre_image_and_never_chains`, not by this property.
//!
//! # Licensed removal
//!
//! Four operations are allowed to take something away, and the harness tracks
//! what: `Delete` removes one task's subtree, `EditTitle` retires the old title,
//! `RenamePrefix` retires every ID under the old prefix — live and archived
//! alike, which is the point of having it here — and `MoveToTrack` retires
//! every ID in the subtree it re-mints. Everything else must conserve.
//!
//! `MoveToTrack` was deliberately **out** of the op set until `267f671`, on the
//! stated grounds that it re-mints IDs by design so ID conservation would need
//! a rename map. `move_task_to_track` now returns that map, so the reason is
//! spent and the operation that carried the defect is in. Only its *ids* are
//! licensed: a move renames, it does not remove, so every title and every dep
//! must survive it, and that is the half of the operation this property checks.
//!
//! Note what that licensing rests on. The harness licenses from the map the
//! product hands back, so a map that under-reports fails claim 2 rather than
//! claim 5 — which is a fair report of the same defect, and worth knowing when
//! reading a failure. Claim 5 earns its keep on the defects that leave the map
//! correct: a rewrite that misses subtasks, one that skips a whole operation's
//! worth of deps (`RenamePrefix`), and one that edits memory without marking
//! the task dirty so nothing reaches the file.
//!
//! # Three file shapes, three pairs
//!
//! Claim 4 used to skip `frame/archive/` entirely, on the grounds that archives
//! were appended to rather than round-tripped. That was true, and it was the
//! problem: the append was string concatenation, so it could leave a file mixing
//! line endings and no property was watching. Done-task archives now settle
//! under the archive pair, `archive/_tracks/` under the track pair.
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
use frame::model::task::TaskState;
use frame::ops::ids::Mint;
use frame::ops::task_ops::{self, InsertPosition};
use frame::ops::track_ops;
use frame::ops::{check, clean, fix, inbox_ops};
use frame::parse::parse_archive;
use proptest::prelude::*;

#[path = "support/tree_checks.rs"]
mod tree_checks;

use tree_checks::{all_text, present, tasks_of, unsettled, walk};

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
  - [ ] `A-002.2` A second subtask
    - added: 2026-01-01
    - dep: A-001
- [ ] `A-003` Third task
  - added: 2026-01-01
  - dep: A-002.1

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
  - dep: A-002.1

## Done
";

const INBOX: &str = "\
# Inbox

- something captured earlier #idea

a stray line between two inbox items

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
    "a stray line between two inbox items",
];

// Reading the tree — `all_markdown`, `present`, `all_text`, `tasks_of`, `walk`
// and `unsettled` — lives in `tests/support/tree_checks.rs`, shared with P8
// (`concurrency.rs`). Three file shapes settle under three different pairs, and
// getting that wrong under-counts silently; the module docs there say why.

/// Every dependency that **resolves**, named by the titles at both ends.
///
/// Titles rather than ids, and that is the whole design of claim 5. An id is
/// what a renumbering changes, so a claim stated in ids can only ever ask
/// whether the dep still points at *something* — and a dep rewritten to point
/// at the wrong existing task passes that. Two real defects have exactly that
/// shape: the chained rewrite `apply_id_map_to_deps` exists to prevent, where a
/// map holding `A -> B` and `B -> C` applied in two passes carries a dep on `A`
/// all the way to `C`; and the positional inverse in
/// `cross_track_move_undo_restores_out_of_order_subtask_ids`, where undoing a
/// re-key of `.1 .3 .2` handed two tasks each other's id and every reference to
/// either one silently pointed at the other. Both leave every dep resolvable.
///
/// A dep that resolves to nothing is simply absent, so it cannot fail: the
/// claim is about references that worked, not about the count of ones that did
/// not.
///
/// **First id wins** when two tasks somehow share one, because that is what
/// `ops::deps` does when it resolves a dep for real — it takes the first match
/// in track order. A reader that disagreed would report a defect the product
/// does not have.
fn resolved_dep_pairs(frame_dir: &Path) -> BTreeSet<(String, String)> {
    let tasks = tree_checks::all_tasks(frame_dir);
    let mut by_id: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for task in &tasks {
        if let Some(id) = &task.id {
            by_id
                .entry(id.to_string())
                .or_insert_with(|| task.title.clone());
        }
    }
    let mut out = BTreeSet::new();
    for task in &tasks {
        if task.title.trim().is_empty() {
            continue;
        }
        for dep in frame::ops::deps::task_deps(task) {
            if let Some(target) = by_id.get(&dep)
                && !target.trim().is_empty()
            {
                out.insert((task.title.clone(), target.clone()));
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Operations
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
enum Op {
    AddTask {
        track: usize,
        title: String,
    },
    SetState {
        task: usize,
        state: TaskState,
    },
    SetNote {
        task: usize,
        text: String,
    },
    EditTitle {
        task: usize,
        title: String,
    },
    AddTag {
        task: usize,
        tag: String,
    },
    Delete {
        task: usize,
    },
    Capture {
        text: String,
    },
    Triage {
        item: usize,
        track: usize,
    },
    Clean,
    CheckFix,
    RenamePrefix {
        track: usize,
    },
    /// `fr dep <a> add <b>` — so deps accumulate during a run rather than only
    /// existing where the fixture put them.
    AddDep {
        from: usize,
        to: usize,
    },
    /// `fr mv <id> --track <other>` — the operation P7 used to leave out.
    MoveToTrack {
        task: usize,
        track: usize,
    },
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

/// The generated titles, made unique by the step that produced them.
///
/// [`arb_title`] draws from four fixed strings so a shrunk case stays readable,
/// and over eight operations a repeat is close to certain. Two tasks sharing a
/// title break more than the dep claim, which identifies a task by its title:
/// `titles` is a `BTreeSet`, so a repeat collapses to one entry, and losing one
/// of the two tasks is invisible because the other still supplies it. Suffixing
/// keeps the shrink output readable — `a plain new task 3` still says which of
/// the four it was — and makes both claims see one task per title.
///
/// A bare trailing number, because `#` re-parses as a tag and a backtick as an
/// ID delimiter. Same constraints [`arb_title`] documents.
fn unique(title: &str, step: usize) -> String {
    format!("{title} {step}")
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
        1 => (0usize..2).prop_map(|track| Op::RenamePrefix { track }),
        3 => (0usize..12, 0usize..12).prop_map(|(from, to)| Op::AddDep { from, to }),
        3 => (0usize..12, 0usize..2).prop_map(|(task, track)| Op::MoveToTrack { task, track }),
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
fn apply_op(project: &mut Project, op: &Op, step: usize) -> Licensed {
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
            let _ = task_ops::add_task(track, unique(title, step), InsertPosition::Bottom, mint);
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
                let _ =
                    task_ops::set_note(track, &id, text.clone(), task_ops::NoteLimits::default());
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
                let _ = task_ops::edit_title(track, &id, unique(title, step));
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
                inbox_ops::add_inbox_item(inbox, unique(text, step), Vec::new(), None);
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
        // The one op that rewrites an archive without going through `clean`, and
        // the path where the archive half silently did nothing at all: it read
        // the archive as a track, found no `## Section` headers, and wrote
        // nothing while reporting success.
        //
        // Renaming a prefix retires every id under the old one, live and
        // archived alike, so all of them are licensed. What still has to hold is
        // that no *title* moves, and — the reason this op is here — that the
        // archive it rewrites lands settled.
        Op::RenamePrefix { track } => {
            let idx = track % project.tracks.len();
            let track_id = project.tracks[idx].0.clone();
            let Some(old_prefix) = project.config.ids.prefixes.get(&track_id).cloned() else {
                return licensed;
            };
            let new_prefix = format!("{old_prefix}X");

            for task in tasks_of(&project.tracks[idx].1) {
                if let Some(id) = &task.id {
                    licensed.ids.insert(id.to_string());
                }
            }
            let archive_path = frame_dir.join("archive").join(format!("{track_id}.md"));
            if let Ok(text) = std::fs::read_to_string(&archive_path) {
                for task in parse_archive(&text).tasks {
                    let mut flat = Vec::new();
                    walk(&task, &mut flat);
                    for t in flat {
                        if let Some(id) = &t.id {
                            licensed.ids.insert(id.to_string());
                        }
                    }
                }
            }

            let mut tracks = std::mem::take(&mut project.tracks);
            let renamed = track_ops::rename_track_prefix(
                &mut project.config,
                &mut tracks,
                &track_id,
                &old_prefix,
                &new_prefix,
            );
            project.tracks = tracks;
            if renamed.is_ok() {
                let _ = track_ops::rename_archive_prefix(
                    &frame_dir,
                    &track_id,
                    &old_prefix,
                    &new_prefix,
                );
                // The config carries the prefix map, so it has to land too or the
                // next step in the sequence mints under a prefix the files no
                // longer use.
                let _ = frame::io::config_io::write_config_from_struct(&frame_dir, &project.config);
            }
        }
        // Deps that only ever come from the fixture are deps on two tasks, in
        // two places. Generating them puts a reference on whatever the run has
        // built by now — including tasks earlier steps created, moved or
        // re-keyed — which is where a rewrite is most likely to go wrong.
        Op::AddDep { from, to } => {
            let all = addressable(project);
            if all.len() < 2 {
                return licensed;
            }
            let (track_id, id) = all[from % all.len()].clone();
            let (_, dep_id) = all[to % all.len()].clone();
            // A task cannot depend on itself, and a dep on one's own descendant
            // is a cycle `fr check` reports — neither is what this is for.
            if dep_id == id
                || dep_id.starts_with(&format!("{id}."))
                || id.starts_with(&format!("{dep_id}."))
            {
                return licensed;
            }
            let snapshot = project.tracks.clone();
            if let Some((_, track)) = project.tracks.iter_mut().find(|(t, _)| *t == track_id) {
                let _ = task_ops::add_dep(track, &id, &dep_id, &snapshot);
            }
        }
        // The operation this suite used to leave out. The stated reason was that
        // it re-mints the task's ID by design, so ID conservation would need a
        // rename map — `move_task_to_track` now returns exactly that map, so the
        // reason is spent and the op that carried the defect is in.
        Op::MoveToTrack { task, track } => {
            let all = addressable(project);
            if all.is_empty() || project.tracks.len() < 2 {
                return licensed;
            }
            let (source_id, id) = all[task % all.len()].clone();
            let target_idx = track % project.tracks.len();
            let target_id = project.tracks[target_idx].0.clone();
            if target_id == source_id {
                return licensed;
            }
            // Only a top-level task moves cross-track; a subtask is not
            // addressable this way and the op is a no-op for one.
            let Some(prefix) = project.config.ids.prefixes.get(&target_id).cloned() else {
                return licensed;
            };
            let source_idx = project
                .tracks
                .iter()
                .position(|(t, _)| *t == source_id)
                .expect("source track was just addressed");
            if !task_ops::is_top_level_in_section(
                &project.tracks[source_idx].1,
                &id,
                frame::model::SectionKind::Backlog,
            ) && !task_ops::is_top_level_in_section(
                &project.tracks[source_idx].1,
                &id,
                frame::model::SectionKind::Parked,
            ) && !task_ops::is_top_level_in_section(
                &project.tracks[source_idx].1,
                &id,
                frame::model::SectionKind::Done,
            ) {
                return licensed;
            }

            let mint = Mint::new(&frame_dir, &target_id, &prefix, None);
            // Both arms yield (source, target). This used to carry the same
            // second swap the CLI handler did, and for the same misreading of
            // `left`/`right` as positional — so this suite reproduced the defect
            // it exists to catch instead of failing on it. The `if let Ok` below
            // is why that was silent: a reversed move looks for the task in the
            // destination, returns `TaskNotFound`, and the error was dropped, so
            // every backward move was a no-op and P7 held over nothing.
            let (source_track, target_track) = if source_idx < target_idx {
                let (l, r) = project.tracks.split_at_mut(target_idx);
                (&mut l[source_idx].1, &mut r[0].1)
            } else {
                let (l, r) = project.tracks.split_at_mut(source_idx);
                (&mut r[0].1, &mut l[target_idx].1)
            };
            let moved = task_ops::move_task_to_track(
                source_track,
                target_track,
                &id,
                InsertPosition::Bottom,
                mint,
            );
            // Loud rather than swallowed: every precondition the op needs has
            // been checked above, so a failure here is a bug in the code under
            // test, which is exactly what this suite is for.
            let moved = moved.unwrap_or_else(|e| {
                panic!(
                    "move {id} from {source_id} (#{source_idx}) to {target_id} (#{target_idx}): {e}"
                )
            });
            {
                // Every id the move retired is licensed; the new ones are picked
                // up when the step re-baselines. The *titles* are not licensed —
                // a move renames, it does not remove, and that is the half of
                // this operation P7 still gets to check.
                for (old, _) in &moved.id_mappings {
                    licensed.ids.insert(old.clone());
                }
                task_ops::apply_id_map_to_deps(&mut project.tracks, &moved.id_mappings);
            }
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
///
/// A done-task archive used to be exempt from this check entirely, on the
/// grounds that it was appended to rather than round-tripped — which was true,
/// and was the problem: the append was string concatenation, so it could and
/// did leave a file mixing line endings.
fn assert_settled(frame_dir: &Path, step: usize, op: &Op) -> Result<(), TestCaseError> {
    match unsettled(frame_dir) {
        Some(detail) => Err(TestCaseError::fail(format!(
            "step {step} ({op:?}): {detail}"
        ))),
        None => Ok(()),
    }
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
            // Read before the operation runs: claim 5 is about what this one
            // step did to references that worked going into it, so a run-wide
            // baseline would miss every dep a earlier step created.
            let before_pairs = resolved_dep_pairs(&frame_dir);

            let mut project = project_io::load_project(root).expect("project loads");
            let licensed = apply_op(&mut project, op, step);

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

            // Claim 5. A pair may go only when one of its ends was licensed
            // to go: `Delete` takes a task and every reference to it, and
            // `EditTitle` retires the name this claim identifies a task by.
            let broken: Vec<_> = before_pairs
                .difference(&resolved_dep_pairs(&frame_dir))
                .filter(|(from, to)| {
                    !licensed.titles.contains(from) && !licensed.titles.contains(to)
                })
                .cloned()
                .collect();
            prop_assert!(
                broken.is_empty(),
                "step {step} ({op:?}) broke these dependencies (dependent, dependency): {broken:?}"
            );

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
        0,
    );

    let mut project = project_io::load_project(root).unwrap();
    apply_op(
        &mut project,
        &Op::SetState {
            task: 1,
            state: TaskState::Done,
        },
        0,
    );

    let mut project = project_io::load_project(root).unwrap();
    apply_op(
        &mut project,
        &Op::SetNote {
            task: 0,
            text: "a note".into(),
        },
        0,
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
        0,
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
