//! Parity tests — assert that paired code paths agree.
//!
//! Three defects in twelve commits came from the same shape: two code paths
//! that answer the same question, written separately, drifting apart.
//! `b664a3e` (human and `--json` disagreed on `--state done`), `e1a8dbe` plus
//! `83be43a` (the same "Parked/Done are movable" fix applied to the CLI, then
//! separately to the TUI), `7071675` (the TUI hid shelved tracks while the CLI
//! still wrote to them). Each shipped because nothing asserted the pair agreed.
//!
//! This file is the standing assertion. It has two parts.
//!
//! **The matrix** ([`ROWS`]) runs every read command under every filter twice —
//! once human, once `--json` — and compares the *ordered sequence of
//! identifiers* each surface names. Sequence rather than set: strictly
//! stronger, and both surfaces are ordered listings, so ordering drift is
//! caught for free. Identifiers rather than full structural equality: parsing
//! the human output back into `TaskJson` would be a second implementation of
//! the formatter, brittle and worth less than it costs. Identifier sequence is
//! precisely what broke in `b664a3e` — one surface named a task the other
//! didn't — and it is immune to cosmetic changes on either side.
//!
//! **The classification guard** ([`CLASSIFICATION`],
//! [`every_subcommand_is_classified`]) is what makes this a standing category
//! rather than a snapshot. It enumerates clap's own subcommand list and
//! requires every entry to be classified: covered by the matrix, or exempted
//! with a stated reason. A new subcommand fails the build until someone
//! decides which it is. Same move as `LOCAL_ONLY_FRAME_FILES` — one list, two
//! consumers — applied to commands, with clap's list as the source that cannot
//! drift from reality.
//!
//! The matrix's own failure mode is vacuity: if both surfaces return nothing,
//! every row passes. So each row declares whether it expects output, and a row
//! that extracts no identifiers from *either* surface without saying so is a
//! failure, not a pass.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

// ---------------------------------------------------------------------------
// Fixture
// ---------------------------------------------------------------------------

/// Every task id the fixture contains. The extractor recognises identifiers by
/// matching against this universe rather than by regex, so a stray token in an
/// error message can never be mistaken for a listed task.
const TASK_IDS: &[&str] = &[
    "M-000", "M-001", "M-002", "M-003", "M-003.1", "M-003.2", "M-004", "M-005", "M-010", "S-000",
    "S-001", "S-002", "S-010", "H-001", "H-002", "H-003", "H-004", "H-005", "H-006", "H-007",
    // Archived, so it appears only in `fr search`'s `[archive:…]` block.
    "M-900",
    // Not a task. A dangling `dep:` target, which `fr deps` names in both
    // surfaces and the extractor therefore has to recognise.
    "H-999",
];

const TRACK_IDS: &[&str] = &["main", "side", "shelf"];

const TRACK_NAMES: &[&str] = &["Main Track", "Side Track", "Shelf Track"];

const INBOX_TITLES: &[&str] = &["Bug in parser", "Think about design", "Quick note"];

/// A fixture built so that no row is vacuous: every filter in [`ROWS`] selects
/// something. Deliberately awkward in three places — `S-002` is state Parked
/// while sitting in the *Backlog* section, `shelf` is a shelved track with live
/// work in it, and the shelf track carries the whole dependency zoo (a diamond,
/// a cycle, a dangling target) — because these are states a real project reaches
/// and where a human/JSON pair is most likely to disagree.
///
/// The dependency structure lives in `shelf` on purpose: a shelved track is
/// excluded from `ready`, `blocked` and the default `list`, so adding deps there
/// gives `fr deps` something to traverse without perturbing every other row.
fn create_fixture(root: &Path) {
    let frame = root.join("frame");
    fs::create_dir_all(frame.join("tracks")).unwrap();

    // Pin this working copy to the primary (null) actor, as `fr init` does, so
    // ids stay in the legacy namespace and the fixture is stable.
    fs::write(frame.join(".actor"), "null\n").unwrap();

    fs::write(
        frame.join("project.toml"),
        r#"[project]
name = "parity-fixture"

[agent]
cc_focus = "main"

[[tracks]]
id = "main"
name = "Main Track"
state = "active"
file = "tracks/main.md"

[[tracks]]
id = "side"
name = "Side Track"
state = "active"
file = "tracks/side.md"

[[tracks]]
id = "shelf"
name = "Shelf Track"
state = "shelved"
file = "tracks/shelf.md"

[ids.prefixes]
main = "M"
side = "S"
shelf = "H"
"#,
    )
    .unwrap();

    fs::write(
        frame.join("tracks/main.md"),
        "\
# Main Track

> The main work stream.

## Backlog

- [ ] `M-001` First task #core
  - added: 2025-05-01
- [>] `M-002` Second task #core #cc
  - added: 2025-05-02
  - dep: M-001
- [-] `M-004` Blocked task #cc
  - added: 2025-05-04
  - dep: M-001
- [ ] `M-003` Third task with subtasks #core
  - added: 2025-05-03
  - [ ] `M-003.1` Sub one #cc
    - added: 2025-05-03
  - [>] `M-003.2` Sub two
    - added: 2025-05-03

## Parked

- [~] `M-010` Parked idea #core
  - added: 2025-04-15

## Done

- [x] `M-000` Setup project #core
  - added: 2025-04-20
  - resolved: 2025-04-25
- [x] `M-005` Second done thing
  - added: 2025-04-21
  - resolved: 2025-04-26
",
    )
    .unwrap();

    fs::write(
        frame.join("tracks/side.md"),
        "\
# Side Track

## Backlog

- [ ] `S-001` Side task one #cc
  - added: 2025-05-01
- [~] `S-002` Side task two
  - added: 2025-05-02

## Parked

- [~] `S-010` Side parked
  - added: 2025-04-11

## Done

- [x] `S-000` Side done
  - added: 2025-04-01
  - resolved: 2025-04-02
",
    )
    .unwrap();

    fs::write(
        frame.join("tracks/shelf.md"),
        "\
# Shelf Track

## Backlog

- [ ] `H-001` Shelved task #core
  - added: 2025-03-01
  - dep: H-002, H-003
- [ ] `H-002` Diamond left
  - added: 2025-03-02
  - dep: H-004
- [ ] `H-003` Diamond right
  - added: 2025-03-03
  - dep: H-004
- [ ] `H-004` Shared leaf
  - added: 2025-03-04
- [ ] `H-005` Cycle a
  - added: 2025-03-05
  - dep: H-006
- [ ] `H-006` Cycle b
  - added: 2025-03-06
  - dep: H-005
- [ ] `H-007` Dangling
  - added: 2025-03-07
  - dep: H-999

## Done
",
    )
    .unwrap();

    // An archive, so `fr search`'s archived block is not permanently empty —
    // without it the live/archive ordering would go untested and any row
    // covering it would pass vacuously.
    fs::create_dir_all(frame.join("archive")).unwrap();
    fs::write(
        frame.join("archive/main.md"),
        "\
# Main Track — Archive

## Done

- [x] `M-900` Archived First task
  - added: 2024-01-01
  - resolved: 2024-01-02
",
    )
    .unwrap();

    fs::write(
        frame.join("inbox.md"),
        "\
# Inbox

- Bug in parser #bug
  Stack trace points to line 142.

- Think about design #design

- Quick note
",
    )
    .unwrap();
}

