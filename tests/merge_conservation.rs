//! P10 — conservation across a **three-way merge**.
//!
//! # The question, and why it is not P7's
//!
//! P7 asks whether a sequence of operations conserved what was on disk. This
//! asks something a merge alone can be wrong about: given three versions of one
//! file, does the result still hold what it was handed?
//!
//! It is the property `fr merge` has now been wrong about twice, both times in
//! the same shape and both times in silence. A done archive parsed as a track
//! yielded zero tasks on both sides, so `rebuild` returned our file verbatim and
//! exited 0 — every task only they had archived, gone. Then a heading frame does
//! not model landed in a `TrackNode::Literal`, which the merge did not read at
//! all, so their repair to it was dropped the same way and reported the same
//! way. Each was found by a person checking the file instead of the exit status.
//!
//! # The invariant, and why it is *additions*
//!
//! A line of theirs missing from the result is usually correct: we deleted the
//! task it belonged to, or we edited it and they did not. Requiring every line
//! to survive would fail on the commonest merge there is.
//!
//! But a line that is **not in the ancestor** is different. Nobody can have
//! deleted something that never existed for them to delete, so an addition that
//! vanished is a loss with no decision behind it — every time, on either side.
//! That is the whole property:
//!
//! > On a merge that reports no conflict, every non-blank line either side added
//! > since the ancestor is in the merged file.
//!
//! **A conflict excuses it, and has to.** A conflicted task is *supposed* to
//! lose one side's lines — that is what the recovery log and the non-zero exit
//! are for. Auditing a conflicted merge would report it working as designed.
//! Silence is the failure mode, so silence is what this watches.
//!
//! # Why it generates edits rather than text
//!
//! Random markdown mostly produces files with no tasks in them, which is the one
//! shape this cannot learn anything from. Instead both sides start from one
//! ancestor and each applies a random run of the edits real writers make —
//! adding a task, retitling one, finishing one, deleting one, and the four kinds
//! of change to the text around them that the merge used to be blind to.

use std::collections::HashSet;

use frame::ops::merge_files::{FileKind, MergeReport, merge_text};
use frame::ops::reconcile::ConflictReason;
use proptest::prelude::*;

const STAMP: &str = "2026-08-23T00:00:00Z";

/// One edit a writer makes to a track.
#[derive(Debug, Clone)]
enum Edit {
    AddTask(u32),
    RetitleTask(usize, u32),
    FinishTask(usize),
    DeleteTask(usize),
    /// The four regions the merge could not see: a heading frame does not model,
    /// the `# Title` line, the `> description`, and a note under everything.
    AddHeading(u32),
    Retitle(u32),
    Describe(u32),
    AppendNote(u32),
    /// Formatting, which must never read as a change to anything.
    Respace,
}

fn arb_edit() -> impl Strategy<Value = Edit> {
    prop_oneof![
        4 => (0u32..900).prop_map(Edit::AddTask),
        3 => (0usize..4, 0u32..900).prop_map(|(i, n)| Edit::RetitleTask(i, n)),
        2 => (0usize..4).prop_map(Edit::FinishTask),
        2 => (0usize..4).prop_map(Edit::DeleteTask),
        2 => (0u32..900).prop_map(Edit::AddHeading),
        1 => (0u32..900).prop_map(Edit::Retitle),
        1 => (0u32..900).prop_map(Edit::Describe),
        2 => (0u32..900).prop_map(Edit::AppendNote),
        1 => Just(Edit::Respace),
    ]
}

const ANCESTOR: &str = "\
# Main

> the main track

## Backlog

- [ ] `MAI-001` One
  - added: 2026-01-01
- [ ] `MAI-002` Two
- [ ] `MAI-003` Three

## Done

- [x] `MAI-004` Already done
";

/// Apply one writer's edits to the ancestor. `side` keeps the two writers'
/// additions distinguishable, so an ID minted by one can never be mistaken for
/// the other's — which is the property actor tokens give the real thing.
fn apply(text: &str, side: char, edits: &[Edit]) -> String {
    let mut track = frame::parse::parse_track(text);
    for edit in edits {
        match edit {
            Edit::AddTask(n) => {
                let task = frame::parse::parse_track(&format!(
                    "# x\n\n## Backlog\n\n- [ ] `MAI-{side}{n}` Added by {side}{n}\n"
                ))
                .backlog()
                .to_vec();
                if let Some(tasks) = track.section_tasks_mut(frame::model::SectionKind::Backlog) {
                    tasks.extend(task);
                }
            }
            Edit::RetitleTask(i, n) => {
                if let Some(tasks) = track.section_tasks_mut(frame::model::SectionKind::Backlog)
                    && let Some(task) = tasks.get_mut(*i)
                {
                    task.title = format!("Retitled by {side} to {n}");
                    task.dirty = true;
                }
            }
            Edit::FinishTask(i) => {
                let moved = track
                    .section_tasks_mut(frame::model::SectionKind::Backlog)
                    .filter(|tasks| *i < tasks.len())
                    .map(|tasks| tasks.remove(*i));
                if let Some(mut task) = moved {
                    task.state = frame::model::TaskState::Done;
                    task.dirty = true;
                    track.ensure_section(frame::model::SectionKind::Done);
                    if let Some(done) = track.section_tasks_mut(frame::model::SectionKind::Done) {
                        done.push(task);
                    }
                }
            }
            Edit::DeleteTask(i) => {
                if let Some(tasks) = track.section_tasks_mut(frame::model::SectionKind::Backlog)
                    && *i < tasks.len()
                {
                    tasks.remove(*i);
                }
            }
            Edit::AddHeading(n) => {
                let text = frame::parse::serialize_track(&track);
                let block = format!("## Notes {side}{n}\n\nwritten by {side}, item {n}\n\n");
                return apply_rest(&text.replacen("## Backlog", &format!("{block}## Backlog"), 1));
            }
            Edit::Retitle(n) => {
                let text = frame::parse::serialize_track(&track);
                return apply_rest(&text.replacen("# Main", &format!("# Main {side}{n}"), 1));
            }
            Edit::Describe(n) => {
                let text = frame::parse::serialize_track(&track);
                return apply_rest(&text.replacen(
                    "> the main track",
                    &format!("> the {side}{n} track"),
                    1,
                ));
            }
            Edit::AppendNote(n) => {
                let mut text = frame::parse::serialize_track(&track);
                text.push_str(&format!("\n<!-- note {side}{n} -->\n"));
                return apply_rest(&text);
            }
            Edit::Respace => {
                let text = frame::parse::serialize_track(&track);
                return apply_rest(&text.replace("\n\n", "\n\n\n"));
            }
        }
    }
    frame::parse::serialize_track(&track)
}

