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
//! navigating and occasionally wander into edit mode to type garbage. A [`Step`]
//! is a (which task, which action) pair; the generator emits indices and the
//! runner resolves them against the live app, so a step always names a task that
//! exists at the moment it runs.
//!
//! **The comparison is the on-disk tree.** Model equality is meaningless here:
//! `source_text` means a clean task serialises verbatim while a touched one
//! serialises canonically, so `Task`-level equality would be trivially true or
//! trivially false depending on `dirty`. [`frame_tree`] is every file under
//! `frame/` minus `LOCAL_ONLY_FRAME_FILES` — fourth consumer of that constant,
//! after `fr init`, `fr check` and `parity.rs`.
//!
//! The fixture must be **settled** (`serialize(parse(x)) == x`), or the first
//! `mark_dirty` canonicalises a task and every diff is noise;
//! [`the_fixture_is_settled`] pins that.
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
use std::path::Path;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use proptest::prelude::*;

use frame::tui::app::{App, Mode, View};
use frame::tui::input::handle_key;

// ---------------------------------------------------------------------------
// Fixture
// ---------------------------------------------------------------------------

/// A small settled project: two active tracks, every section populated, one
/// subtask tree, and an inbox.
///
/// Deliberately smaller than `parity.rs`'s fixture. That one exists so no read
/// command's filter is vacuous; this one exists so a generated sequence reaches
/// every section of a track within a handful of steps, and a smaller tree makes
/// a byte diff readable.
fn create_fixture(root: &Path) {
    let frame = root.join("frame");
    fs::create_dir_all(frame.join("tracks")).unwrap();

    // Pin to the primary (null) actor, as `fr init` does, so minted ids stay in
    // the legacy namespace and a failing case is reproducible.
    fs::write(frame.join(".actor"), "null\n").unwrap();

    fs::write(
        frame.join("project.toml"),
        "\
[project]
name = \"undo-fixture\"

[agent]
cc_focus = \"main\"

[[tracks]]
id = \"main\"
name = \"Main Track\"
state = \"active\"
file = \"tracks/main.md\"

[[tracks]]
id = \"side\"
name = \"Side Track\"
state = \"active\"
file = \"tracks/side.md\"

[ids.prefixes]
main = \"M\"
side = \"S\"
",
    )
    .unwrap();

    fs::write(
        frame.join("tracks/main.md"),
        "\
# Main Track

## Backlog

- [ ] `M-001` First task #core
  - added: 2025-05-01
- [>] `M-002` Second task
  - added: 2025-05-02
- [ ] `M-003` Task with subtasks
  - added: 2025-05-03
  - [ ] `M-003.1` Sub one
    - added: 2025-05-03
  - [>] `M-003.2` Sub two
    - added: 2025-05-03

## Parked

- [~] `M-010` Parked idea
  - added: 2025-04-15

## Done

- [x] `M-000` Setup project
  - added: 2025-04-20
  - resolved: 2025-04-25
",
    )
    .unwrap();

    // `## Parked` is empty *and* bare — no blank under its header, because it
    // has nothing to separate. Every section in `main.md` carries its blank, so
    // without this one the suite would never drive an add, a move or an undo
    // into a section shaped this way.
    //
    // It does not pin the separator rule itself, and the reason is worth
    // knowing: a task welded to `## Parked` is welded in `end` *and* in
    // `redone`, and undoing the move empties the section back to its bare
    // header, so both comparisons pass. P9 sees where undo and the forward path
    // disagree, not where they agree on something wrong. The weld is pinned in
    // `track_serializer.rs`, where it can be stated directly.
    fs::write(
        frame.join("tracks/side.md"),
        "\
# Side Track

## Backlog

- [ ] `S-001` Side task one
  - added: 2025-05-01

## Parked

## Done

- [x] `S-000` Side done
  - added: 2025-04-01
  - resolved: 2025-04-02
",
    )
    .unwrap();

    // Items are spelled the way the serializer spells them — tags inline, a
    // blank line between items. `- Bug in parser` with `  #bug` under it is
    // equally legal and round-trips while the item is clean, but the first edit
    // marks the item dirty and it comes back canonical. That is the
    // `source_text` model working as designed, and a fixture that started
    // non-canonical would report it as an undo failure on every inbox step.
    // `the_fixture_is_settled` checks for exactly that.
    fs::write(
        frame.join("inbox.md"),
        "\
# Inbox

- Bug in parser #bug

- Think about design
",
    )
    .unwrap();
}