// ---------------------------------------------------------------------------
// Running `fr`
// ---------------------------------------------------------------------------

fn fr_bin() -> PathBuf {
    let mut path = std::env::current_exe().unwrap();
    path.pop(); // test binary name
    path.pop(); // deps/
    path.push("fr");
    path
}

fn run_fr(dir: &Path, args: &[&str]) -> String {
    let output = Command::new(fr_bin())
        .args(args)
        .current_dir(dir)
        // Isolate from the real global registry (~/.config/frame/projects.toml).
        .env("XDG_CONFIG_HOME", dir.join(".xdg-config"))
        .output()
        .expect("failed to run fr");
    assert!(
        output.status.success(),
        "fr {:?} failed:\n{}",
        args,
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).to_string()
}

// ---------------------------------------------------------------------------
// Extracting the identifier sequence from each surface
// ---------------------------------------------------------------------------

/// Whether `c` may sit against an identifier without being part of it.
fn is_boundary(c: u8) -> bool {
    !(c.is_ascii_alphanumeric() || c == b'-' || c == b'.' || c == b'_')
}

/// The earliest whole-token occurrence of any universe member in `line`. Ties
/// at the same position break toward the longer match, so `M-003` never
/// shadows `M-003.1`.
fn first_identifier(line: &str, universe: &[&str]) -> Option<String> {
    let bytes = line.as_bytes();
    let mut best: Option<(usize, &str)> = None;

    for cand in universe {
        let mut from = 0;
        while let Some(rel) = line[from..].find(cand) {
            let at = from + rel;
            let end = at + cand.len();
            let before_ok = at == 0 || is_boundary(bytes[at - 1]);
            let after_ok = end >= bytes.len() || is_boundary(bytes[end]);
            if before_ok && after_ok {
                let better = match best {
                    None => true,
                    Some((p, b)) => at < p || (at == p && cand.len() > b.len()),
                };
                if better {
                    best = Some((at, cand));
                }
                break;
            }
            from = at + 1;
        }
    }

    best.map(|(_, c)| c.to_string())
}

/// The identifiers a human listing names, in output order: one per line, the
/// first that appears.
///
/// Metadata lines are skipped. `dep: M-001` and `ref: …` name tasks the listing
/// is not itself listing — `fr blocked` prints `(blocked by: M-001)` on the
/// same line as the blocked task, and `fr show` prints a `dep:` line of its
/// own. Taking only the first identifier per line handles the former; skipping
/// lines whose leading token ends in `:` handles the latter.
fn human_sequence(stdout: &str, universe: &[&str]) -> Vec<String> {
    stdout
        .lines()
        .filter(|line| {
            !line
                .split_whitespace()
                .next()
                .is_some_and(|tok| tok.ends_with(':'))
        })
        .filter_map(|line| first_identifier(line, universe))
        .collect()
}