/// The text-level edits above rewrite the whole file, so they return early and
/// this carries the result back out. Splitting it keeps `apply` from having to
/// re-parse after every one.
fn apply_rest(text: &str) -> String {
    text.to_string()
}

/// Non-blank lines, whitespace-normalized — the same reading the merge's own
/// audit uses, so this cannot pass by disagreeing with it about what a line is.
fn lines(text: &str) -> HashSet<String> {
    text.lines()
        .map(|line| line.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|line| !line.is_empty())
        .collect()
}

/// Whether the merge's own audit fired.
///
/// **Not an excuse — the thing being tested.** `merge_text` runs
/// [`frame::ops::reconcile::ConflictReason::UnaccountedLoss`] over its own
/// result and turns a silent drop into a conflict, which would otherwise let
/// every case this property exists to catch slip past `is_clean()` as "a
/// conflict, so not our business". So a report carrying one fails here, and the
/// merge is judged on the additions either way.
fn audit_fired(report: &MergeReport) -> bool {
    report
        .conflicts
        .iter()
        .any(|c| c.reason == ConflictReason::UnaccountedLoss)
}

/// Every non-blank line `side` added since `base` that is missing from `merged`.
fn dropped_additions(base: &str, side: &str, merged: &str) -> Vec<String> {
    let base = lines(base);
    let merged = lines(merged);
    lines(side)
        .into_iter()
        .filter(|line| !base.contains(line) && !merged.contains(line))
        .collect()
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// P10: a merge that reports clean has kept every line either side added.
    #[test]
    fn p10_a_clean_merge_drops_no_addition(
        our_edits in prop::collection::vec(arb_edit(), 0..5),
        their_edits in prop::collection::vec(arb_edit(), 0..5),
    ) {
        let ours = apply(ANCESTOR, 'o', &our_edits);
        let theirs = apply(ANCESTOR, 't', &their_edits);

        let (merged, report) = merge_text(FileKind::Track, ANCESTOR, &ours, &theirs, STAMP);

        // A conflict is a decision, announced, with the other side in the
        // recovery log — so it is excused. The audit's own conflict is not: it
        // says the merge lost something it never decided to lose.
        prop_assert!(!audit_fired(&report), "the merge's own audit fired:\n{merged}");
        prop_assume!(report.is_clean());

        let lost_theirs = dropped_additions(ANCESTOR, &theirs, &merged);
        prop_assert!(
            lost_theirs.is_empty(),
            "their additions vanished with no conflict: {lost_theirs:?}\n\
             --- ours ---\n{ours}\n--- theirs ---\n{theirs}\n--- merged ---\n{merged}"
        );

        let lost_ours = dropped_additions(ANCESTOR, &ours, &merged);
        prop_assert!(
            lost_ours.is_empty(),
            "our additions vanished with no conflict: {lost_ours:?}\n\
             --- ours ---\n{ours}\n--- theirs ---\n{theirs}\n--- merged ---\n{merged}"
        );
    }

    /// The same property for a done archive — the shape the first silent loss
    /// happened in, and one the merge reads with a different pair entirely.
    #[test]
    fn p10_a_clean_archive_merge_drops_no_addition(
        ours_extra in prop::collection::vec(0u32..500, 0..4),
        theirs_extra in prop::collection::vec(0u32..500, 0..4),
        our_header in prop::option::of(0u32..500),
        their_header in prop::option::of(0u32..500),
    ) {
        let base = "# Archive — main\n\n- [x] `MAI-001` One\n  - resolved: 2026-01-01\n";
        let build = |side: char, extra: &[u32], header: Option<u32>| {
            let mut text = match header {
                Some(n) => base.replace("# Archive — main", &format!("# Archive — main ({side}{n})")),
                None => base.to_string(),
            };
            for n in extra {
                text.push_str(&format!("- [x] `MAI-{side}{n}` Archived by {side}{n}\n"));
            }
            text
        };
        let ours = build('o', &ours_extra, our_header);
        let theirs = build('t', &theirs_extra, their_header);

        let (merged, report) = merge_text(FileKind::Archive, base, &ours, &theirs, STAMP);
        prop_assert!(!audit_fired(&report), "the merge's own audit fired:\n{merged}");
        prop_assume!(report.is_clean());

        for (label, side) in [("theirs", &theirs), ("ours", &ours)] {
            let lost = dropped_additions(base, side, &merged);
            prop_assert!(
                lost.is_empty(),
                "{label} additions vanished with no conflict: {lost:?}\n--- merged ---\n{merged}"
            );
        }
    }
}
