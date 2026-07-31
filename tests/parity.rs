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
    "S-001", "S-002", "S-010", "H-001",
];

const TRACK_IDS: &[&str] = &["main", "side", "shelf"];

const TRACK_NAMES: &[&str] = &["Main Track", "Side Track", "Shelf Track"];

const INBOX_TITLES: &[&str] = &["Bug in parser", "Think about design", "Quick note"];

/// A fixture built so that no row is vacuous: every filter in [`ROWS`] selects
/// something. Deliberately awkward in two places — `S-002` is state Parked while
/// sitting in the *Backlog* section, and `shelf` is a shelved track with live
/// work in it — because both are states a real project reaches and both are
/// where a human/JSON pair is most likely to disagree.
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

## Done
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
    /// `SearchHitJson` exists in `cli/output.rs` and is constructed nowhere —
    /// the JSON shape for `fr search` was designed and never wired up. Both are
    /// tracked as their own items; when either lands, move it to `Covered` and
    /// add its rows.
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
    ("search", Class::JsonIgnored),
    ("deps", Class::JsonIgnored),
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