/// How to read the identifier sequence out of a `--json` payload.
#[derive(Clone, Copy, Debug)]
enum Projection {
    /// Every task id in the payload, parents before their subtasks — the order
    /// the human tree prints them. For `fr list`, whose human surface renders
    /// the whole tree.
    TaskTree,
    /// Only the ids of the top-level entries, not their nested subtasks.
    ///
    /// `ready`, `blocked` and `recent` return whole task objects — subtree and
    /// all — but the *listing* is the array, and the human surface prints one
    /// line per entry. A consumer flattening the nested subtasks would not be
    /// reading the listing.
    ListEntries,
    /// A named field of each object, recursing through container keys.
    Field(&'static str),
    /// `fr deps`: the root id then its dependency tree, pre-order — the order
    /// the human tree prints. Recurses only through `deps`, which on this
    /// command holds objects rather than the bare id strings `TaskJson.deps`
    /// carries.
    DepTree,
    /// `fr search`: task ids from `tasks` then `archived`, which is the order
    /// the human surface prints them. Not `inbox`, whose entries have no id.
    SearchIds,
    /// `fr search`: inbox titles. The inbox block is printed last, so a row
    /// using this must not also expect task hits.
    SearchTitles,
    /// `fr show ID --context`: ancestors outermost-first, then the task, then
    /// its subtasks — the order the human surface prints them. The JSON carries
    /// ancestors in a trailing `ancestors` field; that is a schema detail, not
    /// a difference in what is shown.
    ShowWithContext,
    /// `fr show ID` without `--context`: the task and its subtasks only.
    ///
    /// Declared divergence, deliberate: the JSON *always* populates `ancestors`
    /// (see the `// JSON always includes ancestors` comment in
    /// `cli/handlers/mod.rs`) while the human surface omits them unless asked.
    /// Excluded here so the row asserts the rest of the pair still agrees.
    ShowTaskOnly,
}

/// Collect `field` from every object, descending through the keys that hold
/// nested payloads.
fn collect_field(v: &Value, field: &str, out: &mut Vec<String>) {
    match v {
        Value::Array(items) => {
            for item in items {
                collect_field(item, field, out);
            }
        }
        Value::Object(obj) => {
            if let Some(Value::String(s)) = obj.get(field) {
                out.push(s.clone());
            }
            for key in ["tasks", "subtasks", "tracks"] {
                if let Some(nested) = obj.get(key) {
                    collect_field(nested, field, out);
                }
            }
        }
        _ => {}
    }
}

/// Collect ids from an object and its `subtasks`, but not from sibling
/// containers.
fn collect_subtree(v: &Value, out: &mut Vec<String>) {
    if let Some(id) = v.get("id").and_then(Value::as_str) {
        out.push(id.to_string());
    }
    if let Some(Value::Array(subs)) = v.get("subtasks") {
        for sub in subs {
            collect_subtree(sub, out);
        }
    }
}

/// Collect ids from a dependency tree, parent before its dependencies.
fn collect_dep_tree(v: &Value, out: &mut Vec<String>) {
    if let Some(id) = v.get("id").and_then(Value::as_str) {
        out.push(id.to_string());
    }
    if let Some(Value::Array(deps)) = v.get("deps") {
        for dep in deps {
            collect_dep_tree(dep, out);
        }
    }
}

fn json_sequence(v: &Value, projection: Projection) -> Vec<String> {
    let mut out = Vec::new();
    match projection {
        Projection::TaskTree => collect_field(v, "id", &mut out),
        Projection::Field(name) => collect_field(v, name, &mut out),
        Projection::ListEntries => {
            let entries = match v {
                Value::Array(items) => items.clone(),
                Value::Object(obj) => obj
                    .get("tasks")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default(),
                _ => Vec::new(),
            };
            for entry in &entries {
                if let Some(id) = entry.get("id").and_then(Value::as_str) {
                    out.push(id.to_string());
                }
            }
        }
        Projection::ShowWithContext => {
            if let Some(Value::Array(ancestors)) = v.get("ancestors") {
                for a in ancestors {
                    out.push(a["id"].as_str().unwrap_or_default().to_string());
                }
            }
            collect_subtree(v, &mut out);
        }
        Projection::DepTree => collect_dep_tree(v, &mut out),
        Projection::SearchIds => {
            for key in ["tasks", "archived"] {
                if let Some(Value::Array(items)) = v.get(key) {
                    for item in items {
                        if let Some(id) = item.get("id").and_then(Value::as_str) {
                            out.push(id.to_string());
                        }
                    }
                }
            }
        }
        Projection::SearchTitles => {
            if let Some(Value::Array(items)) = v.get("inbox") {
                for item in items {
                    if let Some(title) = item.get("title").and_then(Value::as_str) {
                        out.push(title.to_string());
                    }
                }
            }
        }
        Projection::ShowTaskOnly => collect_subtree(v, &mut out),
    }
    out
}

// ---------------------------------------------------------------------------
// The matrix
// ---------------------------------------------------------------------------

struct Row {
    args: &'static [&'static str],
    universe: &'static [&'static str],
    projection: Projection,
    /// Rows are expected to name something. A row that extracts nothing from
    /// either surface is a fixture that stopped exercising the filter, not a
    /// pass — see the module docs on vacuity.
    expect_empty: bool,
}

const fn row(
    args: &'static [&'static str],
    universe: &'static [&'static str],
    p: Projection,
) -> Row {
    Row {
        args,
        universe,
        projection: p,
        expect_empty: false,
    }
}

