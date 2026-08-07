//! Driving the TUI from a generated sequence of semantic steps.
//!
//! Shared by every property that needs a real `App` doing real things: P9
//! (`undo_properties.rs`) and P8 (`concurrency.rs`). Not a test target of its
//! own — it lives under `tests/support/` so cargo does not compile it as one,
//! and each suite pulls it in with
//!
//! ```ignore
//! #[path = "support/tui_steps.rs"]
//! mod tui_steps;
//! ```
//!
//! ## Why it is shared rather than copied
//!
//! Almost none of what is here is obvious, and all of it is the kind of thing
//! that rots silently in a second copy. [`apply_step`] resolves generated
//! indices against the *live* app, so a sequence stays valid however earlier
//! steps rearranged the project. [`typed`], [`moved`] and [`palette`] each
//! check the mode they were supposed to reach and [`bail`] out if they did not,
//! because in this harness a step that half-runs sends the *next* step's keys
//! somewhere nobody intended — `z` outside Navigate mode is undo, not a
//! character. A divergent second copy of that logic would not fail; it would
//! quietly test a different thing.
//!
//! ## The two invariants a consumer must know
//!
//! **The fixture is settled, and canonical.** A clean task is emitted from its
//! `source_text` verbatim, so a file can round-trip perfectly and still not be
//! spelled the way the serializer spells it — the first edit to touch it then
//! canonicalises, and any byte comparison reports that as loss. `fixture()`
//! writes files that are already in the writer's own form, and
//! `undo_properties::the_fixture_is_settled` is the pin that keeps them that
//! way. Both suites depend on it; it is stated once, there.
//!
//! **`frame_tree` excludes working-copy-local files** via
//! `LOCAL_ONLY_FRAME_FILES` — the lock, the UI state, the actor token, the id
//! frontier, the in-flight marker, the recovery log. They are not project
//! content. The frontier in particular must *not* be rewound by anything: an id
//! that was minted and then undone still must never be handed out again,
//! because a collaborator may already have seen it.

#![allow(dead_code)] // each consumer uses a different subset

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
pub fn create_fixture(root: &Path) {
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
pub fn fixture() -> tempfile::TempDir {
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

/// Every file under `frame/` that is project content, keyed by path relative to
/// `frame/`.
///
/// Working-copy-local files are excluded via `LOCAL_ONLY_FRAME_FILES` — see the
/// module docs for why, and for why the id frontier must never be rewound.
pub fn frame_tree(root: &Path) -> Vec<(String, String)> {
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
pub fn tree_diff(expected: &[(String, String)], actual: &[(String, String)]) -> Option<String> {
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

pub fn key(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
    KeyEvent::new(code, mods)
}

/// Press a printable character, with SHIFT set for uppercase the way a terminal
/// reports it.
pub fn press_char(app: &mut App, c: char) {
    let mods = if c.is_ascii_uppercase() {
        KeyModifiers::SHIFT
    } else {
        KeyModifiers::NONE
    };
    handle_key(app, key(KeyCode::Char(c), mods));
}

pub fn press(app: &mut App, code: KeyCode) {
    handle_key(app, key(code, KeyModifiers::NONE));
}

pub fn type_str(app: &mut App, s: &str) {
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
pub enum Surface {
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
pub enum ActionKind {
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
    pub fn surface(self) -> Surface {
        use ActionKind::*;
        match self {
            InboxAdd | InboxEditTitle | InboxDelete | InboxMoveDown | InboxTriage => Surface::Inbox,
            TrackAdd | TrackRename | TrackShelve | TrackMoveDown | TrackMoveUp | TrackCcFocus
            | TrackDelete | TrackArchive => Surface::Tracks,
            _ => Surface::Task,
        }
    }
}

pub const ACTIONS: &[ActionKind] = &[
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
pub struct Step {
    pub action: ActionKind,
    pub target: usize,
    pub text: u8,
}

pub fn arb_step() -> impl Strategy<Value = Step> {
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
pub fn live_task_ids(app: &App) -> Vec<String> {
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
pub fn apply_step(app: &mut App, step: &Step) -> bool {
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
            // Against the list the cursor actually indexes, not `config.tracks`.
            // A row whose state is none of active/shelved/archived is not in the
            // tracks view at all, so taking the modulo over the config would let
            // the cursor point past the last row there is.
            let count = app.tracks_view_order().len();
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
pub fn typed(app: &mut App, trigger: char, text: &str) -> bool {
    press_char(app, trigger);
    if app.mode != Mode::Edit {
        return bail(app);
    }
    type_str(app, text);
    press(app, KeyCode::Enter);
    app.mode == Mode::Navigate || bail(app)
}

/// Enter move mode, step once, and commit. Same precondition, same reason.
pub fn moved(app: &mut App, direction: char) -> bool {
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
pub fn palette(app: &mut App, label: &str, expect: &str) -> bool {
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
pub fn bail(app: &mut App) -> bool {
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
pub fn flush_and_save(app: &mut App) -> Vec<String> {
    let flushed = app.flush_all_pending_moves();
    for track_id in &flushed {
        app.save_track_logged(track_id);
    }
    flushed
}
