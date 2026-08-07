//! P9 — undo and redo as a property.
//!
//! The claim is algebraic and users state it themselves: whatever I just did,
//! `u` puts it back. Written out over a whole session:
//!
//! > For a settled project and any sequence of TUI actions — undoing everything
//! > restores the project byte for byte, and redoing everything restores the
//! > result byte for byte.
//!
//! Both halves earn their place. `undone == start` is the claim users rely on.
//! `redone == end` is the one that catches an operation that records too little
//! to replay — which is exactly the shape of the `TrackAdd` display-name loss,
//! where redo re-created a track under a derived name instead of the one the
//! user typed.
//!
//! ## Why this drives `App`, not `UndoStack`
//!
//! `UndoStack::undo` is only half the undo path. Eight of the `Operation`
//! variants — every `Track*` one — hit a match arm in `apply_inverse` that
//! returns `None` with the comment "handled by the caller (needs config +
//! filesystem access)". The caller is `apply_nav_side_effects` in
//! `src/tui/input/common.rs`: the code that rewrites `project.toml`, deletes and
//! re-creates track files, and moves files in and out of `archive/_tracks/`.
//!
//! Both undo defects this project has actually shipped lived there, not in
//! `undo.rs` — F3 (delete a track, undo, get an empty shell back) and the
//! `TrackAdd` display-name loss. A property over
//! `UndoStack::undo(&mut tracks, inbox)` would have passed both times. So the
//! unit under test is the App-level undo, driven the way a user drives it and
//! observed where the damage lands: on disk.
//!
//! ## Three things that follow from that
//!
//! **Actions are performed, not synthesised.** Steps are pressed through
//! `handle_key`, as in `parity.rs`. Pushing generated `Operation` values would
//! test a fiction — a `SectionMove { from_index: 7 }` that no code path
//! produces proves nothing about code paths that do. Performing the action means
//! the recorded operation is whatever the real handler recorded, which is where
//! the bugs are.
//!
//! **Steps are semantic, not keystrokes.** Random `KeyEvent`s spend their time
//! navigating and occasionally wander into edit mode to type garbage. A
//! `tui_steps::Step` is a (which task, which action) pair; the generator emits
//! indices and the runner resolves them against the live app, so a step always
//! names a task that exists at the moment it runs.
//!
//! **The comparison is the on-disk tree.** Model equality is meaningless here:
//! `source_text` means a clean task serialises verbatim while a touched one
//! serialises canonically, so `Task`-level equality would be trivially true or
//! trivially false depending on `dirty`. `tui_steps::frame_tree` is every file
//! under `frame/` minus `LOCAL_ONLY_FRAME_FILES` — fourth consumer of that
//! constant, after `fr init`, `fr check` and `parity.rs`.
//!
//! All three live in `tests/support/tui_steps.rs`, shared with P8
//! (`concurrency.rs`), which drives the same steps against a project a second
//! writer is also writing to. What stays here is the undo-specific part: press
//! `u` until the stack is empty, press `Z` until it is empty the other way, and
//! compare.
//!
//! The fixture must be **settled** (`serialize(parse(x)) == x`), or the first
//! `mark_dirty` canonicalises a task and every diff is noise;
//! [`the_fixture_is_settled`] pins that — for both suites, since both build the
//! same fixture.
//!
//! ## Byte-exact, and unqualified
//!
//! The comparison was expected to need an exemption list: byte-exact flags
//! churn that is not data loss, and `parity.rs` carries `known_divergence` for
//! exactly that. It turned out not to. Every divergence this suite found on the
//! way in was a defect worth fixing — including the two that looked cosmetic, a
//! blank line at end of file and a doubled separator in a drained section, both
//! of which turned out to be *accumulating* and invisible to every other check
//! because the result round-trips through the parser unchanged.
//!
//! So the comparison stands with nothing carved out of it. A future divergence
//! gets a fix or a stated exemption; it does not get quietly excluded from the
//! tree.
//!
//! ## What it cannot see
//!
//! Anything the forward path and the undo path agree on. A writer that formats
//! something badly but *consistently* produces that formatting in `end`, again
//! in `redone`, and undoes cleanly back to `start` — three passing comparisons
//! and a wrong file. The blank line welded under a bare section header is
//! exactly that shape, and it is pinned in `track_serializer.rs` instead.
//!
//! Which is the boundary of the whole suite, worth stating plainly: this asks
//! whether undo is the inverse of what was done, not whether what was done was
//! right.

use std::fs;

use proptest::prelude::*;

use frame::tui::app::{App, Mode};

#[path = "support/tui_steps.rs"]
mod tui_steps;

use tui_steps::{apply_step, arb_step, fixture, flush_and_save, frame_tree, press_char, tree_diff};

/// Press undo until the stack is empty. The cap is a guard against an operation
/// that puts itself back — `SyncMarker` does exactly that by design.
fn undo_all(app: &mut App) -> usize {
    let mut count = 0;
    while app.undo_stack.peek_last_undo().is_some() && count < 512 {
        press_char(app, 'u');
        count += 1;
    }
    count
}

fn redo_all(app: &mut App) -> usize {
    let mut count = 0;
    while app.undo_stack.peek_last_redo().is_some() && count < 512 {
        press_char(app, 'Z');
        count += 1;
    }
    count
}

// ---------------------------------------------------------------------------
// The property
// ---------------------------------------------------------------------------