const ROWS: &[Row] = &[
    // -- fr list: every filter, and the combinations that cross them ---------
    row(&["list"], TASK_IDS, Projection::TaskTree),
    row(&["list", "--all"], TASK_IDS, Projection::TaskTree),
    row(&["list", "main"], TASK_IDS, Projection::TaskTree),
    row(&["list", "side"], TASK_IDS, Projection::TaskTree),
    // A shelved track named explicitly: still listed, per `--track` bypassing
    // the active-only default. The CLI/TUI half of this pair is `7071675`.
    row(&["list", "shelf"], TASK_IDS, Projection::TaskTree),
    row(&["list", "--state", "todo"], TASK_IDS, Projection::TaskTree),
    row(
        &["list", "--state", "active"],
        TASK_IDS,
        Projection::TaskTree,
    ),
    row(
        &["list", "--state", "blocked"],
        TASK_IDS,
        Projection::TaskTree,
    ),
    row(
        &["list", "--state", "parked"],
        TASK_IDS,
        Projection::TaskTree,
    ),
    // The `b664a3e` row: human read only Backlog and Parked while the JSON path
    // already included Done.
    row(&["list", "--state", "done"], TASK_IDS, Projection::TaskTree),
    row(
        &["list", "--state", "done", "--all"],
        TASK_IDS,
        Projection::TaskTree,
    ),
    row(&["list", "--tag", "core"], TASK_IDS, Projection::TaskTree),
    row(&["list", "--tag", "cc"], TASK_IDS, Projection::TaskTree),
    row(
        &["list", "--state", "todo", "--tag", "core"],
        TASK_IDS,
        Projection::TaskTree,
    ),
    row(
        &["list", "main", "--state", "done"],
        TASK_IDS,
        Projection::TaskTree,
    ),
    // -- fr show -------------------------------------------------------------
    row(&["show", "M-003"], TASK_IDS, Projection::ShowTaskOnly),
    row(&["show", "M-003.1"], TASK_IDS, Projection::ShowTaskOnly),
    row(
        &["show", "M-003.1", "--context"],
        TASK_IDS,
        Projection::ShowWithContext,
    ),
    // -- fr ready / blocked / recent -----------------------------------------
    row(&["ready"], TASK_IDS, Projection::ListEntries),
    row(&["ready", "--cc"], TASK_IDS, Projection::ListEntries),
    row(&["ready", "--tag", "cc"], TASK_IDS, Projection::ListEntries),
    row(
        &["ready", "--track", "side"],
        TASK_IDS,
        Projection::ListEntries,
    ),
    row(&["blocked"], TASK_IDS, Projection::ListEntries),
    row(&["recent"], TASK_IDS, Projection::ListEntries),
    row(
        &["recent", "--limit", "2"],
        TASK_IDS,
        Projection::ListEntries,
    ),
    // -- fr tracks / stats ---------------------------------------------------
    row(&["tracks"], TRACK_IDS, Projection::Field("id")),
    // `fr stats` prints track *names*, not ids — the JSON carries both.
    row(&["stats"], TRACK_NAMES, Projection::Field("name")),
    row(&["stats", "--all"], TRACK_NAMES, Projection::Field("name")),
    // -- fr search -----------------------------------------------------------
    // Matches a live task and an archived one, which is what pins the order the
    // two blocks print in: live first, then archive.
    row(&["search", "First task"], TASK_IDS, Projection::SearchIds),
    // --no-archive drops the archived block and keeps the live one.
    row(
        &["search", "--no-archive", "First task"],
        TASK_IDS,
        Projection::SearchIds,
    ),
    // Matches by id, and by the `dep:` lines of the two tasks depending on it —
    // three tasks, one of which the human line names in a different position.
    row(&["search", "M-001"], TASK_IDS, Projection::SearchIds),
    // A track filter also excludes the inbox, which belongs to no track.
    row(
        &["search", "--track", "side", "Side"],
        TASK_IDS,
        Projection::SearchIds,
    ),
    // Shelved tracks are not searched by default (`search_tasks` filters to
    // active), so this finds nothing — declared, not accidental.
    Row {
        args: &["search", "Diamond"],
        universe: TASK_IDS,
        projection: Projection::SearchIds,
        expect_empty: true,
    },
    // Inbox hits carry no task id, so this row compares titles.
    row(
        &["search", "design"],
        INBOX_TITLES,
        Projection::SearchTitles,
    ),
    Row {
        args: &["search", "zzz-no-such-thing"],
        universe: TASK_IDS,
        projection: Projection::SearchIds,
        expect_empty: true,
    },
    // -- fr deps -------------------------------------------------------------
    // A diamond: H-001 → H-002, H-003 → both → H-004. The second path reports
    // `repeat`, not `cycle`.
    row(&["deps", "H-001"], TASK_IDS, Projection::DepTree),
    // A genuine cycle, which must stop at the root rather than run a lap past it.
    row(&["deps", "H-005"], TASK_IDS, Projection::DepTree),
    // A dangling `dep:` target: named by both surfaces, held by neither.
    row(&["deps", "H-007"], TASK_IDS, Projection::DepTree),
    // A leaf. The human surface prints `(no dependencies)`; both still name the
    // root, so this is not an expect_empty row.
    row(&["deps", "H-004"], TASK_IDS, Projection::DepTree),
    // -- fr inbox ------------------------------------------------------------
    row(&["inbox"], INBOX_TITLES, Projection::Field("title")),
];

#[test]
fn human_and_json_name_the_same_things() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    create_fixture(root);

    let mut failures: Vec<String> = Vec::new();

    for row in ROWS {
        let human = human_sequence(&run_fr(root, row.args), row.universe);

        let mut json_args = vec!["--json"];
        json_args.extend_from_slice(row.args);
        let raw = run_fr(root, &json_args);
        let value: Value = serde_json::from_str(&raw)
            .unwrap_or_else(|e| panic!("fr --json {:?} emitted invalid JSON: {e}", row.args));
        let json = json_sequence(&value, row.projection);

        if human != json {
            failures.push(format!(
                "fr {}\n     human: {:?}\n      json: {:?}",
                row.args.join(" "),
                human,
                json
            ));
        } else if human.is_empty() && !row.expect_empty {
            failures.push(format!(
                "fr {}\n     both surfaces named nothing, and the row does not \
                 declare expect_empty — the fixture has stopped exercising this filter",
                row.args.join(" ")
            ));
        }
    }

    assert!(
        failures.is_empty(),
        "{} of {} parity rows failed:\n\n{}",
        failures.len(),
        ROWS.len(),
        failures.join("\n\n")
    );
}