/// Build the fixture in a fresh temp dir and hand back the root.
///
/// The actor registry is materialised here rather than written as a literal:
/// its row carries the machine's hostname and the date it was claimed, so the
/// only version of it that is correct on every machine is the one the code
/// writes. Doing it before the snapshot also keeps a first mint from creating
/// `actors.toml` mid-run, which undo would then be blamed for not deleting —
/// correctly, since the registry is shared state that no undo should rewind.
fn fixture() -> tempfile::TempDir {
    let tmp = tempfile::TempDir::new().unwrap();
    create_fixture(tmp.path());
    let frame_dir = tmp.path().join("frame");
    frame::io::actors::resolve_actor_token(&frame_dir).expect("claim actor token");
    // Settle `project.toml` the same way, and for the same reason the track
    // files are written settled: writing the config back from the struct
    // materialises every default the literal above leaves out, so the first
    // config write of a run would otherwise show up as a diff undo is blamed
    // for. Pinned by `the_fixture_is_settled`.
    let project = frame::io::project_io::load_project(tmp.path()).expect("project loads");
    frame::io::config_io::write_config_from_struct(&frame_dir, &project.config)
        .expect("write config");
    tmp
}

// ---------------------------------------------------------------------------
// The tree under comparison
// ---------------------------------------------------------------------------

/// Every file under `frame/` that undo is expected to restore, keyed by path
/// relative to `frame/`.
///
/// Working-copy-local files are excluded via `LOCAL_ONLY_FRAME_FILES`: the lock,
/// the UI state, the actor token, the id frontier and the in-flight marker are
/// not project content, and undo is not expected to rewind them. The id frontier
/// in particular *must* not rewind — an id that was minted and then undone still
/// must never be handed out again, because a collaborator may already have seen
/// it.
fn frame_tree(root: &Path) -> Vec<(String, String)> {
    let frame = root.join("frame");
    let mut out = Vec::new();
    let mut stack = vec![frame.clone()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let name = path.file_name().unwrap().to_string_lossy().to_string();
            if frame::io::project_io::LOCAL_ONLY_FRAME_FILES.contains(&name.as_str()) {
                continue;
            }
            let rel = path
                .strip_prefix(&frame)
                .unwrap()
                .to_string_lossy()
                .to_string();
            out.push((rel, fs::read_to_string(&path).unwrap_or_default()));
        }
    }
    out.sort();
    out
}