proptest! {
    #![proptest_config(ProptestConfig::with_cases(96))]

    /// P9: undoing everything restores the project, and redoing everything
    /// restores the result.
    #[test]
    fn p9_undo_all_restores_and_redo_all_reapplies(
        steps in prop::collection::vec(arb_step(), 1..9)
    ) {
        let tmp = fixture();
        let root = tmp.path();

        let start = frame_tree(root);

        let project = frame::io::project_io::load_project(root).expect("project loads");
        let mut app = App::new(project);

        for step in &steps {
            let before_tree = frame_tree(root);
            let before_depth = app.undo_stack.undo_depth();
            apply_step(&mut app, step);
            prop_assert!(
                app.mode == Mode::Navigate,
                "step {step:?} left the app in {:?}",
                app.mode
            );
            // The premise the rest of the property rests on: a keystroke that
            // wrote to the project recorded something to undo it with. Without
            // this, an operation that records nothing is *invisible* below —
            // undo-all skips it, and the tree it left behind is the tree
            // undo-all is compared against. `c` (toggle `#cc`) was exactly
            // that: it wrote the file and pushed nothing.
            prop_assert!(
                frame_tree(root) == before_tree
                    || app.undo_stack.undo_depth() > before_depth,
                "step {step:?} changed the project but recorded no undo entry"
            );
        }
        flush_and_save(&mut app);

        // Vacuity guard. A run that recorded nothing proves nothing: both
        // comparisons below would pass on a project no keystroke ever touched.
        //
        // Discarded rather than failed, because such a run is legitimate — `x`
        // on a task that is already done is a no-op, and so is a step whose
        // target the track view will not put a cursor on. What must not happen
        // is a run being *counted* as evidence when it is not, which is why the
        // per-step check above is an assertion and this one is not.
        prop_assume!(!app.undo_stack.is_empty());

        let end = frame_tree(root);

        undo_all(&mut app);
        // Undo cancels the pending move that goes with the operation it is
        // undoing, so nothing should be left waiting. Something left here would
        // execute later and re-dirty a project the user believes is restored.
        let pending = flush_and_save(&mut app);
        prop_assert!(
            pending.is_empty(),
            "undo left pending section moves in {pending:?}"
        );
        let undone = frame_tree(root);

        redo_all(&mut app);
        flush_and_save(&mut app);
        let redone = frame_tree(root);

        if let Some(diff) = tree_diff(&start, &undone) {
            return Err(TestCaseError::fail(format!(
                "undo-all did not restore the project.\n{diff}\nsteps: {steps:?}"
            )));
        }
        if let Some(diff) = tree_diff(&end, &redone) {
            return Err(TestCaseError::fail(format!(
                "redo-all did not restore the result.\n{diff}\nsteps: {steps:?}"
            )));
        }
    }
}

// ---------------------------------------------------------------------------
// Fixed cases
// ---------------------------------------------------------------------------

/// The precondition the whole suite rests on: the fixture is a fixpoint of the
/// parse/serialize pair, **and stays one once everything in it is dirty**.
///
/// The second half is the one that matters here and it is not the usual
/// settledness check. A clean task or inbox item is emitted from its
/// `source_text` verbatim, so a file can round-trip perfectly and still be
/// spelled differently from the way the serializer would spell it. The moment a
/// step touches such a record, it comes back canonical — and P9 would report
/// that as undo failing to restore the project, when nothing was lost at all.
/// Marking everything dirty asks the question P9 actually needs answered: is
/// this file already in the form the writer produces?
///
/// It is stated once here rather than in both suites that build this fixture:
/// `concurrency.rs` shares it through `tests/support/tui_steps.rs` and rests on
/// the same precondition for the same reason.
#[test]
fn the_fixture_is_settled() {
    let tmp = fixture();
    let frame = tmp.path().join("frame");
    for (path, text) in frame_tree(tmp.path()) {
        if !path.ends_with(".md") {
            continue;
        }
        let (clean, dirty) = if path == "inbox.md" {
            let (inbox, _) = frame::parse::parse_inbox(&text);
            let clean = frame::parse::serialize_inbox(&inbox);
            let (mut inbox, _) = frame::parse::parse_inbox(&text);
            for item in &mut inbox.items {
                item.dirty = true;
            }
            (clean, frame::parse::serialize_inbox(&inbox))
        } else {
            let track = frame::parse::parse_track(&text);
            let clean = frame::parse::serialize_track(&track);
            let mut track = frame::parse::parse_track(&text);
            for node in &mut track.nodes {
                if let frame::model::track::TrackNode::Section { tasks, .. } = node {
                    dirty_all(tasks);
                }
            }
            (clean, frame::parse::serialize_track(&track))
        };
        let path = frame.join(path);
        assert_eq!(clean, text, "{} is not settled", path.display());
        assert_eq!(dirty, text, "{} is not in canonical form", path.display());
    }

    // And `project.toml` under the config pair, which is the one every
    // track-level operation rewrites. Writing it back from the struct
    // materialises defaults the literal omits, so a fixture that skipped this
    // would report its own canonicalisation as an undo failure.
    let before = fs::read_to_string(frame.join("project.toml")).unwrap();
    let project = frame::io::project_io::load_project(tmp.path()).expect("project loads");
    frame::io::config_io::write_config_from_struct(&frame, &project.config).unwrap();
    let after = fs::read_to_string(frame.join("project.toml")).unwrap();
    assert_eq!(after, before, "project.toml is not settled");
}

/// Mark every task and subtask dirty, so the serializer emits its canonical
/// form rather than the source lines it was parsed from.
fn dirty_all(tasks: &mut [frame::model::task::Task]) {
    for task in tasks {
        task.mark_dirty();
        dirty_all(&mut task.subtasks);
    }
}