// ---------------------------------------------------------------------------
// The standing guard
// ---------------------------------------------------------------------------

/// Why a subcommand is or is not in the matrix.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Class {
    /// A read command with a `--json` surface, covered by [`ROWS`].
    Covered,
    /// Writes. Prints a confirmation line; there is no `--json` surface and no
    /// listing for the two to disagree about.
    Write,
    /// `--json` is a global flag, so it is *accepted* here and silently
    /// ignored: the command prints human text either way. A consumer gets a
    /// parse error with no indication the flag did nothing.
    ///
    /// Empty today — `search` and `deps` were both here and both moved to
    /// `Covered`. Kept because this is the class a newly added read command
    /// most easily falls into, and naming it is what makes that visible.
    #[allow(dead_code)]
    JsonIgnored,
    /// Read command whose output is a scalar summary, not a listing — no
    /// sequence for the two surfaces to disagree about.
    NotAListing,
    /// Deferred, with the reason.
    Deferred(&'static str),
}

/// Every `fr` subcommand, classified. This list is checked against clap's own
/// subcommand list in both directions, so a new command fails the build until
/// someone decides which class it belongs to.
const CLASSIFICATION: &[(&str, Class)] = &[
    ("list", Class::Covered),
    ("show", Class::Covered),
    ("ready", Class::Covered),
    ("blocked", Class::Covered),
    ("tracks", Class::Covered),
    ("stats", Class::Covered),
    ("recent", Class::Covered),
    ("inbox", Class::Covered),
    ("search", Class::Covered),
    ("deps", Class::Covered),
    ("info", Class::NotAListing),
    (
        "check",
        Class::Deferred(
            "check's human and JSON surfaces are a real pair, but on a healthy \
             fixture both are empty — covering it needs the damaged-fixture corpus",
        ),
    ),
    (
        "projects",
        Class::Deferred("reads the global registry, not project content"),
    ),
    (
        "actor",
        Class::Deferred("reads the actor registry, not project content"),
    ),
    (
        "recovery",
        Class::Deferred("reads the recovery log, which is empty on a healthy fixture"),
    ),
    ("init", Class::Write),
    // Writes the merged file the VCS handed it. Its real interface is an exit
    // status, not a listing, and `--json` has nothing to describe.
    ("merge", Class::Write),
    // Writes .gitignore/.gitattributes/.git/config. Its `--json` surface reports
    // what it did, not a listing of project content.
    ("git", Class::Write),
    ("add", Class::Write),
    ("push", Class::Write),
    ("sub", Class::Write),
    ("state", Class::Write),
    ("start", Class::Write),
    ("done", Class::Write),
    ("tag", Class::Write),
    ("dep", Class::Write),
    ("note", Class::Write),
    ("ref", Class::Write),
    ("spec", Class::Write),
    ("title", Class::Write),
    ("mv", Class::Write),
    ("triage", Class::Write),
    ("track", Class::Write),
    ("clean", Class::Write),
    ("import", Class::Write),
    ("delete", Class::Write),
];

fn class_of(name: &str) -> Option<Class> {
    CLASSIFICATION
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, c)| *c)
}

#[test]
fn every_subcommand_is_classified() {
    use clap::CommandFactory;

    let cmd = frame::cli::commands::Cli::command();
    let mut unclassified: Vec<&str> = Vec::new();
    for sub in cmd.get_subcommands() {
        if class_of(sub.get_name()).is_none() {
            unclassified.push(sub.get_name());
        }
    }

    assert!(
        unclassified.is_empty(),
        "new subcommand(s) {unclassified:?} are not classified in tests/parity.rs.\n\
         Add each to CLASSIFICATION: `Covered` (and add matrix rows) if it is a read \
         command with a --json surface, or one of the exempt classes with a reason."
    );
}

#[test]
fn classification_names_only_real_subcommands() {
    use clap::CommandFactory;

    let cmd = frame::cli::commands::Cli::command();
    let real: Vec<&str> = cmd.get_subcommands().map(|s| s.get_name()).collect();
    let stale: Vec<&str> = CLASSIFICATION
        .iter()
        .map(|(n, _)| *n)
        .filter(|n| !real.contains(n))
        .collect();

    assert!(
        stale.is_empty(),
        "CLASSIFICATION names subcommand(s) that no longer exist: {stale:?}"
    );
}

#[test]
fn covered_subcommands_have_matrix_rows() {
    let missing: Vec<&str> = CLASSIFICATION
        .iter()
        .filter(|(_, c)| *c == Class::Covered)
        .map(|(n, _)| *n)
        .filter(|name| !ROWS.iter().any(|r| r.args.first() == Some(name)))
        .collect();

    assert!(
        missing.is_empty(),
        "subcommand(s) {missing:?} are classified Covered but have no rows in ROWS"
    );
}

#[test]
fn matrix_rows_are_all_covered_subcommands() {
    let wrong: Vec<&str> = ROWS
        .iter()
        .filter_map(|r| r.args.first().copied())
        .filter(|name| class_of(name) != Some(Class::Covered))
        .collect();

    assert!(
        wrong.is_empty(),
        "ROWS exercise subcommand(s) {wrong:?} that are not classified Covered"
    );
}