/// Describe the first way two trees differ, or `None` if they are identical.
///
/// `prop_assert_eq!` on the whole tree prints both copies of every file,
/// including the ones that match; this prints the one file that does not, with
/// the differing lines marked. What a failure needs is the smallest true
/// statement about what moved.
fn tree_diff(expected: &[(String, String)], actual: &[(String, String)]) -> Option<String> {
    for (path, want) in expected {
        match actual.iter().find(|(p, _)| p == path) {
            None => return Some(format!("{path} is missing")),
            Some((_, got)) if got != want => {
                let mut lines = vec![format!("{path} differs:")];
                let want_lines: Vec<&str> = want.lines().collect();
                let got_lines: Vec<&str> = got.lines().collect();
                for i in 0..want_lines.len().max(got_lines.len()) {
                    let w = want_lines.get(i).copied();
                    let g = got_lines.get(i).copied();
                    if w == g {
                        continue;
                    }
                    lines.push(format!("  line {}: want {w:?}", i + 1));
                    lines.push(format!("           got  {g:?}"));
                }
                return Some(lines.join("\n"));
            }
            Some(_) => {}
        }
    }
    for (path, _) in actual {
        if !expected.iter().any(|(p, _)| p == path) {
            return Some(format!("{path} is unexpected"));
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Keystrokes
// ---------------------------------------------------------------------------

fn key(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
    KeyEvent::new(code, mods)
}

/// Press a printable character, with SHIFT set for uppercase the way a terminal
/// reports it.
fn press_char(app: &mut App, c: char) {
    let mods = if c.is_ascii_uppercase() {
        KeyModifiers::SHIFT
    } else {
        KeyModifiers::NONE
    };
    handle_key(app, key(KeyCode::Char(c), mods));
}

fn press(app: &mut App, code: KeyCode) {
    handle_key(app, key(code, KeyModifiers::NONE));
}

fn type_str(app: &mut App, s: &str) {
    for c in s.chars() {
        press_char(app, c);
    }
}

// ---------------------------------------------------------------------------
// Steps
// ---------------------------------------------------------------------------

/// Which view a step drives, and therefore what its `target` indexes.
///
/// The view is set directly rather than navigated to, the way `parity.rs` sets
/// its `Start`: getting to a task is not the behaviour under test, and driving
/// `Tab` around the views would spend the sequence on navigation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Surface {
    /// Track view, cursor on a task.
    Task,
    /// Inbox view, cursor on an item.
    Inbox,
    /// Tracks view, cursor on a track.
    Tracks,
}

/// What a generated step does. Every variant is a real key sequence a user can
/// press, and every one is expected to record an undo operation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ActionKind {
    /// `x` — mark done. Schedules a section move behind the grace period.
    SetDone,
    /// `o` — set todo.
    SetTodo,
    /// `b` — toggle blocked.
    ToggleBlocked,
    /// `~` — toggle parked.
    ToggleParked,
    /// `Space` — cycle todo → active → done → todo.
    CycleState,
    /// `c` — toggle the `#cc` tag.
    ToggleCc,
    /// `e` — edit the title, appending to what is there.
    EditTitle,
    /// `a` — add a task at the bottom of the backlog.
    AddTask,
    /// `A` — add a subtask under the cursor.
    AddSubtask,
    /// `-` — insert a sibling after the cursor.
    InsertAfter,
    /// `m j Enter` — move the task one position down.
    MoveDown,
    /// `m k Enter` — move the task one position up.
    MoveUp,
    /// The palette's "Delete task", then `y`.
    DeleteTask,
    /// `m l Enter` — indent the task under its previous sibling. Rekeys ids.
    Indent,
    /// `m h Enter` — outdent the task to its parent's level. Rekeys ids.
    Outdent,
    /// `M Enter b` — move the task to the other track, at the bottom. The
    /// fixture has exactly two active tracks, so the candidate list has one
    /// entry and `Enter` takes it.
    CrossTrackMove,

    /// Inbox: `a` — add an item at the bottom.
    InboxAdd,
    /// Inbox: `e` — edit an item's title.
    InboxEditTitle,
    /// Inbox: `x y` — delete an item.
    InboxDelete,
    /// Inbox: `m j Enter` — reorder an item.
    InboxMoveDown,
    /// Inbox: `Enter Enter b` — triage the item into a track.
    InboxTriage,

    /// Tracks: `a` — add a track.
    TrackAdd,
    /// Tracks: `e` — rename a track. Rewrites `project.toml` and the file header.
    TrackRename,
    /// Tracks: `s` — shelve or reactivate.
    TrackShelve,
    /// Tracks: `m j Enter` — reorder a track in `project.toml`.
    TrackMoveDown,
    /// Tracks: `m k Enter` — reorder a track the other way.
    TrackMoveUp,
    /// Tracks: `C` — set the cc-focus track.
    TrackCcFocus,
    /// Tracks: the palette's "Delete track", then `y`. The F3 path: the track
    /// file is removed from disk and undo has to write its bytes back.
    TrackDelete,
    /// Tracks: the palette's "Archive track", then `y`. Moves the file into
    /// `archive/_tracks/`.
    TrackArchive,
}

impl ActionKind {
    fn surface(self) -> Surface {
        use ActionKind::*;
        match self {
            InboxAdd | InboxEditTitle | InboxDelete | InboxMoveDown | InboxTriage => Surface::Inbox,
            TrackAdd | TrackRename | TrackShelve | TrackMoveDown | TrackMoveUp | TrackCcFocus
            | TrackDelete | TrackArchive => Surface::Tracks,
            _ => Surface::Task,
        }
    }
}

const ACTIONS: &[ActionKind] = &[
    ActionKind::SetDone,
    ActionKind::SetTodo,
    ActionKind::ToggleBlocked,
    ActionKind::ToggleParked,
    ActionKind::CycleState,
    ActionKind::ToggleCc,
    ActionKind::EditTitle,
    ActionKind::AddTask,
    ActionKind::AddSubtask,
    ActionKind::InsertAfter,
    ActionKind::MoveDown,
    ActionKind::MoveUp,
    ActionKind::DeleteTask,
    ActionKind::Indent,
    ActionKind::Outdent,
    ActionKind::CrossTrackMove,
    ActionKind::InboxAdd,
    ActionKind::InboxEditTitle,
    ActionKind::InboxDelete,
    ActionKind::InboxMoveDown,
    ActionKind::InboxTriage,
    ActionKind::TrackAdd,
    ActionKind::TrackRename,
    ActionKind::TrackShelve,
    ActionKind::TrackMoveDown,
    ActionKind::TrackMoveUp,
    ActionKind::TrackCcFocus,
    ActionKind::TrackDelete,
    ActionKind::TrackArchive,
];

/// One generated step. `target` and `text` are indices resolved against the live
/// app when the step runs, so a generated sequence stays valid however the
/// earlier steps rearranged the project.
#[derive(Clone, Copy, Debug)]
struct Step {
    action: ActionKind,
    target: usize,
    text: u8,
}

fn arb_step() -> impl Strategy<Value = Step> {
    (0..ACTIONS.len(), 0usize..64, 0u8..26).prop_map(|(a, target, text)| Step {
        action: ACTIONS[a],
        target,
        text,
    })
}

/// Every task id currently in the project, in document order.
///
/// Document order rather than sorted: it is the order the user sees, so a
/// shrunk failing case names the task in the position the reader will look for.
fn live_task_ids(app: &App) -> Vec<String> {
    fn walk(tasks: &[frame::model::task::Task], out: &mut Vec<String>) {
        for task in tasks {
            if let Some(id) = &task.id {
                out.push(id.to_string());
            }
            walk(&task.subtasks, out);
        }
    }
    use frame::model::SectionKind;
    let mut out = Vec::new();
    for (_, track) in &app.project.tracks {
        for section in [SectionKind::Backlog, SectionKind::Parked, SectionKind::Done] {
            walk(track.section_tasks(section), &mut out);
        }
    }
    out
}

/// Run one step. Returns false if its precondition did not hold — no task to
/// aim at, or a task the track view will not put a cursor on (a Done task is
/// not listed there) — in which case nothing was pressed.
fn apply_step(app: &mut App, step: &Step) -> bool {
    match step.action.surface() {
        Surface::Task => {
            let targets = live_task_ids(app);
            if targets.is_empty() {
                return false;
            }
            let target = targets[step.target % targets.len()].clone();
            if !app.jump_to_task(&target) {
                return false;
            }
        }
        Surface::Inbox => {
            let count = app.project.inbox.as_ref().map_or(0, |i| i.items.len());
            if count == 0 {
                return false;
            }
            app.view = View::Inbox;
            app.inbox_cursor = step.target % count;
        }
        Surface::Tracks => {
            let count = app.project.config.tracks.len();
            if count == 0 {
                return false;
            }
            app.view = View::Tracks;
            app.tracks_cursor = step.target % count;
        }
    }
    let text = format!("{}{}", (b'a' + step.text % 26) as char, step.text % 10);

    match step.action {
        ActionKind::SetDone => press_char(app, 'x'),
        ActionKind::SetTodo => press_char(app, 'o'),
        ActionKind::ToggleBlocked => press_char(app, 'b'),
        ActionKind::ToggleParked => press_char(app, '~'),
        ActionKind::CycleState => press_char(app, ' '),
        ActionKind::ToggleCc => press_char(app, 'c'),
        ActionKind::EditTitle => return typed(app, 'e', &text),
        ActionKind::AddTask => return typed(app, 'a', &text),
        ActionKind::AddSubtask => return typed(app, 'A', &text),
        ActionKind::InsertAfter => return typed(app, '-', &text),
        ActionKind::MoveDown => return moved(app, 'j'),
        ActionKind::MoveUp => return moved(app, 'k'),
        ActionKind::DeleteTask => return palette(app, "delete task", "Delete "),
        ActionKind::Indent => return moved(app, 'l'),
        ActionKind::Outdent => return moved(app, 'h'),
        ActionKind::CrossTrackMove => {
            press_char(app, 'M');
            if app.mode != Mode::Triage {
                return false;
            }
            press(app, KeyCode::Enter);
            press_char(app, 'b');
        }

        ActionKind::InboxAdd => return typed(app, 'a', &text),
        ActionKind::InboxEditTitle => return typed(app, 'e', &text),
        ActionKind::InboxDelete => {
            press_char(app, 'x');
            if app.mode != Mode::Confirm {
                return false;
            }
            press_char(app, 'y');
        }
        ActionKind::InboxMoveDown => return moved(app, 'j'),
        ActionKind::InboxTriage => {
            press(app, KeyCode::Enter);
            if app.mode != Mode::Triage {
                return false;
            }
            press(app, KeyCode::Enter);
            press_char(app, 'b');
        }

        ActionKind::TrackAdd => return typed(app, 'a', &text),
        ActionKind::TrackRename => return typed(app, 'e', &text),
        ActionKind::TrackShelve => press_char(app, 's'),
        ActionKind::TrackMoveDown => return moved(app, 'j'),
        ActionKind::TrackMoveUp => return moved(app, 'k'),
        ActionKind::TrackCcFocus => press_char(app, 'C'),
        ActionKind::TrackDelete => return palette(app, "delete track", "Delete "),
        ActionKind::TrackArchive => return palette(app, "archive track", "Archive "),
    }
    if app.mode != Mode::Navigate {
        press(app, KeyCode::Esc);
        return false;
    }
    true
}

/// Press a key that should open the inline editor, type a title, and commit.
///
/// The mode check is not defensive padding, it is the precondition. `-` on a
/// task outside the Backlog has nowhere to insert and does nothing at all, and
/// without this the rest of the sequence lands in Navigate mode — where the
/// harness's own `z` means *undo*. A step that silently half-runs is worse than
/// one that does not run: it makes the suite report on a sequence nobody
/// generated.
fn typed(app: &mut App, trigger: char, text: &str) -> bool {
    press_char(app, trigger);
    if app.mode != Mode::Edit {
        return bail(app);
    }
    type_str(app, text);
    press(app, KeyCode::Enter);
    app.mode == Mode::Navigate || bail(app)
}

/// Enter move mode, step once, and commit. Same precondition, same reason.
fn moved(app: &mut App, direction: char) -> bool {
    press_char(app, 'm');
    if app.mode != Mode::Move {
        return bail(app);
    }
    press_char(app, direction);
    press(app, KeyCode::Enter);
    app.mode == Mode::Navigate || bail(app)
}

/// Run a command-palette action and confirm it.
///
/// `label` is typed into the palette and `expect` is checked against the
/// confirmation that comes back, so a palette whose ranking shifts makes this
/// step do nothing rather than silently perform some other command.
fn palette(app: &mut App, label: &str, expect: &str) -> bool {
    press_char(app, '>');
    if app.mode != Mode::Command {
        return bail(app);
    }
    type_str(app, label);
    press(app, KeyCode::Enter);
    let matched = app.mode == Mode::Confirm
        && app
            .confirm_state
            .as_ref()
            .is_some_and(|c| c.message.starts_with(expect));
    if !matched {
        return bail(app);
    }
    press_char(app, 'y');
    app.mode == Mode::Navigate || bail(app)
}

/// Abandon a step that could not run, leaving the app back in Navigate mode.
///
/// Always returns false, so callers read as `return bail(app)`. Leaving a popup
/// open would send the *next* step's keystrokes somewhere nobody intended —
/// and in this harness `z` outside Navigate mode is not a character, it is undo.
fn bail(app: &mut App) -> bool {
    for _ in 0..3 {
        if app.mode == Mode::Navigate {
            break;
        }
        press(app, KeyCode::Esc);
    }
    false
}

/// Flush the grace-period section moves and save what they touched — what
/// `app.rs` does on a view change and on quit.
fn flush_and_save(app: &mut App) -> Vec<String> {
    let flushed = app.flush_all_pending_moves();
    for track_id in &flushed {
        app.save_track_logged(track_id);
    }
    flushed
}

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