// ---------------------------------------------------------------------------
// Extractor self-tests
// ---------------------------------------------------------------------------
//
// The extractors are the part of this file that could pass a row for the wrong
// reason, so they are tested directly.

#[test]
fn identifier_match_respects_token_boundaries() {
    // `M-003` must not shadow `M-003.1` when both are candidates.
    assert_eq!(
        first_identifier("  [ ] M-003.1 Sub one", TASK_IDS).as_deref(),
        Some("M-003.1")
    );
    assert_eq!(
        first_identifier("[ ] M-003 Third task", TASK_IDS).as_deref(),
        Some("M-003")
    );
    // A track id inside a path is not the id column.
    assert_eq!(
        first_identifier(" Side Track  side  S  tracks/side.md", TRACK_IDS).as_deref(),
        Some("side")
    );
    assert_eq!(first_identifier("== Main Track (main) ==", TASK_IDS), None);
}

#[test]
fn human_sequence_ignores_dependency_mentions() {
    // `fr blocked` prints the blocker on the same line as the blocked task.
    let out = "[main] [-] M-004 Blocked task #cc (blocked by: M-001)\n";
    assert_eq!(human_sequence(out, TASK_IDS), vec!["M-004"]);

    // `fr show` prints a `dep:` line of its own.
    let out = "[>] M-002 Second task\nadded: 2025-05-02\ndep: M-001\n";
    assert_eq!(human_sequence(out, TASK_IDS), vec!["M-002"]);
}

#[test]
fn json_projections_read_the_documented_shapes() {
    // ListEntries must not descend into subtasks: `fr ready` returns whole task
    // objects, but the listing is the array.
    let v: Value = serde_json::from_str(
        r#"{"tasks":[{"track":"main","id":"M-003","subtasks":[{"id":"M-003.2"}]},
                    {"track":"main","id":"M-003.1"}]}"#,
    )
    .unwrap();
    assert_eq!(
        json_sequence(&v, Projection::ListEntries),
        vec!["M-003", "M-003.1"]
    );
    // TaskTree does descend, parent before subtask.
    assert_eq!(
        json_sequence(&v, Projection::TaskTree),
        vec!["M-003", "M-003.2", "M-003.1"]
    );

    // ShowWithContext puts ancestors first, matching the human `--context` order.
    let v: Value =
        serde_json::from_str(r#"{"id":"M-003.1","ancestors":[{"id":"M-003"}]}"#).unwrap();
    assert_eq!(
        json_sequence(&v, Projection::ShowWithContext),
        vec!["M-003", "M-003.1"]
    );
    assert_eq!(json_sequence(&v, Projection::ShowTaskOnly), vec!["M-003.1"]);
}

// ---------------------------------------------------------------------------
// CLI vs TUI
// ---------------------------------------------------------------------------
//
// The second pair. `e1a8dbe` fixed `ops::task_ops` so a Parked or Done task
// could be moved across tracks; `83be43a` then had to fix the TUI separately,
// because the defect was never in the shared op — it was the TUI's own guard,
// upstream of it, refusing to call it.
//
// That is why these cases are driven by **keystrokes** rather than by calling
// `App`'s action methods. Calling the methods would be easier and would skip
// the guard, which is the thing that broke.
//
// What is *not* driven by keystrokes is getting to the task: the case declares
// a starting view and cursor position, set directly. Positioning is not the
// behavior under test, and the routes differ per view — a Done task is not
// reachable in the track view at all ("Done tasks are NOT shown in track view"
// in `build_flat_items`), so its cases start from the detail or recent view the
// way a user would arrive there.
//
// Scope is deliberately narrow: cross-track move and state change, the two
// operations with a documented drift history. The helpers below are most of the
// work; the remaining shared operations grow onto them.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use frame::tui::app::{App, View};
use frame::tui::input::handle_key;

/// A keystroke, spelled so the case table stays readable.
#[derive(Clone, Copy, Debug)]
enum Press {
    Char(char),
    Enter,
    Space,
}

fn press(app: &mut App, stroke: Press) {
    let event = match stroke {
        Press::Char(c) if c.is_ascii_uppercase() => {
            KeyEvent::new(KeyCode::Char(c), KeyModifiers::SHIFT)
        }
        Press::Char(c) => KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE),
        Press::Enter => KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        Press::Space => KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE),
    };
    handle_key(app, event);
}

/// Where the TUI cursor sits before the keys are pressed.
#[derive(Clone, Copy)]
enum Start {
    /// Track view, cursor on this task — what `J` (jump to task) does.
    Track(&'static str),
    /// Detail view for `(track_id, task_id)`. The route to a Done task, which
    /// the track view does not list; a user arrives here from Recent or search.
    Detail(&'static str, &'static str),
    /// Recent view, cursor on the entry at this index.
    Recent(usize),
}

struct SurfaceCase {
    /// What the case does, for the failure message.
    what: &'static str,
    cli: &'static [&'static str],
    start: Start,
    keys: &'static [Press],
    /// The CLI is expected to refuse. Both surfaces must then leave the project
    /// alone — a refusal on one side and a completed write on the other is the
    /// `7071675` shape.
    cli_refuses: bool,
    /// A divergence this suite has found and that is not fixed yet. Set it in
    /// a struct literal, the way `cli_refuses` is set below.
    ///
    /// Such a case is inverted rather than skipped: it asserts the two surfaces
    /// still *disagree*, so landing the fix fails the test until the note comes
    /// off. A skipped case would go on passing forever after the fix, and a
    /// stale note is exactly the drift this file exists to catch. Both notes
    /// this suite started with are gone because their fixes landed.
    known_divergence: Option<&'static str>,
}

const fn case(
    what: &'static str,
    cli: &'static [&'static str],
    start: Start,
    keys: &'static [Press],
) -> SurfaceCase {
    SurfaceCase {
        what,
        cli,
        start,
        keys,
        cli_refuses: false,
        known_divergence: None,
    }
}

use Press::{Char, Enter, Space};

const SURFACE_CASES: &[SurfaceCase] = &[
    // -- state changes within the Backlog section ----------------------------
    case(
        "mark a Backlog task done",
        &["state", "M-001", "done"],
        Start::Track("M-001"),
        &[Char('x')],
    ),
    case(
        "park a Backlog task",
        &["state", "M-001", "parked"],
        Start::Track("M-001"),
        &[Char('~')],
    ),
    case(
        "block a Backlog task",
        &["state", "M-001", "blocked"],
        Start::Track("M-001"),
        &[Char('b')],
    ),
    // A subtask never moves between sections, so the two surfaces have less to
    // disagree about — included as the control for the cases that do move.
    case(
        "mark a subtask done",
        &["state", "M-003.1", "done"],
        Start::Track("M-003.1"),
        &[Char('x')],
    ),
    // -- state changes that cross a section boundary -------------------------
    case(
        "mark a top-level Parked task done",
        &["state", "M-010", "done"],
        Start::Track("M-010"),
        &[Char('x')],
    ),
    case(
        "unpark a top-level Parked task",
        &["state", "M-010", "todo"],
        Start::Track("M-010"),
        &[Char('o')],
    ),
    // Reopening is Space in the Recent view — the track view has no Done tasks
    // to put a cursor on. Recent order is M-005, M-000, S-000.
    case(
        "reopen a top-level Done task",
        &["state", "M-000", "todo"],
        Start::Recent(1),
        &[Space],
    ),
    // The cell both surfaces used to miss. Each enumerated its section moves as
    // `from → to` pairs and neither listed Done → Parked, so the task stayed in
    // `## Done` wearing `[~]`. Parity could not see it: the two agreed with each
    // other and disagreed with `canonical_section`, which is a third opinion this
    // suite never compares against. Both now compute the target instead.
    case(
        "park a top-level Done task",
        &["state", "M-000", "parked"],
        Start::Detail("main", "M-000"),
        &[Char('~')],
    ),
    // Reopening used to be *view*-dependent, which is worse than the
    // state-dependent gaps above: the same key on the same task did different
    // things depending on where you were looking at it from. The Board had a
    // branch of its own and Recent had `reopen_recent_task`; the detail view had
    // neither, so a reopened task stayed in `## Done` as `[ ]`. The Recent case
    // above passed throughout, which is why nothing caught it.
    case(
        "reopen a top-level Done task from the detail view",
        &["state", "M-000", "todo"],
        Start::Detail("main", "M-000"),
        &[Char('o')],
    ),
    case(
        "block a top-level Done task from the detail view",
        &["state", "M-000", "blocked"],
        Start::Detail("main", "M-000"),
        &[Char('b')],
    ),
    // -- cross-track move ----------------------------------------------------
    // `M`, type the target's prefix, Enter, `b` for bottom. The CLI's default
    // position for `mv --track` is Bottom too.
    case(
        "move a Backlog task to another track",
        &["mv", "M-001", "--track", "side"],
        Start::Track("M-001"),
        &[Char('M'), Char('S'), Enter, Char('b')],
    ),
    case(
        "move a Parked task to another track",
        &["mv", "M-010", "--track", "side"],
        Start::Track("M-010"),
        &[Char('M'), Char('S'), Enter, Char('b')],
    ),
    // The `83be43a` case itself.
    case(
        "move a Done task to another track",
        &["mv", "M-000", "--track", "side"],
        Start::Detail("main", "M-000"),
        &[Char('M'), Char('S'), Enter, Char('b')],
    ),
    // -- refusal -------------------------------------------------------------
    // `reject_add_to_shelved` makes the CLI refuse a move into a shelved track.
    // The TUI must refuse too, or this is `7071675` again with the surfaces
    // swapped.
    SurfaceCase {
        what: "move a task into a shelved track",
        cli: &["mv", "M-001", "--track", "shelf"],
        start: Start::Track("M-001"),
        keys: &[Char('M'), Char('H'), Enter, Char('b')],
        cli_refuses: true,
        known_divergence: None,
    },
    // -- stated divergence ---------------------------------------------------
    // The one place the two surfaces are deliberately not the same. `fr ref add`
    // refuses a path that leaves the project; the detail-view editor stores it
    // and paints it red, and `fr check` reports it as a warning.
    //
    // Not the `7071675` shape — that was two surfaces that each believed they
    // agreed. This is a decision: refusing in the TUI needs a `--force`
    // equivalent that does not exist, and discarding what someone just typed is
    // worse than keeping it and saying it is wrong.
    //
    // Opened with `Enter` from the track view rather than via `Start::Detail`,
    // which sets `app.view` directly and leaves `detail_state` unbuilt — `@` has
    // no region to jump to then, and the case passes for the wrong reason.
    SurfaceCase {
        what: "add a ref that leaves the project",
        cli: &["ref", "M-001", "add", "../outside.md"],
        start: Start::Track("M-001"),
        keys: &[
            Enter,
            Char('@'),
            Char('.'),
            Char('.'),
            Char('/'),
            Char('o'),
            Char('u'),
            Char('t'),
            Char('s'),
            Char('i'),
            Char('d'),
            Char('e'),
            Char('.'),
            Char('m'),
            Char('d'),
            Enter,
        ],
        cli_refuses: true,
        known_divergence: Some(
            "the CLI refuses a ref that leaves the project and writes nothing; \
             the TUI stores it and renders it in the error colour, and `fr check` \
             reports `ref_outside_project`. If these ever agree, the TUI started \
             refusing — take this note off.",
        ),
    },
];

/// Run `fr`, returning whether it succeeded rather than asserting it did.
fn try_fr(dir: &Path, args: &[&str]) -> bool {
    Command::new(fr_bin())
        .args(args)
        .current_dir(dir)
        .env("XDG_CONFIG_HOME", dir.join(".xdg-config"))
        .output()
        .expect("failed to run fr")
        .status
        .success()
}

/// Every file under `frame/` the two surfaces are expected to agree on, keyed
/// by path relative to `frame/`.
///
/// Working-copy-local files are excluded via `LOCAL_ONLY_FRAME_FILES` — the
/// lock, the UI state, the actor token, the id frontier and the in-flight
/// marker are per-working-copy by definition, and these are two working copies.
/// Third consumer of that constant, after `fr init` and `fr check`.
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

/// Apply a case through the TUI the way the event loop does: position, press
/// keys, then flush the grace-period section moves and save what they touched
/// (`app.rs` does exactly this on view change and on quit).
fn drive_tui(root: &Path, case: &SurfaceCase) {
    let project = frame::io::project_io::load_project(root).expect("load project");
    let mut app = App::new(project);

    match case.start {
        Start::Track(task_id) => assert!(
            app.jump_to_task(task_id),
            "{}: no cursor position for {task_id} in the track view",
            case.what
        ),
        Start::Detail(track_id, task_id) => {
            app.view = View::Detail {
                track_id: track_id.to_string(),
                task_id: task_id.to_string(),
            };
        }
        Start::Recent(index) => {
            app.view = View::Recent;
            app.recent_cursor = index;
        }
    }

    for stroke in case.keys {
        press(&mut app, *stroke);
    }

    for track_id in app.flush_all_pending_moves() {
        app.save_track_logged(&track_id);
    }
}

#[test]
fn cli_and_tui_leave_the_same_files() {
    let dir = tempfile::tempdir().unwrap();
    let mut failures: Vec<String> = Vec::new();

    for (i, case) in SURFACE_CASES.iter().enumerate() {
        // A fresh pair of working copies per case: the surfaces must agree about
        // one operation applied to the same starting state.
        let via_cli = dir.path().join(format!("case{i}-cli"));
        let via_tui = dir.path().join(format!("case{i}-tui"));
        create_fixture(&via_cli);
        create_fixture(&via_tui);

        let ok = try_fr(&via_cli, case.cli);
        if ok == case.cli_refuses {
            failures.push(format!(
                "{} — `fr {}` {} but the case says it should {}",
                case.what,
                case.cli.join(" "),
                if ok { "succeeded" } else { "failed" },
                if case.cli_refuses {
                    "refuse"
                } else {
                    "succeed"
                },
            ));
            continue;
        }

        drive_tui(&via_tui, case);

        let cli_tree = frame_tree(&via_cli);
        let tui_tree = frame_tree(&via_tui);
        let agree = cli_tree == tui_tree;

        match (agree, case.known_divergence) {
            (true, None) => {}
            (false, Some(_)) => {}
            (true, Some(note)) => failures.push(format!(
                "{} — the surfaces now agree, so the known-divergence note is stale \
                 and should come off:\n  {note}",
                case.what,
            )),
            (false, None) => {
                let keys: Vec<String> = case.keys.iter().map(|k| format!("{k:?}")).collect();
                failures.push(format!(
                    "{} — `fr {}` vs [{}]\n{}",
                    case.what,
                    case.cli.join(" "),
                    keys.join(", "),
                    describe_tree_diff(&cli_tree, &tui_tree),
                ));
            }
        }
    }

    assert!(
        failures.is_empty(),
        "{} of {} CLI/TUI cases disagreed:\n\n{}",
        failures.len(),
        SURFACE_CASES.len(),
        failures.join("\n\n")
    );
}

/// Render the first differing file as two labelled blocks. Whole files rather
/// than a line diff: these are small, and section placement — the thing most of
/// these cases turn on — is only legible with the sections around it.
fn describe_tree_diff(cli: &[(String, String)], tui: &[(String, String)]) -> String {
    for (path, cli_body) in cli {
        match tui.iter().find(|(p, _)| p == path) {
            Some((_, tui_body)) if tui_body == cli_body => continue,
            Some((_, tui_body)) => {
                return format!(
                    "  {path} differs.\n  --- via CLI ---\n{}\n  --- via TUI ---\n{}",
                    indent(cli_body),
                    indent(tui_body)
                );
            }
            None => return format!("  {path} exists only in the CLI copy"),
        }
    }
    for (path, _) in tui {
        if !cli.iter().any(|(p, _)| p == path) {
            return format!("  {path} exists only in the TUI copy");
        }
    }
    "  trees differ, but no single differing file was found".to_string()
}

fn indent(body: &str) -> String {
    body.lines()
        .map(|l| format!("  | {l}"))
        .collect::<Vec<_>>()
        .join("\n")
}
