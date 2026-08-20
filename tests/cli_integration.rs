//! Integration tests for the `fr` CLI.
//!
//! Each test creates a temp project directory, runs `fr` as a subprocess,
//! and verifies stdout and/or file contents.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Get the path to the built `fr` binary.
fn fr_bin() -> PathBuf {
    // cargo test builds to target/debug/
    let mut path = std::env::current_exe().unwrap();
    path.pop(); // remove test binary name
    path.pop(); // remove deps/
    path.push("fr");
    path
}

/// Create a minimal test project in the given directory.
fn create_test_project(root: &Path) {
    let frame_dir = root.join("frame");
    fs::create_dir_all(frame_dir.join("tracks")).unwrap();

    // Record this working copy as the primary (null) actor, exactly as `fr init`
    // does, so mints stay in the legacy null namespace (e.g. `M-011`) and don't
    // auto-claim a letter token.
    fs::write(frame_dir.join(".actor"), "null\n").unwrap();

    fs::write(
        frame_dir.join("project.toml"),
        r#"[project]
name = "test-project"

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

[ids.prefixes]
main = "M"
side = "S"
"#,
    )
    .unwrap();

    fs::write(
        frame_dir.join("tracks/main.md"),
        "\
# Main Track

> The main work stream.

## Backlog

- [ ] `M-001` First task #core
  - added: 2025-05-01
- [>] `M-002` Second task #core #cc
  - added: 2025-05-02
  - dep: M-001
- [ ] `M-003` Third task with subtasks
  - added: 2025-05-03
  - [ ] `M-003.1` Sub one
    - added: 2025-05-03
  - [ ] `M-003.2` Sub two
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

    fs::write(
        frame_dir.join("tracks/side.md"),
        "\
# Side Track

## Backlog

- [ ] `S-001` Side task one
  - added: 2025-05-01
- [ ] `S-002` Side task two
  - added: 2025-05-02

## Done
",
    )
    .unwrap();

    fs::write(
        frame_dir.join("inbox.md"),
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

/// Run `fr` with the given args in the given directory, returning (stdout, stderr, success).
/// Overwrite a track file wholesale, for tests that need a shape the shared
/// fixture doesn't have.
fn write_track(root: &Path, track_id: &str, body: &str) {
    fs::write(
        root.join("frame")
            .join("tracks")
            .join(format!("{track_id}.md")),
        body,
    )
    .unwrap();
}

fn run_fr(dir: &Path, args: &[&str]) -> (String, String, bool) {
    let output = Command::new(fr_bin())
        .args(args)
        .current_dir(dir)
        // Isolate tests from the real global registry (~/.config/frame/projects.toml)
        .env("XDG_CONFIG_HOME", dir.join(".xdg-config"))
        .output()
        .expect("failed to run fr");

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    (stdout, stderr, output.status.success())
}

/// Run `fr` with extra environment variables. Used by the crash-injection tests
/// to fail a specific write via `FRAME_FAIL_WRITE` (see `src/io/fault.rs`).
fn run_fr_env(dir: &Path, args: &[&str], env: &[(&str, &str)]) -> (String, String, bool) {
    let mut cmd = Command::new(fr_bin());
    cmd.args(args)
        .current_dir(dir)
        .env("XDG_CONFIG_HOME", dir.join(".xdg-config"));
    for (k, v) in env {
        cmd.env(k, v);
    }
    let output = cmd.output().expect("failed to run fr");
    (
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
        output.status.success(),
    )
}

/// Run `fr` expecting success, return stdout.
fn run_fr_ok(dir: &Path, args: &[&str]) -> String {
    let (stdout, stderr, success) = run_fr(dir, args);
    if !success {
        panic!(
            "fr {:?} failed:\nstdout: {}\nstderr: {}",
            args, stdout, stderr
        );
    }
    stdout
}

// ---------------------------------------------------------------------------
// Read command tests
// ---------------------------------------------------------------------------

#[test]
fn test_list_default() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());

    let out = run_fr_ok(tmp.path(), &["list"]);
    assert!(out.contains("Main Track"));
    assert!(out.contains("M-001"));
    assert!(out.contains("Side Track"));
    assert!(out.contains("S-001"));
    // Done tasks are omitted from the default listing.
    assert!(
        !out.contains("M-000"),
        "default list should not show done tasks"
    );
}

#[test]
fn test_list_state_done_shows_done_tasks() {
    // `--state done` must surface the Done section in human output, matching the
    // `--json` path — previously the human listing only read Backlog/Parked, so
    // this filter silently returned nothing.
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());

    // M-000 ("Setup project") is a done task in the fixture's main track.
    let human = run_fr_ok(tmp.path(), &["list", "main", "--state", "done"]);
    assert!(
        human.contains("M-000") && human.contains("Setup project"),
        "human list --state done should show the done task: {human}"
    );
    // Non-done tasks are filtered out.
    assert!(
        !human.contains("M-001"),
        "should not show todo tasks: {human}"
    );

    // The JSON path already worked; confirm the two agree.
    let json = run_fr_ok(tmp.path(), &["list", "main", "--state", "done", "--json"]);
    assert!(
        json.contains("M-000"),
        "json list --state done should include M-000"
    );
}

#[test]
fn test_projects_prune_removes_not_found() {
    // All `fr` calls share one isolated registry via the XDG anchor `base`.
    let base = tempfile::TempDir::new().unwrap();
    let live = base.path().join("live");
    let ghost = base.path().join("ghost");
    create_test_project(&live);
    create_test_project(&ghost);

    run_fr_ok(base.path(), &["projects", "add", live.to_str().unwrap()]);
    run_fr_ok(base.path(), &["projects", "add", ghost.to_str().unwrap()]);

    // The ghost project's directory disappears (e.g. a temp smoke-test project).
    fs::remove_dir_all(&ghost).unwrap();

    // Dry run reports the ghost but mutates nothing.
    let dry = run_fr_ok(base.path(), &["projects", "prune", "--dry-run", "--json"]);
    assert!(dry.contains("ghost"));
    assert!(!dry.contains("\"live\"") && !dry.contains("/live\""));
    let still = run_fr_ok(base.path(), &["projects", "list", "--json"]);
    assert!(still.contains("/ghost"), "dry-run must not remove anything");

    // Real prune drops the ghost, keeps the live project.
    let pruned = run_fr_ok(base.path(), &["projects", "prune"]);
    assert!(pruned.contains("Removed 1 not-found project"));
    let after = run_fr_ok(base.path(), &["projects", "list", "--json"]);
    assert!(after.contains("/live"));
    assert!(!after.contains("/ghost"));

    // Pruning again is a no-op.
    let again = run_fr_ok(base.path(), &["projects", "prune"]);
    assert!(again.contains("No not-found projects"));
}

/// Run `fr` against a registry shared with the other calls in a test, rather
/// than the per-directory one `run_fr` anchors on the cwd. A worktree test has to
/// run `fr` from two directories and see one registry.
fn run_fr_registry(dir: &Path, args: &[&str], xdg: &Path) -> String {
    let (stdout, stderr, success) =
        run_fr_env(dir, args, &[("XDG_CONFIG_HOME", &xdg.to_string_lossy())]);
    if !success {
        panic!("fr {args:?} failed:\nstdout: {stdout}\nstderr: {stderr}");
    }
    stdout
}

/// A committed project in a git repo, ready for `git worktree add`. The
/// working-copy-local frame files are gitignored exactly as `fr init` leaves
/// them, so `git worktree remove` does not refuse over them later.
/// `None` when git is unavailable, in which case the caller skips.
fn repo_project(root: &Path) -> Option<()> {
    create_test_project(root);
    let ignore: String = frame::io::project_io::LOCAL_ONLY_FRAME_FILES
        .iter()
        .map(|name| format!("frame/{}\n", name))
        .collect();
    fs::write(root.join(".gitignore"), ignore).unwrap();
    if !git(root, &["init", "-q"]) {
        return None;
    }
    git(root, &["add", "-A"]);
    git(
        root,
        &[
            "-c",
            "user.name=frame-test",
            "-c",
            "user.email=frame@test.invalid",
            "commit",
            "-q",
            "-m",
            "init",
        ],
    )
    .then_some(())
}

/// The whole worktree lifecycle: a worktree registers itself, lists under its
/// project labelled by branch, and its row retires itself once the worktree is
/// gone — with no `fr projects prune` in the story.
#[test]
fn test_worktree_registers_nests_and_self_heals() {
    let base = tempfile::TempDir::new().unwrap();
    let main = base.path().join("main");
    let Some(()) = repo_project(&main) else {
        return; // git unavailable
    };
    // A worktree beside its parent, not nested under it — the relationship is
    // git's answer, not a path prefix.
    let wt = base.path().join("wt-feature");
    assert!(git(
        &main,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "feature",
            wt.to_str().unwrap()
        ]
    ));

    // Touching each one from the CLI registers it, exactly as an agent would.
    let xdg = base.path().join(".xdg-shared");
    run_fr_registry(&main, &["list"], &xdg);
    run_fr_registry(&wt, &["list"], &xdg);

    let json = run_fr_registry(base.path(), &["projects", "list", "--json"], &xdg);
    assert!(
        json.contains("worktree_of"),
        "the worktree's provenance is recorded: {json}"
    );
    assert!(
        json.contains("\"branch\": \"feature\""),
        "the branch is reported: {json}"
    );

    // Human output nests the worktree under its project and labels it by branch,
    // because both rows carry the project's committed name.
    let human = run_fr_registry(base.path(), &["projects", "list"], &xdg);
    let lines: Vec<&str> = human
        .lines()
        .filter(|l| l.contains("test-project"))
        .collect();
    assert_eq!(lines.len(), 1, "one project row: {human}");
    let nested = human
        .lines()
        .find(|l| l.contains("\u{2514} feature"))
        .unwrap_or_else(|| panic!("a branch-labelled worktree row: {human}"));
    assert!(
        nested.starts_with("    "),
        "the worktree row is indented under its project: {nested:?}"
    );

    // The worktree goes away. Nothing is run against it; the next listing is
    // where the row is noticed, and it retires itself there.
    assert!(git(&main, &["worktree", "remove", wt.to_str().unwrap()]));
    let healed = run_fr_registry(base.path(), &["projects", "list"], &xdg);
    assert!(
        !healed.contains("wt-feature") && !healed.contains("\u{2514} feature"),
        "the removed worktree's row is gone: {healed}"
    );
    assert!(
        healed.contains("Retired 1 worktree entry"),
        "and says so rather than silently rewriting the registry: {healed}"
    );
    assert!(
        healed.contains("main"),
        "the project itself stays: {healed}"
    );

    // Nothing left for prune to do — the point of the exercise.
    let prune = run_fr_registry(base.path(), &["projects", "prune"], &xdg);
    assert!(prune.contains("No not-found projects"), "{prune}");
}

/// An entry registered before frame recorded provenance — or by another `fr` on
/// the machine — is stamped by the next listing, so it groups and, once the
/// worktree dies, retires itself. Without this, the entries already in a user's
/// registry would never get either.
#[test]
fn test_listing_backfills_provenance_on_an_older_entry() {
    let base = tempfile::TempDir::new().unwrap();
    let main = base.path().join("main");
    let Some(()) = repo_project(&main) else {
        return; // git unavailable
    };
    let wt = base.path().join("wt-old");
    assert!(git(
        &main,
        &["worktree", "add", "-q", "-b", "old", wt.to_str().unwrap()]
    ));

    // A registry written by a frame that knew nothing about worktrees.
    let xdg = base.path().join(".xdg-shared");
    let reg_path = xdg.join("frame").join("projects.toml");
    fs::create_dir_all(reg_path.parent().unwrap()).unwrap();
    fs::write(
        &reg_path,
        format!(
            "[[projects]]\nname = \"test-project\"\npath = {:?}\n\n\
             [[projects]]\nname = \"test-project\"\npath = {:?}\n",
            main.to_str().unwrap(),
            wt.to_str().unwrap()
        ),
    )
    .unwrap();
    assert!(
        !fs::read_to_string(&reg_path)
            .unwrap()
            .contains("worktree_of")
    );

    // Listing asks git, stamps what it learns, and nests the row.
    let human = run_fr_registry(base.path(), &["projects", "list"], &xdg);
    assert!(
        human.contains("\u{2514} old"),
        "the older entry groups under its project: {human}"
    );
    let stamped = fs::read_to_string(&reg_path).unwrap();
    assert!(
        stamped.contains("worktree_of"),
        "and the registry now records why: {stamped}"
    );

    // Which is what lets it retire itself when the worktree goes.
    assert!(git(&main, &["worktree", "remove", wt.to_str().unwrap()]));
    let healed = run_fr_registry(base.path(), &["projects", "list"], &xdg);
    assert!(
        !healed.contains("wt-old"),
        "the dead row goes without being asked: {healed}"
    );
}

/// A live worktree checked out to a branch with no `frame/` still exists, so
/// neither the self-heal nor `prune` may take its row.
#[test]
fn test_prune_keeps_a_live_worktree_without_a_frame_dir() {
    let base = tempfile::TempDir::new().unwrap();
    let main = base.path().join("main");
    let Some(()) = repo_project(&main) else {
        return; // git unavailable
    };
    let wt = base.path().join("wt-empty");
    assert!(git(
        &main,
        &["worktree", "add", "-q", "-b", "empty", wt.to_str().unwrap()]
    ));

    // Registered while `frame/` was there; then the branch it switches to does
    // not have one, so the directory stays and the project inside it goes.
    let xdg = base.path().join(".xdg-shared");
    run_fr_registry(&wt, &["list"], &xdg);
    fs::remove_dir_all(wt.join("frame")).unwrap();

    let listed = run_fr_registry(base.path(), &["projects", "list", "--json"], &xdg);
    assert!(
        listed.contains("wt-empty"),
        "a live worktree keeps its row even with no frame/: {listed}"
    );
    let prune = run_fr_registry(base.path(), &["projects", "prune"], &xdg);
    assert!(
        prune.contains("No not-found projects"),
        "prune must not remove a directory that is right there: {prune}"
    );
}

#[test]
fn test_list_specific_track() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());

    let out = run_fr_ok(tmp.path(), &["list", "main"]);
    assert!(out.contains("M-001"));
    assert!(!out.contains("S-001"));
}

#[test]
fn test_list_with_state_filter() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());

    let out = run_fr_ok(tmp.path(), &["list", "main", "--state", "active"]);
    assert!(out.contains("M-002"));
    assert!(!out.contains("M-001")); // M-001 is todo, not active
}

#[test]
fn test_list_with_tag_filter() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());

    let out = run_fr_ok(tmp.path(), &["list", "main", "--tag", "cc"]);
    assert!(out.contains("M-002"));
    assert!(!out.contains("M-001")); // M-001 doesn't have #cc
}

#[test]
fn test_list_json() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());

    let out = run_fr_ok(tmp.path(), &["list", "main", "--json"]);
    let parsed: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert!(parsed.is_array());
    let arr = parsed.as_array().unwrap();
    assert_eq!(arr.len(), 1); // One track
    assert_eq!(arr[0]["track"], "main");
    assert!(arr[0]["tasks"].is_array());
}

#[test]
fn test_show() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());

    let out = run_fr_ok(tmp.path(), &["show", "M-001"]);
    assert!(out.contains("First task"));
    assert!(out.contains("added: 2025-05-01"));
}

#[test]
fn test_show_json() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());

    let out = run_fr_ok(tmp.path(), &["show", "M-002", "--json"]);
    let parsed: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(parsed["id"], "M-002");
    assert_eq!(parsed["state"], "active");
    assert!(
        parsed["deps"]
            .as_array()
            .unwrap()
            .contains(&serde_json::json!("M-001"))
    );
}

#[test]
fn test_show_not_found() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());

    let (_stdout, stderr, success) = run_fr(tmp.path(), &["show", "NOEXIST-999"]);
    assert!(!success);
    assert!(stderr.contains("not found"));
}

/// Write a done-task archive by hand, the shape `fr clean` leaves behind.
fn write_archive(root: &Path, track_id: &str, body: &str) {
    let dir = root.join("frame").join("archive");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join(format!("{track_id}.md")), body).unwrap();
}

/// The archive fixture: one done task with a subtask, out of the `main` track.
const ARCHIVED_MAIN: &str = "\
# Archive — main

- [x] `M-900` Archived widget #legacy
  - added: 2025-01-02
  - resolved: 2025-02-03
  - [x] `M-900.1` Archived subtask
    - resolved: 2025-02-03
";

/// `fr clean` moves a done task out of its track file into `archive/<track>.md`.
/// The task is still in the project and `fr show` is the only surface that can
/// read it — the TUI's archive search hits are read-only stubs — so reporting
/// `task not found` for one was the whole bug.
#[test]
fn show_falls_back_to_the_done_archive() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());
    write_archive(tmp.path(), "main", ARCHIVED_MAIN);

    let out = run_fr_ok(tmp.path(), &["show", "M-900"]);
    assert!(out.contains("Archived widget"), "{out}");
    assert!(out.contains("resolved: 2025-02-03"), "{out}");
    assert!(
        out.contains("archived: main (frame/archive/main.md)"),
        "the file it came out of has to be on the record: {out}"
    );
    // Everything the live surface prints, it still prints.
    assert!(out.contains("tags: #legacy"), "{out}");
    assert!(out.contains("M-900.1"), "subtasks too: {out}");
}

/// A whole track moved by `fr track archive` is the other archive shape, and it
/// is invisible to `load_project` for a different reason: the config row's
/// `file` field still says `tracks/<id>.md` and that file is gone. Its tasks
/// need not be done, so this one is still `[ ]`.
#[test]
fn show_falls_back_to_an_archived_track() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());
    run_fr_ok(tmp.path(), &["track", "archive", "side"]);

    let out = run_fr_ok(tmp.path(), &["show", "S-001"]);
    assert!(out.contains("Side task one"), "{out}");
    assert!(
        out.starts_with("[ ]"),
        "state is preserved, not implied: {out}"
    );
    assert!(
        out.contains("archived: side (frame/archive/_tracks/side.md)"),
        "the path is what tells the two archive shapes apart: {out}"
    );
}

/// The live copy wins. An interrupted `fr clean` leaves the same id in both
/// places, and the live one is what every write command acts on — showing the
/// archived record would describe something no command can reach.
#[test]
fn show_prefers_the_live_task_over_an_archived_copy() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());
    write_archive(
        tmp.path(),
        "main",
        "# Archive — main\n\n- [x] `M-001` Stale archived copy\n  - resolved: 2025-02-03\n",
    );

    let out = run_fr_ok(tmp.path(), &["show", "M-001"]);
    assert!(out.contains("First task"), "{out}");
    assert!(!out.contains("Stale archived copy"), "{out}");
    assert!(
        !out.contains("archived:"),
        "a live task says nothing: {out}"
    );
}

#[test]
fn show_no_archive_opts_out_of_the_fallback() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());
    write_archive(tmp.path(), "main", ARCHIVED_MAIN);

    let (_out, stderr, success) = run_fr(tmp.path(), &["show", "--no-archive", "M-900"]);
    assert!(!success);
    assert!(stderr.contains("task not found: M-900"), "{stderr}");
}

/// `--context` resolves the parent chain in whatever container the task came
/// out of, and names the archive once — under the task, not under each ancestor.
#[test]
fn show_context_resolves_ancestors_inside_the_archive() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());
    write_archive(tmp.path(), "main", ARCHIVED_MAIN);

    let out = run_fr_ok(tmp.path(), &["show", "M-900.1", "--context"]);
    assert!(out.contains("── Parent ── M-900 Archived widget"), "{out}");
    assert!(out.contains("── Task ── M-900.1 Archived subtask"), "{out}");
    assert_eq!(
        out.lines()
            .filter(|l| l.trim_start().starts_with("archived:"))
            .count(),
        1,
        "{out}"
    );
}

/// `--json` carries the same two strings the human `archived:` line composes,
/// and a live task carries no `archived` key at all — absent, not null, so an
/// existing consumer sees the bytes it always saw.
#[test]
fn show_json_reports_where_an_archived_task_lives() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());
    write_archive(tmp.path(), "main", ARCHIVED_MAIN);

    let out = run_fr_ok(tmp.path(), &["show", "M-900", "--json"]);
    let parsed: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(parsed["id"], "M-900");
    assert_eq!(parsed["archived"]["track"], "main");
    assert_eq!(parsed["archived"]["file"], "frame/archive/main.md");

    let live = run_fr_ok(tmp.path(), &["show", "M-001", "--json"]);
    let parsed: serde_json::Value = serde_json::from_str(&live).unwrap();
    assert!(parsed.get("archived").is_none(), "{live}");
}

/// A write command still refuses — an archived task is not editable — but it
/// answers the question the bare message provoked: I can see it in the file.
#[test]
fn a_write_command_says_where_the_archived_task_went() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());
    write_archive(tmp.path(), "main", ARCHIVED_MAIN);

    let (_out, stderr, success) = run_fr(tmp.path(), &["title", "M-900", "New title"]);
    assert!(!success);
    assert!(
        stderr.contains("archived in main") && stderr.contains("frame/archive/main.md"),
        "{stderr}"
    );
    assert!(stderr.contains("fr show M-900"), "{stderr}");

    // A genuinely absent id keeps the short message.
    let (_out, stderr, success) = run_fr(tmp.path(), &["title", "M-404", "New title"]);
    assert!(!success);
    assert_eq!(stderr.trim(), "error: task not found: M-404", "{stderr}");
}

#[test]
fn test_ready() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());

    let out = run_fr_ok(tmp.path(), &["ready"]);
    // M-001 is todo with no deps → ready
    assert!(out.contains("M-001"));
    // M-002 is active, not todo → not ready
    assert!(!out.contains("M-002"));
    // S-001 is todo with no deps → ready
    assert!(out.contains("S-001"));
}

#[test]
fn test_ready_cc() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());

    // Add a #cc todo task to the side track
    let frame_dir = tmp.path().join("frame");
    fs::write(
        frame_dir.join("tracks/side.md"),
        "\
# Side Track

## Backlog

- [ ] `S-001` Side task one
  - added: 2025-05-01
- [ ] `S-002` Side task two #cc
  - added: 2025-05-02

## Done
",
    )
    .unwrap();

    let out = run_fr_ok(tmp.path(), &["ready", "--cc"]);
    // S-002 is todo with #cc tag → ready (cross-track scan)
    assert!(out.contains("S-002"));
    // M-001 is ready but not cc-tagged → excluded
    assert!(!out.contains("M-001"));
    // M-002 is cc-tagged but active (not todo) → excluded
    assert!(!out.contains("M-002"));
    // S-001 is todo but not cc-tagged → excluded
    assert!(!out.contains("S-001"));
}

#[test]
fn test_ready_cc_no_focus() {
    let tmp = tempfile::TempDir::new().unwrap();
    let frame_dir = tmp.path().join("frame");
    fs::create_dir_all(frame_dir.join("tracks")).unwrap();

    // Project without cc_focus set
    fs::write(
        frame_dir.join("project.toml"),
        r#"[project]
name = "test-project"

[[tracks]]
id = "main"
name = "Main Track"
state = "active"
file = "tracks/main.md"

[ids.prefixes]
main = "M"
"#,
    )
    .unwrap();

    fs::write(
        frame_dir.join("tracks/main.md"),
        "\
# Main Track

## Backlog

- [ ] `M-001` Task with cc #cc
  - added: 2025-05-01

## Done
",
    )
    .unwrap();

    fs::write(frame_dir.join("inbox.md"), "# Inbox\n").unwrap();

    // Should work without cc_focus (no error)
    let out = run_fr_ok(tmp.path(), &["ready", "--cc"]);
    assert!(out.contains("M-001"));
}

#[test]
fn test_ready_cc_ordering() {
    let tmp = tempfile::TempDir::new().unwrap();
    let frame_dir = tmp.path().join("frame");
    fs::create_dir_all(frame_dir.join("tracks")).unwrap();

    fs::write(
        frame_dir.join("project.toml"),
        r#"[project]
name = "test-project"

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

[ids.prefixes]
main = "M"
side = "S"
"#,
    )
    .unwrap();

    fs::write(
        frame_dir.join("tracks/main.md"),
        "\
# Main Track

## Backlog

- [ ] `M-001` Main cc task #cc
  - added: 2025-05-01

## Done
",
    )
    .unwrap();

    fs::write(
        frame_dir.join("tracks/side.md"),
        "\
# Side Track

## Backlog

- [ ] `S-001` Side cc task #cc
  - added: 2025-05-01

## Done
",
    )
    .unwrap();

    fs::write(frame_dir.join("inbox.md"), "# Inbox\n").unwrap();

    let out = run_fr_ok(tmp.path(), &["ready", "--cc", "--json"]);
    let parsed: serde_json::Value = serde_json::from_str(&out).unwrap();
    let tasks = parsed["tasks"].as_array().unwrap();
    assert_eq!(tasks.len(), 2);
    // Focus track (main) tasks should appear first
    assert_eq!(tasks[0]["track"].as_str().unwrap(), "main");
    assert_eq!(tasks[1]["track"].as_str().unwrap(), "side");
}

#[test]
fn test_track_cc_focus_clear() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());

    // Clear cc-focus
    let out = run_fr_ok(tmp.path(), &["track", "cc-focus", "--clear"]);
    assert!(out.contains("cleared"));

    // The key stays and is emptied, which is what the template documents as
    // meaning none — removing it would take its trailing comment with it, and
    // would make re-focusing re-add the key at the end of `[agent]`. What must
    // be gone is the *focus*, not the line.
    let config_text = fs::read_to_string(tmp.path().join("frame/project.toml")).unwrap();
    assert!(config_text.contains(r#"cc_focus = """#));
    let config: frame::model::ProjectConfig = toml::from_str(&config_text).unwrap();
    assert!(config.agent.cc_focus.is_none());

    // fr ready --cc should still work (no error)
    let _out = run_fr_ok(tmp.path(), &["ready", "--cc"]);
}

/// `fr track mv` was the one CLI command that wrote the config by serializing
/// the struct, and it took the file's comments — and any key `ProjectConfig`
/// does not model — with it every time. On a project made by `fr init` that was
/// 107 lines down to 51.
#[test]
fn test_track_mv_keeps_comments_and_unmodelled_keys() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());
    let path = tmp.path().join("frame/project.toml");

    let annotated = format!(
        "# The project's own notes\n{}\n[experimental]\nnot_in_the_struct = true\n",
        fs::read_to_string(&path).unwrap()
    );
    fs::write(&path, &annotated).unwrap();

    run_fr_ok(tmp.path(), &["track", "mv", "side", "0"]);

    let after = fs::read_to_string(&path).unwrap();
    assert!(
        after.contains("# The project's own notes"),
        "the comment was erased by the reorder:\n{after}"
    );
    assert!(
        after.contains("not_in_the_struct = true"),
        "a key the struct does not model was erased by the reorder:\n{after}"
    );

    let config: frame::model::ProjectConfig = toml::from_str(&after).unwrap();
    let ids: Vec<&str> = config.tracks.iter().map(|t| t.id.as_str()).collect();
    assert_eq!(
        ids,
        vec!["side", "main"],
        "the reorder itself must still work"
    );
}

#[test]
fn test_ready_json() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());

    let out = run_fr_ok(tmp.path(), &["ready", "--json"]);
    let parsed: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert!(parsed["tasks"].is_array());
}

#[test]
fn test_blocked() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());

    let out = run_fr_ok(tmp.path(), &["blocked"]);
    // No tasks are in blocked state in our test data
    assert!(out.is_empty() || !out.contains("M-"));
}

#[test]
fn test_search() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());

    let out = run_fr_ok(tmp.path(), &["search", "subtasks"]);
    assert!(out.contains("M-003"));
}

#[test]
fn test_search_with_track_filter() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());

    let out = run_fr_ok(tmp.path(), &["search", "task", "--track", "side"]);
    assert!(out.contains("S-001"));
    assert!(!out.contains("M-001"));
}

#[test]
fn test_inbox_list() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());

    let out = run_fr_ok(tmp.path(), &["inbox"]);
    assert!(out.contains("Bug in parser"));
    assert!(out.contains("Think about design"));
    assert!(out.contains("Quick note"));
}

#[test]
fn test_inbox_json() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());

    let out = run_fr_ok(tmp.path(), &["inbox", "--json"]);
    let parsed: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert!(parsed.is_array());
    let arr = parsed.as_array().unwrap();
    assert_eq!(arr.len(), 3);
    assert_eq!(arr[0]["title"], "Bug in parser");
    assert!(
        arr[0]["tags"]
            .as_array()
            .unwrap()
            .contains(&serde_json::json!("bug"))
    );
}

#[test]
fn test_tracks() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());

    let out = run_fr_ok(tmp.path(), &["tracks"]);
    assert!(out.contains("Main Track"));
    assert!(out.contains("Side Track"));
}

#[test]
fn test_tracks_json() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());

    let out = run_fr_ok(tmp.path(), &["tracks", "--json"]);
    let parsed: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert!(parsed.is_array());
    let arr = parsed.as_array().unwrap();
    assert_eq!(arr.len(), 2);
}

#[test]
fn test_stats() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());

    let out = run_fr_ok(tmp.path(), &["stats"]);
    assert!(out.contains("Main Track"));
    assert!(out.contains("Total"));
}

#[test]
fn test_stats_json() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());

    let out = run_fr_ok(tmp.path(), &["stats", "--json"]);
    let parsed: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert!(parsed["totals"].is_object());
}

#[test]
fn test_recent() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());

    let out = run_fr_ok(tmp.path(), &["recent"]);
    assert!(out.contains("M-000"));
    assert!(out.contains("Setup project"));
}

#[test]
fn test_deps() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());

    let out = run_fr_ok(tmp.path(), &["deps", "M-002"]);
    assert!(out.contains("M-002"));
    assert!(out.contains("M-001"));
}

/// A diamond -- two tasks depending on the same third -- is the ordinary shape
/// of a real backlog. `fr deps` used to report the second path as `(circular)`,
/// because one `visited` set spanned the whole traversal and was never popped.
#[test]
fn deps_reports_a_shared_dependency_as_already_shown() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());
    write_track(
        tmp.path(),
        "main",
        "# Main Track\n\n## Backlog\n\n\
         - [ ] `M-001` Root\n  - dep: M-002, M-003\n\
         - [ ] `M-002` Left\n  - dep: M-004\n\
         - [ ] `M-003` Right\n  - dep: M-004\n\
         - [ ] `M-004` Shared leaf\n\n## Done\n",
    );

    let out = run_fr_ok(tmp.path(), &["deps", "M-001"]);
    assert!(
        out.contains("M-004 (already shown)"),
        "expected the second path to M-004 to be marked as a repeat:\n{out}"
    );
    assert!(
        !out.contains("(circular)"),
        "a diamond is not a cycle:\n{out}"
    );
    // The first path still expands it in full.
    assert!(out.contains("[ ] M-004 Shared leaf"), "{out}");
}

/// A cycle through the root used to run one lap too far, because the root id
/// was never seeded into the visited set.
#[test]
fn deps_stops_a_cycle_at_the_root() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());
    write_track(
        tmp.path(),
        "main",
        "# Main Track\n\n## Backlog\n\n\
         - [ ] `M-005` Cycle a\n  - dep: M-006\n\
         - [ ] `M-006` Cycle b\n  - dep: M-005\n\n## Done\n",
    );

    let out = run_fr_ok(tmp.path(), &["deps", "M-005"]);
    assert!(out.contains("M-005 (circular)"), "{out}");
    // Three lines: the root, M-006, and the cycle marker. A fourth means the
    // root was expanded a second time.
    assert_eq!(
        out.lines().filter(|l| !l.trim().is_empty()).count(),
        3,
        "{out}"
    );
}

#[test]
fn deps_reports_a_dangling_dependency_as_not_found() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());
    write_track(
        tmp.path(),
        "main",
        "# Main Track\n\n## Backlog\n\n- [ ] `M-007` Dangling\n  - dep: M-999\n\n## Done\n",
    );

    let out = run_fr_ok(tmp.path(), &["deps", "M-007"]);
    assert!(out.contains("M-999 (not found)"), "{out}");
}

/// Archives are searched by default; `--no-archive` is the opt-out. The old
/// `--archive` flag was declared and never read, so it did nothing either way.
#[test]
fn search_archive_is_opt_out_not_opt_in() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());
    let archive = tmp.path().join("frame").join("archive");
    fs::create_dir_all(&archive).unwrap();
    fs::write(
        archive.join("main.md"),
        "# Main Track — Archive\n\n## Done\n\n- [x] `M-900` Archived widget\n  - resolved: 2024-01-02\n",
    )
    .unwrap();

    let out = run_fr_ok(tmp.path(), &["search", "widget"]);
    assert!(out.contains("[archive:main]"), "{out}");
    assert!(out.contains("M-900"), "{out}");

    let out = run_fr_ok(tmp.path(), &["search", "--no-archive", "widget"]);
    assert!(
        !out.contains("M-900"),
        "--no-archive should skip it:\n{out}"
    );

    let (_, stderr, ok) = run_fr(tmp.path(), &["search", "-a", "widget"]);
    assert!(!ok, "the removed -a flag should be an error, not a no-op");
    assert!(stderr.contains("unexpected argument"), "{stderr}");
}

/// `matched_fields` lists every field that matched, not whichever one the scan
/// reached first. Searching for an id matches the task by `id` and its
/// dependents by `dep`.
#[test]
fn search_json_reports_all_matched_fields() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());

    let out = run_fr_ok(tmp.path(), &["--json", "search", "M-001"]);
    let parsed: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(parsed["pattern"], "M-001");

    let tasks = parsed["tasks"].as_array().unwrap();
    let by_id = |id: &str| {
        tasks
            .iter()
            .find(|t| t["id"] == id)
            .unwrap_or_else(|| panic!("{id} missing from {out}"))
    };
    assert_eq!(by_id("M-001")["matched_fields"][0], "id");
    assert_eq!(by_id("M-002")["matched_fields"][0], "dep");

    // The three arrays are always present, so a consumer never has to test for
    // a key's existence.
    assert!(parsed["archived"].is_array());
    assert!(parsed["inbox"].is_array());
}

#[test]
fn test_check() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());

    let out = run_fr_ok(tmp.path(), &["check"]);
    assert!(out.contains("valid"));
}

#[test]
fn test_check_json() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());

    let out = run_fr_ok(tmp.path(), &["check", "--json"]);
    let parsed: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(parsed["valid"], true);
}

// ---------------------------------------------------------------------------
// Write command tests
// ---------------------------------------------------------------------------

#[test]
fn test_add_task() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());

    let out = run_fr_ok(tmp.path(), &["add", "main", "New task from CLI"]);
    assert!(out.contains("M-011")); // Next ID after M-010

    // Verify it appears in the file
    let track = fs::read_to_string(tmp.path().join("frame/tracks/main.md")).unwrap();
    assert!(track.contains("New task from CLI"));
    assert!(track.contains("M-011"));
}

#[test]
fn test_add_task_after() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());

    let out = run_fr_ok(
        tmp.path(),
        &["add", "main", "After first", "--after", "M-001"],
    );
    assert!(out.contains("M-011"));

    // Verify position in file - should appear after M-001
    let track = fs::read_to_string(tmp.path().join("frame/tracks/main.md")).unwrap();
    let pos_001 = track.find("M-001").unwrap();
    let pos_011 = track.find("M-011").unwrap();
    let pos_002 = track.find("M-002").unwrap();
    assert!(pos_011 > pos_001);
    assert!(pos_011 < pos_002);
}

#[test]
fn test_push_task() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());

    let out = run_fr_ok(tmp.path(), &["push", "main", "Top priority task"]);
    assert!(out.contains("M-011"));

    // Verify it's at the top of backlog
    let track = fs::read_to_string(tmp.path().join("frame/tracks/main.md")).unwrap();
    let pos_011 = track.find("M-011").unwrap();
    let pos_001 = track.find("M-001").unwrap();
    assert!(pos_011 < pos_001);
}

#[test]
fn test_sub_task() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());

    let out = run_fr_ok(tmp.path(), &["sub", "M-001", "New subtask"]);
    assert!(out.contains("M-001.1"));

    let track = fs::read_to_string(tmp.path().join("frame/tracks/main.md")).unwrap();
    assert!(track.contains("M-001.1"));
    assert!(track.contains("New subtask"));
}

#[test]
fn test_state_change() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());

    let out = run_fr_ok(tmp.path(), &["state", "M-001", "active"]);
    assert!(out.contains("M-001"));
    assert!(out.contains("active"));

    // Verify file changed
    let track = fs::read_to_string(tmp.path().join("frame/tracks/main.md")).unwrap();
    // M-001 should now have [>] instead of [ ]
    assert!(track.contains("[>] `M-001`"));
}

#[test]
fn test_state_done_adds_resolved() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());

    run_fr_ok(tmp.path(), &["state", "M-001", "done"]);
    let track = fs::read_to_string(tmp.path().join("frame/tracks/main.md")).unwrap();
    assert!(track.contains("[x] `M-001`"));
    assert!(track.contains("resolved:"));
}

/// Every section a state change can start from lands in the right place.
///
/// Written as a loop over the whole (state × starting section) space rather than
/// one case per transition, because enumerating transitions is exactly how the
/// hole below got in: `cmd_state` listed `from → to` pairs and nobody listed
/// Done → Parked.
///
/// `tests/parity.rs` cannot cover this. It compares the CLI to the TUI, and both
/// surfaces had the identical gap — they agreed with each other and disagreed
/// only with `canonical_section`. Verified by reverting both fixes: the parity
/// case for this cell still passed. Agreement is not correctness, so the
/// behaviour needs asserting somewhere that knows the right answer.
#[test]
fn state_change_moves_a_task_to_the_section_its_state_calls_for() {
    // (state to set, expected section header the task must end up under)
    let expectations = [
        ("done", "## Done"),
        ("parked", "## Parked"),
        ("todo", "## Backlog"),
        ("active", "## Backlog"),
        ("blocked", "## Backlog"),
    ];
    // Each starting section, as the track content that puts M-001 there.
    let starts = [
        (
            "backlog",
            "## Backlog\n\n- [ ] `M-001` Task\n\n## Parked\n\n## Done\n",
        ),
        (
            "parked",
            "## Backlog\n\n## Parked\n\n- [~] `M-001` Task\n\n## Done\n",
        ),
        (
            "done",
            "## Backlog\n\n## Parked\n\n## Done\n\n- [x] `M-001` Task\n  - resolved: 2026-01-01\n",
        ),
    ];

    for (start_name, body) in starts {
        for (state, want_section) in expectations {
            let tmp = tempfile::TempDir::new().unwrap();
            create_test_project(tmp.path());
            let path = tmp.path().join("frame/tracks/main.md");
            fs::write(&path, format!("# Main\n\n{body}")).unwrap();

            run_fr_ok(tmp.path(), &["state", "M-001", state]);
            let track = fs::read_to_string(&path).unwrap();

            // The section the task actually landed in: the last header above it.
            let idx = track.find("`M-001`").expect("task survived");
            let landed = track[..idx]
                .rmatch_indices("## ")
                .next()
                .map(|(i, _)| track[i..].lines().next().unwrap())
                .expect("task sits under a section header");

            assert_eq!(
                landed, want_section,
                "M-001 starting in {start_name}, set to {state}, landed under {landed:?}\n{track}"
            );
        }
    }
}

#[test]
fn test_tag_add_remove() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());

    // Add tag
    run_fr_ok(tmp.path(), &["tag", "M-001", "add", "urgent"]);
    let track = fs::read_to_string(tmp.path().join("frame/tracks/main.md")).unwrap();
    assert!(track.contains("#urgent"));

    // Remove tag
    run_fr_ok(tmp.path(), &["tag", "M-001", "rm", "urgent"]);
    let track = fs::read_to_string(tmp.path().join("frame/tracks/main.md")).unwrap();
    assert!(!track.contains("#urgent"));
}

#[test]
fn test_dep_add_remove() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());

    // Add dep (use M-010 to avoid conflict with M-002's existing dep: M-001)
    run_fr_ok(tmp.path(), &["dep", "M-003", "add", "M-010"]);
    let track = fs::read_to_string(tmp.path().join("frame/tracks/main.md")).unwrap();
    assert!(track.contains("dep: M-010"));

    // Remove dep
    run_fr_ok(tmp.path(), &["dep", "M-003", "rm", "M-010"]);
    let track_content = fs::read_to_string(tmp.path().join("frame/tracks/main.md")).unwrap();
    assert!(
        !track_content.contains("dep: M-010"),
        "dep should be removed from M-003"
    );
}

#[test]
fn test_note() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());

    run_fr_ok(tmp.path(), &["note", "M-001", "This is a CLI note."]);
    let track = fs::read_to_string(tmp.path().join("frame/tracks/main.md")).unwrap();
    assert!(track.contains("This is a CLI note."));
}

#[test]
fn test_note_append() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());

    run_fr_ok(tmp.path(), &["note", "M-001", "First note."]);
    run_fr_ok(tmp.path(), &["note", "M-001", "Second note."]);
    let track = fs::read_to_string(tmp.path().join("frame/tracks/main.md")).unwrap();
    assert!(
        track.contains("First note."),
        "first note should be preserved"
    );
    assert!(
        track.contains("Second note."),
        "second note should be appended"
    );
}

/// Set `limits.note_max_bytes` on a project the helpers just built.
fn set_note_limit(root: &Path, value: &str) {
    let path = root.join("frame/project.toml");
    let text = fs::read_to_string(&path).unwrap();
    fs::write(
        &path,
        format!("{text}\n[limits]\nnote_max_bytes = {value}\n"),
    )
    .unwrap();
}

/// A duplicate `## Done` is reported as an error, and healed by the next write
/// with every task kept and in order.
#[test]
fn a_duplicate_section_is_reported_then_merged_by_the_next_write() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());
    let path = tmp.path().join("frame/tracks/main.md");
    let text = fs::read_to_string(&path).unwrap();
    fs::write(
        &path,
        format!(
            "{text}\n## Done\n\n- [x] `M-900` First hidden\n  - resolved: 2026-01-01\n\
             - [x] `M-901` Second hidden\n  - resolved: 2026-01-02\n"
        ),
    )
    .unwrap();

    let (stdout, _, ok) = run_fr(tmp.path(), &["check"]);
    assert!(
        !ok,
        "a duplicate section is an error, so check should exit 1"
    );
    assert!(
        stdout.contains("'## Done' sections"),
        "check should name it: {stdout}"
    );

    // Read-only: check must not have repaired anything.
    assert_eq!(
        fs::read_to_string(&path)
            .unwrap()
            .matches("## Done")
            .count(),
        2,
        "check is read-only"
    );

    // Any write heals it.
    run_fr_ok(tmp.path(), &["clean"]);
    let healed = fs::read_to_string(&path).unwrap();
    assert_eq!(
        healed.matches("## Done").count(),
        1,
        "the write should merge"
    );
    assert!(
        healed.contains("M-900") && healed.contains("M-901"),
        "no task lost"
    );
    assert!(
        healed.find("M-900").unwrap() < healed.find("M-901").unwrap(),
        "relative order preserved"
    );
    run_fr_ok(tmp.path(), &["check"]);
}

/// A heading frame cannot read is an error even when nothing is behind it —
/// anything written under it would stop being a task.
#[test]
fn an_unknown_heading_is_an_error() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());
    let path = tmp.path().join("frame/tracks/main.md");
    let text = fs::read_to_string(&path).unwrap();
    fs::write(&path, format!("{text}\n## Someday\n")).unwrap();

    let (stdout, _, ok) = run_fr(tmp.path(), &["check"]);
    assert!(!ok, "an unknown heading is an error");
    assert!(
        stdout.contains("Someday"),
        "check should name the heading: {stdout}"
    );
}

/// A `##` inside an inbox item body or a task note is prose, not a heading.
///
/// The first cut of this scanned raw lines with a `trim()`, and reported five
/// findings against a real project's inbox — every one of them a markdown
/// heading someone had written inside an item body, indented two spaces. Body
/// text is freeform and frame does not get to object to what is in it.
#[test]
fn a_heading_inside_a_body_is_not_an_unknown_heading() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());

    let inbox = tmp.path().join("frame/inbox.md");
    let text = fs::read_to_string(&inbox).unwrap();
    fs::write(
        &inbox,
        format!(
            "{text}\n- An item with a structured body\n  ## Reproducer (minimal)\n  some detail\n"
        ),
    )
    .unwrap();

    run_fr_ok(
        tmp.path(),
        &["note", "M-001", "See below.\n\n## Findings\n\nDetail."],
    );

    let (stdout, _, ok) = run_fr(tmp.path(), &["check"]);
    assert!(
        ok,
        "a heading inside body text must not be reported: {stdout}"
    );
    assert!(!stdout.contains("Reproducer"), "{stdout}");
    assert!(!stdout.contains("Findings"), "{stdout}");
}

/// The whole-note re-append, caught on the second one. This is the shape that
/// took a real note to eight copies of itself.
#[test]
fn appending_text_the_note_already_holds_is_refused() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());
    let para = "A".repeat(200);

    run_fr_ok(tmp.path(), &["note", "M-001", &para]);
    let (_, stderr, ok) = run_fr(tmp.path(), &["note", "M-001", &para]);

    assert!(!ok, "re-appending the same paragraph should be refused");
    assert!(
        stderr.contains("--replace"),
        "the message must point at --replace: {stderr}"
    );
    let track = fs::read_to_string(tmp.path().join("frame/tracks/main.md")).unwrap();
    assert_eq!(
        track.matches(&para).count(),
        1,
        "the paragraph should appear exactly once"
    );
}

/// The realistic case: the agent rewrites the note, keeping most of it and
/// adding a paragraph, then appends the lot. The unchanged part is what trips.
#[test]
fn appending_a_grown_copy_of_the_note_is_refused() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());
    let first = "B".repeat(200);

    run_fr_ok(tmp.path(), &["note", "M-001", &first]);
    let grown = format!("{first}\n\nAnd here is what I learned since.");
    let (_, _, ok) = run_fr(tmp.path(), &["note", "M-001", &grown]);
    assert!(!ok, "a grown copy of the note should be refused");

    // --replace is the way through, and it lands.
    run_fr_ok(tmp.path(), &["note", "M-001", &grown, "--replace"]);
    let track = fs::read_to_string(tmp.path().join("frame/tracks/main.md")).unwrap();
    assert_eq!(track.matches(&first).count(), 1);
    assert!(track.contains("what I learned since"));
}

/// Genuinely new text still appends, and a short repeated fragment — a code
/// line, an error string — is not enough to refuse a write.
#[test]
fn new_text_and_short_repeats_still_append() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());

    run_fr_ok(tmp.path(), &["note", "M-001", &"C".repeat(200)]);
    run_fr_ok(
        tmp.path(),
        &["note", "M-001", "A genuinely different finding."],
    );
    // Below limits.note_repeat_bytes (120), so not a repeat as far as this cares.
    run_fr_ok(tmp.path(), &["note", "M-001", "short line"]);
    run_fr_ok(tmp.path(), &["note", "M-001", "short line"]);

    let track = fs::read_to_string(tmp.path().join("frame/tracks/main.md")).unwrap();
    assert_eq!(track.matches("short line").count(), 2);
}

#[test]
fn note_over_the_limit_is_refused_and_writes_nothing() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());
    set_note_limit(tmp.path(), "200");

    let (_, stderr, ok) = run_fr(tmp.path(), &["note", "M-001", &"x".repeat(300)]);
    assert!(!ok, "an over-limit note should exit non-zero");
    assert!(
        stderr.contains("note_max_bytes"),
        "the message should name the knob: {stderr}"
    );

    let track = fs::read_to_string(tmp.path().join("frame/tracks/main.md")).unwrap();
    assert!(
        !track.contains("xxx"),
        "a refused write must leave the file untouched"
    );
}

/// Appending is how notes get large, so an append that would cross the limit is
/// refused even though each piece on its own is small.
#[test]
fn note_append_that_would_cross_the_limit_is_refused() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());
    set_note_limit(tmp.path(), "200");

    run_fr_ok(tmp.path(), &["note", "M-001", &"a".repeat(150)]);
    let (_, _, ok) = run_fr(tmp.path(), &["note", "M-001", &"b".repeat(100)]);
    assert!(!ok, "the append crosses 200 bytes and should be refused");

    let track = fs::read_to_string(tmp.path().join("frame/tracks/main.md")).unwrap();
    assert!(track.contains(&"a".repeat(150)), "the first note survives");
    assert!(!track.contains("bbb"), "the refused append wrote nothing");
}

/// The case an absolute `new_len <= limit` check would get wrong, and the reason
/// the rule is non-increasing: a note that predates the limit must stay
/// editable, and must be reachable in more than one pass. Lowering the limit
/// under an existing note is exactly how every project meets this feature.
#[test]
fn a_note_already_over_the_limit_can_still_be_shortened() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());

    // Written while the limit is generous...
    set_note_limit(tmp.path(), "4000");
    run_fr_ok(tmp.path(), &["note", "M-001", &"x".repeat(3000)]);

    // ...then the limit drops well below it.
    let path = tmp.path().join("frame/project.toml");
    let text = fs::read_to_string(&path).unwrap();
    fs::write(
        &path,
        text.replace("note_max_bytes = 4000", "note_max_bytes = 200"),
    )
    .unwrap();

    // Growing it is refused.
    let (_, _, grew) = run_fr(tmp.path(), &["note", "M-001", "more"]);
    assert!(!grew, "an over-limit note must not be grown");

    // Shrinking it is allowed even though the result is still over the limit —
    // otherwise the only legal edit is one that lands under 200 bytes in a
    // single shot, and a long note could never be worked down.
    run_fr_ok(
        tmp.path(),
        &["note", "M-001", &"y".repeat(1000), "--replace"],
    );
    let track = fs::read_to_string(tmp.path().join("frame/tracks/main.md")).unwrap();
    assert!(track.contains(&"y".repeat(1000)), "the shrink should land");
    assert!(!track.contains("xxx"), "the long note should be gone");

    // And the rest of the way down, under the limit, still works.
    run_fr_ok(tmp.path(), &["note", "M-001", "short", "--replace"]);
}

#[test]
fn note_limit_can_be_switched_off() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());
    set_note_limit(tmp.path(), "\"off\"");

    run_fr_ok(tmp.path(), &["note", "M-001", &"x".repeat(50_000)]);
    let track = fs::read_to_string(tmp.path().join("frame/tracks/main.md")).unwrap();
    assert!(track.contains(&"x".repeat(50_000)));
}

/// Operations that do not lengthen the note must not be caught by the limit.
#[test]
fn an_oversize_note_does_not_block_other_edits() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());
    set_note_limit(tmp.path(), "\"off\"");
    run_fr_ok(tmp.path(), &["note", "M-001", &"x".repeat(5000)]);

    let path = tmp.path().join("frame/project.toml");
    let text = fs::read_to_string(&path).unwrap();
    fs::write(
        &path,
        text.replace("note_max_bytes = \"off\"", "note_max_bytes = 100"),
    )
    .unwrap();

    run_fr_ok(tmp.path(), &["state", "M-001", "done"]);
    run_fr_ok(tmp.path(), &["check"]);
}

#[test]
fn test_note_replace() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());

    run_fr_ok(tmp.path(), &["note", "M-001", "First note."]);
    run_fr_ok(
        tmp.path(),
        &["note", "M-001", "Replacement note.", "--replace"],
    );
    let track = fs::read_to_string(tmp.path().join("frame/tracks/main.md")).unwrap();
    assert!(
        !track.contains("First note."),
        "first note should be replaced"
    );
    assert!(track.contains("Replacement note."));
}

/// A bare `-` is the reflex spelling for "read from stdin", and frame has no
/// stdin form — so it arrived as note text and stored itself over the note.
/// Real incident: `fr note ID --replace -` left a note reading `-`.
#[test]
fn a_bare_dash_is_refused_as_note_text() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());
    let note = "A".repeat(200);
    run_fr_ok(tmp.path(), &["note", "M-001", &note]);

    let (_, stderr, ok) = run_fr(tmp.path(), &["note", "M-001", "-", "--replace"]);

    assert!(!ok, "a bare - should be refused, not stored");
    assert!(
        stderr.contains("--file"),
        "the message must offer the way to pass real text: {stderr}"
    );
    let track = fs::read_to_string(tmp.path().join("frame/tracks/main.md")).unwrap();
    assert!(track.contains(&note), "the note must survive");
}

/// `--` is the options terminator, so `fr note ID -- --replace` fed `--replace`
/// to the text argument *and* dropped the flag: the note gained the literal
/// string `--replace` and was appended to rather than replaced.
#[test]
fn a_swallowed_flag_is_refused_as_note_text() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());
    let note = "B".repeat(200);
    run_fr_ok(tmp.path(), &["note", "M-001", &note]);

    let (_, _, ok) = run_fr(tmp.path(), &["note", "M-001", "--", "--replace"]);

    assert!(!ok, "a swallowed flag should be refused, not stored");
    let track = fs::read_to_string(tmp.path().join("frame/tracks/main.md")).unwrap();
    assert!(!track.contains("--replace"), "the flag must not be stored");
    assert!(track.contains(&note), "the note must survive");
}

/// The rule is an exact-match list, not "starts with `-`": a note is markdown,
/// and a markdown bullet list starts with `-`. `--file` is how it gets in,
/// since the argument parser reads a leading `-` as a flag.
#[test]
fn a_markdown_bullet_list_lands_through_file() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());
    let src = tmp.path().join("bullets.md");
    fs::write(&src, "- found it in layout.rs\n- fix is one line\n").unwrap();

    run_fr_ok(
        tmp.path(),
        &["note", "M-001", "--file", src.to_str().unwrap()],
    );

    let track = fs::read_to_string(tmp.path().join("frame/tracks/main.md")).unwrap();
    assert!(track.contains("- found it in layout.rs"));
    assert!(track.contains("- fix is one line"));
}

/// A file with nothing in it is an upstream step that produced nothing, not a
/// request to blank the note — and under `--replace` those differ by the whole
/// note.
#[test]
fn an_empty_file_is_refused() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());
    let note = "C".repeat(200);
    run_fr_ok(tmp.path(), &["note", "M-001", &note]);
    let src = tmp.path().join("empty.md");
    fs::write(&src, "\n  \n").unwrap();

    let (_, _, ok) = run_fr(
        tmp.path(),
        &[
            "note",
            "M-001",
            "--file",
            src.to_str().unwrap(),
            "--replace",
        ],
    );

    assert!(!ok, "an empty file should be refused");
    let track = fs::read_to_string(tmp.path().join("frame/tracks/main.md")).unwrap();
    assert!(track.contains(&note), "the note must survive");
}

/// One trailing newline goes, because every editor writes one; the note should
/// not begin life with a blank line on the end.
#[test]
fn file_note_drops_one_trailing_newline() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());
    let src = tmp.path().join("n.md");
    fs::write(&src, "the finding\n").unwrap();

    let out = run_fr_ok(
        tmp.path(),
        &[
            "--json",
            "note",
            "M-001",
            "--file",
            src.to_str().unwrap(),
            "--replace",
        ],
    );
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["tasks"][0]["note"], "the finding");
}

/// A replacement that discards something says what it discarded — the result
/// line is otherwise identical whether one byte or a thousand words landed, so
/// there is nothing to notice while the change is still uncommitted.
#[test]
fn a_replacement_reports_what_it_displaced() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());
    run_fr_ok(tmp.path(), &["note", "M-001", &"D".repeat(200)]);

    let stdout = run_fr_ok(tmp.path(), &["note", "M-001", "short", "--replace"]);
    assert!(
        stdout.contains("note replaced") && stdout.contains("200B"),
        "the result line must name what went: {stdout}"
    );

    // Distinct text, not a superset: text containing the old note verbatim is a
    // read-modify-write and displaces nothing, which the next test covers.
    let out = run_fr_ok(
        tmp.path(),
        &["--json", "note", "M-001", "different", "--replace"],
    );
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["displaced_bytes"], 5);
}

/// A note that survives verbatim inside the new text displaced nothing — that
/// is a read-modify-write, the shape `--replace` is supposed to have.
#[test]
fn a_read_modify_write_displaces_nothing() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());
    let first = "E".repeat(200);
    run_fr_ok(tmp.path(), &["note", "M-001", &first]);

    let grown = format!("{first}\n\nAnd what I learned since.");
    let out = run_fr_ok(
        tmp.path(),
        &["--json", "note", "M-001", &grown, "--replace"],
    );
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert!(
        v.get("displaced_bytes").is_none(),
        "nothing was displaced: {v}"
    );
    assert!(v["warnings"].as_array().is_none_or(|w| w.is_empty()));
}

/// A few bytes over a substantial note is what a flag or a filename looks like
/// once stored. The write still goes through — discarding a note is what
/// `--replace` is for — but the caller is told.
#[test]
fn a_clobber_shaped_replacement_warns() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());
    run_fr_ok(tmp.path(), &["note", "M-001", &"F".repeat(200)]);

    let (stdout, stderr, ok) = run_fr(tmp.path(), &["note", "M-001", "wip", "--replace"]);
    assert!(ok, "the write still goes through: {stderr}");
    assert!(stdout.contains("note replaced"));
    assert!(
        stderr.contains("warning:"),
        "a clobber-shaped write must warn: {stderr}"
    );

    // And the same warning reaches a program, not just a terminal.
    run_fr_ok(
        tmp.path(),
        &["note", "M-001", &"G".repeat(200), "--replace"],
    );
    let out = run_fr_ok(tmp.path(), &["--json", "note", "M-001", "wip", "--replace"]);
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["warnings"].as_array().map(Vec::len), Some(1));
}

/// The bounds are far apart on purpose: shortening a note by hand is the
/// workflow the non-increasing size rule exists to protect, and it must stay
/// quiet unless it lands in the clobber shape.
#[test]
fn an_ordinary_short_supersede_does_not_warn() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());
    run_fr_ok(tmp.path(), &["note", "M-001", &"H".repeat(200)]);

    let (stdout, stderr, ok) = run_fr(
        tmp.path(),
        &[
            "note",
            "M-001",
            "Superseded: see doc/design.md, which now carries the whole rationale.",
            "--replace",
        ],
    );
    assert!(ok);
    assert!(stdout.contains("note replaced"), "the cost is still named");
    assert!(
        !stderr.contains("warning:"),
        "a real supersede must not warn: {stderr}"
    );
}

/// A preview must show the cost too — it is the one place a caller can look
/// before the write happens, and it said only which file would change.
#[test]
fn a_dry_run_replacement_reports_the_cost() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());
    let note = "I".repeat(200);
    run_fr_ok(tmp.path(), &["note", "M-001", &note]);

    let stdout = run_fr_ok(
        tmp.path(),
        &["note", "M-001", "oops", "--replace", "--dry-run"],
    );
    assert!(
        stdout.contains("note replaced") && stdout.contains("200B"),
        "a preview must name the cost: {stdout}"
    );
    let track = fs::read_to_string(tmp.path().join("frame/tracks/main.md")).unwrap();
    assert!(track.contains(&note), "a preview writes nothing");
}

/// `doc/design.md`, `doc/spec.md` and `src/parser.rs`, for the ref/spec tests —
/// a path must exist before frame will point at it.
fn create_ref_targets(root: &Path) {
    fs::create_dir_all(root.join("doc")).unwrap();
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(root.join("doc/design.md"), "# Design\n").unwrap();
    fs::write(root.join("doc/spec.md"), "# Spec\n").unwrap();
    fs::write(root.join("src/parser.rs"), "fn main() {}\n").unwrap();
}

/// `ref` and `spec` take the same actions and behave identically under each,
/// so the cases below run against both. Only the metadata key differs.
const PATH_FIELDS: [&str; 2] = ["ref", "spec"];

#[test]
fn test_path_field_add() {
    for field in PATH_FIELDS {
        let tmp = tempfile::TempDir::new().unwrap();
        create_test_project(tmp.path());
        create_ref_targets(tmp.path());

        run_fr_ok(tmp.path(), &[field, "M-001", "add", "doc/design.md"]);
        run_fr_ok(tmp.path(), &[field, "M-001", "add", "src/parser.rs"]);

        let track = fs::read_to_string(tmp.path().join("frame/tracks/main.md")).unwrap();
        assert!(
            track.contains(&format!("{}: doc/design.md, src/parser.rs", field)),
            "{} add did not append:\n{}",
            field,
            track
        );
    }
}

#[test]
fn test_path_field_add_is_idempotent() {
    for field in PATH_FIELDS {
        let tmp = tempfile::TempDir::new().unwrap();
        create_test_project(tmp.path());
        create_ref_targets(tmp.path());

        run_fr_ok(tmp.path(), &[field, "M-001", "add", "doc/design.md"]);
        let out = run_fr_ok(tmp.path(), &[field, "M-001", "add", "doc/design.md"]);
        assert!(out.contains("unchanged"), "{}: {}", field, out);

        let track = fs::read_to_string(tmp.path().join("frame/tracks/main.md")).unwrap();
        assert_eq!(
            track.matches("doc/design.md").count(),
            1,
            "{} duplicated a path",
            field
        );
    }
}

#[test]
fn test_path_field_set_replaces_the_list() {
    for field in PATH_FIELDS {
        let tmp = tempfile::TempDir::new().unwrap();
        create_test_project(tmp.path());
        create_ref_targets(tmp.path());

        run_fr_ok(tmp.path(), &[field, "M-001", "add", "doc/design.md"]);
        run_fr_ok(
            tmp.path(),
            &[
                field,
                "M-001",
                "set",
                "doc/spec.md#section",
                "src/parser.rs",
            ],
        );

        let track = fs::read_to_string(tmp.path().join("frame/tracks/main.md")).unwrap();
        assert!(track.contains(&format!("{}: doc/spec.md#section, src/parser.rs", field)));
        assert!(
            !track.contains("doc/design.md"),
            "{} set did not replace",
            field
        );
    }
}

#[test]
fn test_path_field_rm() {
    for field in PATH_FIELDS {
        let tmp = tempfile::TempDir::new().unwrap();
        create_test_project(tmp.path());
        create_ref_targets(tmp.path());

        run_fr_ok(
            tmp.path(),
            &[field, "M-001", "add", "doc/design.md", "src/parser.rs"],
        );
        run_fr_ok(tmp.path(), &[field, "M-001", "rm", "doc/design.md"]);

        let track = fs::read_to_string(tmp.path().join("frame/tracks/main.md")).unwrap();
        assert!(track.contains(&format!("{}: src/parser.rs", field)));
        assert!(!track.contains("doc/design.md"));
    }
}

/// Removing the last path takes the metadata line with it.
#[test]
fn test_path_field_rm_last_removes_the_line() {
    for field in PATH_FIELDS {
        let tmp = tempfile::TempDir::new().unwrap();
        create_test_project(tmp.path());
        create_ref_targets(tmp.path());

        run_fr_ok(tmp.path(), &[field, "M-001", "add", "doc/design.md"]);
        run_fr_ok(tmp.path(), &[field, "M-001", "rm", "doc/design.md"]);

        let track = fs::read_to_string(tmp.path().join("frame/tracks/main.md")).unwrap();
        assert!(
            !track.contains(&format!("{}:", field)),
            "{} left an empty line:\n{}",
            field,
            track
        );
    }
}

/// `rm` never checks the filesystem: a path is most worth removing precisely
/// when the file behind it is gone.
#[test]
fn test_path_field_rm_works_after_the_file_is_deleted() {
    for field in PATH_FIELDS {
        let tmp = tempfile::TempDir::new().unwrap();
        create_test_project(tmp.path());
        create_ref_targets(tmp.path());

        run_fr_ok(tmp.path(), &[field, "M-001", "add", "doc/design.md"]);
        fs::remove_file(tmp.path().join("doc/design.md")).unwrap();

        run_fr_ok(tmp.path(), &[field, "M-001", "rm", "doc/design.md"]);
        let track = fs::read_to_string(tmp.path().join("frame/tracks/main.md")).unwrap();
        assert!(!track.contains("doc/design.md"));
    }
}

#[test]
fn test_path_field_rm_of_an_absent_path_says_so() {
    for field in PATH_FIELDS {
        let tmp = tempfile::TempDir::new().unwrap();
        create_test_project(tmp.path());
        create_ref_targets(tmp.path());

        let out = run_fr_ok(tmp.path(), &[field, "M-001", "rm", "doc/design.md"]);
        assert!(out.contains("unchanged"), "{}: {}", field, out);
    }
}

/// Write a `ref:`/`spec:` value into the track file directly, bypassing the CLI.
///
/// The only way to produce a value in a spelling frame would no longer write —
/// which is exactly the case that matters, since the point of matching by normal
/// form is to reach values written before normalization existed. Any project
/// touched by an older `fr` can hold these.
fn store_raw_path_value(root: &Path, field: &str, value: &str) {
    let path = root.join("frame/tracks/main.md");
    let track = fs::read_to_string(&path).unwrap();
    let updated = track.replace(
        "- [ ] `M-001` First task #core\n  - added: 2025-05-01\n",
        &format!(
            "- [ ] `M-001` First task #core\n  - added: 2025-05-01\n  - {}: {}\n",
            field, value
        ),
    );
    assert_ne!(
        track, updated,
        "fixture shape changed; nothing was injected"
    );
    fs::write(&path, updated).unwrap();
}

/// One file has many spellings. The one that gets stored is the folded one, so
/// the list reads as a set of files rather than a set of strings.
#[test]
fn test_path_field_stores_the_normal_form() {
    for field in PATH_FIELDS {
        let tmp = tempfile::TempDir::new().unwrap();
        create_test_project(tmp.path());
        create_ref_targets(tmp.path());

        let out = run_fr_ok(
            tmp.path(),
            &[field, "M-001", "add", "./doc/../doc/design.md"],
        );
        assert!(out.contains("doc/design.md"), "{}: {}", field, out);
        assert!(
            !out.contains(".."),
            "{} echoed the raw spelling: {}",
            field,
            out
        );

        let track = fs::read_to_string(tmp.path().join("frame/tracks/main.md")).unwrap();
        assert!(
            track.contains(&format!("{}: doc/design.md", field)),
            "{} stored an unfolded path:\n{}",
            field,
            track
        );
    }
}

/// The suffix rides through normalization untouched.
#[test]
fn test_path_field_normalizes_without_disturbing_the_suffix() {
    for field in PATH_FIELDS {
        let tmp = tempfile::TempDir::new().unwrap();
        create_test_project(tmp.path());
        create_ref_targets(tmp.path());

        run_fr_ok(
            tmp.path(),
            &[
                field,
                "M-001",
                "add",
                "./doc/../doc/design.md#rationale",
                "./src/../src/parser.rs:807",
            ],
        );
        let track = fs::read_to_string(tmp.path().join("frame/tracks/main.md")).unwrap();
        assert!(
            track.contains(&format!(
                "{}: doc/design.md#rationale, src/parser.rs:807",
                field
            )),
            "{}:\n{}",
            field,
            track
        );
    }
}

/// `rm` matches by normal form, so the argument reaches the stored value
/// whichever of the two carries the awkward spelling.
#[test]
fn test_path_field_rm_matches_an_equivalent_spelling() {
    for field in PATH_FIELDS {
        // Stored folded, asked for unfolded.
        let tmp = tempfile::TempDir::new().unwrap();
        create_test_project(tmp.path());
        create_ref_targets(tmp.path());
        run_fr_ok(tmp.path(), &[field, "M-001", "add", "doc/design.md"]);
        let out = run_fr_ok(
            tmp.path(),
            &[field, "M-001", "rm", "./doc/../doc/design.md"],
        );
        assert!(out.contains("removed"), "{}: {}", field, out);
        let track = fs::read_to_string(tmp.path().join("frame/tracks/main.md")).unwrap();
        assert!(!track.contains("design.md"), "{}:\n{}", field, track);

        // Stored unfolded — a value from before normalization existed — and
        // asked for folded. This is the direction that was unreachable.
        let tmp = tempfile::TempDir::new().unwrap();
        create_test_project(tmp.path());
        create_ref_targets(tmp.path());
        store_raw_path_value(tmp.path(), field, "./doc/../doc/design.md");
        let out = run_fr_ok(tmp.path(), &[field, "M-001", "rm", "doc/design.md"]);
        assert!(out.contains("removed"), "{}: {}", field, out);
        // The message names what actually left the file, not what was typed.
        assert!(
            out.contains("./doc/../doc/design.md"),
            "{} reported the argument rather than the stored value: {}",
            field,
            out
        );
        let track = fs::read_to_string(tmp.path().join("frame/tracks/main.md")).unwrap();
        assert!(!track.contains("design.md"), "{}:\n{}", field, track);
    }
}

/// Dedup matches by normal form too, or `add` appends a second entry for a file
/// the task already carries under another spelling.
#[test]
fn test_path_field_add_does_not_duplicate_an_equivalent_spelling() {
    for field in PATH_FIELDS {
        let tmp = tempfile::TempDir::new().unwrap();
        create_test_project(tmp.path());
        create_ref_targets(tmp.path());
        store_raw_path_value(tmp.path(), field, "./doc/../doc/design.md");

        let out = run_fr_ok(tmp.path(), &[field, "M-001", "add", "doc/design.md"]);
        assert!(out.contains("unchanged"), "{}: {}", field, out);

        let track = fs::read_to_string(tmp.path().join("frame/tracks/main.md")).unwrap();
        assert_eq!(
            track.matches("design.md").count(),
            1,
            "{} stored one file twice:\n{}",
            field,
            track
        );
    }
}

/// The suffix is part of the identity: a ref to a file and a ref to a line in it
/// are different references, and removing one must not take the other.
#[test]
fn test_path_field_rm_keeps_a_line_reference_apart() {
    for field in PATH_FIELDS {
        let tmp = tempfile::TempDir::new().unwrap();
        create_test_project(tmp.path());
        create_ref_targets(tmp.path());

        run_fr_ok(
            tmp.path(),
            &[field, "M-001", "add", "src/parser.rs", "src/parser.rs:807"],
        );
        run_fr_ok(tmp.path(), &[field, "M-001", "rm", "src/parser.rs"]);

        let track = fs::read_to_string(tmp.path().join("frame/tracks/main.md")).unwrap();
        assert!(
            track.contains(&format!("{}: src/parser.rs:807", field)),
            "{} removed the line reference too:\n{}",
            field,
            track
        );
    }
}

/// `set` is the one door through which a list could be handed two spellings of
/// one file at once.
#[test]
fn test_path_field_set_dedupes_equivalent_spellings() {
    for field in PATH_FIELDS {
        let tmp = tempfile::TempDir::new().unwrap();
        create_test_project(tmp.path());
        create_ref_targets(tmp.path());

        let out = run_fr_ok(
            tmp.path(),
            &[
                field,
                "M-001",
                "set",
                "doc/design.md",
                "./doc/../doc/design.md",
                "src/parser.rs",
            ],
        );
        assert!(
            out.contains("set: doc/design.md, src/parser.rs"),
            "{} reported what it was handed rather than what it stored: {}",
            field,
            out
        );

        let track = fs::read_to_string(tmp.path().join("frame/tracks/main.md")).unwrap();
        assert!(
            track.contains(&format!("{}: doc/design.md, src/parser.rs", field)),
            "{}:\n{}",
            field,
            track
        );
    }
}

#[test]
fn test_path_field_unknown_action_is_refused() {
    for field in PATH_FIELDS {
        let tmp = tempfile::TempDir::new().unwrap();
        create_test_project(tmp.path());
        create_ref_targets(tmp.path());

        let (_, stderr, ok) = run_fr(tmp.path(), &[field, "M-001", "append", "doc/design.md"]);
        assert!(!ok, "{} accepted a bogus action", field);
        assert!(stderr.contains("add, rm, set"), "{}", stderr);
    }
}

/// An anchor and a line reference are part of the value, not part of the path —
/// they say *where* in the file, and frame does not read the file to check.
#[test]
fn test_path_field_accepts_an_anchor_or_a_line_reference() {
    for field in PATH_FIELDS {
        let tmp = tempfile::TempDir::new().unwrap();
        create_test_project(tmp.path());
        create_ref_targets(tmp.path());

        run_fr_ok(
            tmp.path(),
            &[
                field,
                "M-001",
                "add",
                "doc/design.md#rationale",
                "src/parser.rs:807",
                "src/parser.rs:807-820",
            ],
        );
        let track = fs::read_to_string(tmp.path().join("frame/tracks/main.md")).unwrap();
        assert!(track.contains(&format!(
            "{}: doc/design.md#rationale, src/parser.rs:807, src/parser.rs:807-820",
            field
        )));

        let (stdout, _, ok) = run_fr(tmp.path(), &["check"]);
        assert!(ok, "check rejected a valid {}: {}", field, stdout);
    }
}

#[test]
fn test_path_field_refuses_a_missing_path() {
    for field in PATH_FIELDS {
        let tmp = tempfile::TempDir::new().unwrap();
        create_test_project(tmp.path());
        create_ref_targets(tmp.path());

        let (_, stderr, ok) = run_fr(tmp.path(), &[field, "M-001", "add", "doc/typo.md"]);
        assert!(!ok, "a missing path should exit non-zero");
        assert!(stderr.contains("no such file: doc/typo.md"), "{}", stderr);
        assert!(stderr.contains("--force"), "{}", stderr);

        let track = fs::read_to_string(tmp.path().join("frame/tracks/main.md")).unwrap();
        assert!(
            !track.contains(&format!("{}:", field)),
            "nothing should have been written"
        );
    }
}

/// All or nothing: one bad path in a list writes none of them, and every bad one
/// is named so the fix is a single retry.
#[test]
fn test_path_field_names_every_missing_path_and_writes_none() {
    for field in PATH_FIELDS {
        let tmp = tempfile::TempDir::new().unwrap();
        create_test_project(tmp.path());
        create_ref_targets(tmp.path());

        let (_, stderr, ok) = run_fr(
            tmp.path(),
            &[field, "M-001", "add", "doc/design.md", "a.md", "b.md"],
        );
        assert!(!ok);
        assert!(
            stderr.contains("a.md") && stderr.contains("b.md"),
            "{}",
            stderr
        );

        let track = fs::read_to_string(tmp.path().join("frame/tracks/main.md")).unwrap();
        assert!(!track.contains(&format!("{}:", field)));
    }
}

#[test]
fn test_path_field_force_accepts_a_missing_path() {
    for field in PATH_FIELDS {
        let tmp = tempfile::TempDir::new().unwrap();
        create_test_project(tmp.path());
        create_ref_targets(tmp.path());

        run_fr_ok(
            tmp.path(),
            &[field, "M-001", "add", "doc/not-yet.md", "--force"],
        );
        let track = fs::read_to_string(tmp.path().join("frame/tracks/main.md")).unwrap();
        assert!(track.contains(&format!("{}: doc/not-yet.md", field)));
    }
}

/// A file outside the project, and a file named from the filesystem root. Both
/// resolve here and neither survives a clone, which is why they are refused
/// rather than reported broken.
#[test]
fn test_path_field_refuses_a_path_outside_the_project() {
    for field in PATH_FIELDS {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path().join("project");
        fs::create_dir_all(&root).unwrap();
        create_test_project(&root);
        create_ref_targets(&root);
        // A real file, one level up from the project root.
        fs::write(tmp.path().join("outside.md"), "outside\n").unwrap();
        let inside_absolute = root.join("doc/design.md").to_string_lossy().into_owned();

        for (path, expected) in [
            ("../outside.md", "leaves the project root"),
            ("/etc/hosts", "is absolute"),
            // Absolute and pointing *into* the project is still absolute.
            (inside_absolute.as_str(), "is absolute"),
        ] {
            let (_, stderr, ok) = run_fr(&root, &[field, "M-001", "add", path]);
            assert!(!ok, "{} accepted {}", field, path);
            assert!(
                stderr.contains(path) && stderr.contains(expected),
                "{} on {}: {}",
                field,
                path,
                stderr
            );
        }

        let track = fs::read_to_string(root.join("frame/tracks/main.md")).unwrap();
        assert!(
            !track.contains(&format!("{}:", field)),
            "nothing should have been written:\n{}",
            track
        );
    }
}

/// The check runs on the folded value, so an escape assembled out of `..`
/// segments is caught just as a leading one is.
#[test]
fn test_path_field_refuses_an_escape_that_only_appears_after_folding() {
    for field in PATH_FIELDS {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path().join("project");
        fs::create_dir_all(&root).unwrap();
        create_test_project(&root);
        create_ref_targets(&root);
        fs::write(tmp.path().join("outside.md"), "outside\n").unwrap();

        let (_, stderr, ok) = run_fr(&root, &[field, "M-001", "add", "doc/../../outside.md"]);
        assert!(!ok, "{} accepted a folded escape", field);
        assert!(stderr.contains("leaves the project root"), "{}", stderr);

        // A path that dips out of a subdirectory and comes back is fine.
        run_fr_ok(&root, &[field, "M-001", "add", "doc/../src/parser.rs"]);
        let track = fs::read_to_string(root.join("frame/tracks/main.md")).unwrap();
        assert!(
            track.contains(&format!("{}: src/parser.rs", field)),
            "{}",
            track
        );
    }
}

/// Every offender is named with its own reason, and nothing is written.
#[test]
fn test_path_field_names_every_uncontained_path() {
    for field in PATH_FIELDS {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path().join("project");
        fs::create_dir_all(&root).unwrap();
        create_test_project(&root);
        create_ref_targets(&root);
        fs::write(tmp.path().join("outside.md"), "outside\n").unwrap();

        let (_, stderr, ok) = run_fr(
            &root,
            &[
                field,
                "M-001",
                "add",
                "doc/design.md",
                "../outside.md",
                "/etc/hosts",
            ],
        );
        assert!(!ok);
        assert!(
            stderr.contains("../outside.md") && stderr.contains("/etc/hosts"),
            "{}",
            stderr
        );
        assert!(stderr.contains("--force"), "{}", stderr);

        let track = fs::read_to_string(root.join("frame/tracks/main.md")).unwrap();
        assert!(!track.contains(&format!("{}:", field)));
    }
}

/// `--force` is one flag with one meaning: write it anyway.
#[test]
fn test_path_field_force_accepts_an_uncontained_path() {
    for field in PATH_FIELDS {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path().join("project");
        fs::create_dir_all(&root).unwrap();
        create_test_project(&root);
        create_ref_targets(&root);
        fs::write(tmp.path().join("outside.md"), "outside\n").unwrap();

        run_fr_ok(&root, &[field, "M-001", "add", "../outside.md", "--force"]);
        let track = fs::read_to_string(root.join("frame/tracks/main.md")).unwrap();
        assert!(
            track.contains(&format!("{}: ../outside.md", field)),
            "{}",
            track
        );
    }
}

/// A project in a git repo that ignores `scratch/` and `*.tmp`. `None` when git
/// is unavailable, so callers skip rather than fail.
fn project_ignoring_scratch(root: &Path) -> Option<()> {
    create_test_project(root);
    create_ref_targets(root);
    fs::create_dir_all(root.join("scratch")).unwrap();
    fs::write(root.join("scratch/notes.md"), "notes\n").unwrap();
    fs::write(root.join("doc/draft.tmp"), "draft\n").unwrap();
    fs::write(root.join(".gitignore"), "scratch/\n*.tmp\nframe/.*\n").unwrap();
    Command::new("git")
        .current_dir(root)
        .args(["init", "-q"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
        .then_some(())
}

/// A gitignored file is in this working copy and will be in no one else's, so a
/// ref to it resolves here and nowhere else — the same failure as an escaping
/// path, arrived at a different way.
#[test]
fn test_path_field_refuses_a_gitignored_path() {
    for field in PATH_FIELDS {
        let tmp = tempfile::TempDir::new().unwrap();
        if project_ignoring_scratch(tmp.path()).is_none() {
            return; // git unavailable
        }

        for path in ["scratch/notes.md", "doc/draft.tmp"] {
            let (_, stderr, ok) = run_fr(tmp.path(), &[field, "M-001", "add", path]);
            assert!(!ok, "{} accepted {}", field, path);
            assert!(
                stderr.contains(path) && stderr.contains("ignored by git"),
                "{} on {}: {}",
                field,
                path,
                stderr
            );
        }

        // The file next to them, covered by no rule, is fine.
        run_fr_ok(tmp.path(), &[field, "M-001", "add", "doc/design.md"]);
        let track = fs::read_to_string(tmp.path().join("frame/tracks/main.md")).unwrap();
        assert!(
            track.contains(&format!("{}: doc/design.md", field)),
            "{}",
            track
        );
    }
}

/// The suffix is stripped before git is asked — `doc/draft.tmp:12` is not a
/// filename, and the `*.tmp` rule would not match it.
#[test]
fn test_path_field_gitignore_check_ignores_the_suffix() {
    for field in PATH_FIELDS {
        let tmp = tempfile::TempDir::new().unwrap();
        if project_ignoring_scratch(tmp.path()).is_none() {
            return;
        }

        let (_, stderr, ok) = run_fr(tmp.path(), &[field, "M-001", "add", "doc/draft.tmp:12"]);
        assert!(!ok, "{} accepted a suffixed gitignored path", field);
        assert!(stderr.contains("ignored by git"), "{}", stderr);
    }
}

/// Ignore rules do not apply to files already in the index, so a tracked path
/// travels whatever `.gitignore` says — and refusing it would be a lie.
#[test]
fn test_path_field_accepts_a_gitignored_path_that_is_tracked() {
    for field in PATH_FIELDS {
        let tmp = tempfile::TempDir::new().unwrap();
        if project_ignoring_scratch(tmp.path()).is_none() {
            return;
        }
        // Force it into the index, as someone would for a file that must be
        // committed despite a broad rule.
        assert!(git(tmp.path(), &["add", "-f", "scratch/notes.md"]));

        run_fr_ok(tmp.path(), &[field, "M-001", "add", "scratch/notes.md"]);
        let track = fs::read_to_string(tmp.path().join("frame/tracks/main.md")).unwrap();
        assert!(
            track.contains(&format!("{}: scratch/notes.md", field)),
            "{}",
            track
        );
    }
}

#[test]
fn test_path_field_force_accepts_a_gitignored_path() {
    for field in PATH_FIELDS {
        let tmp = tempfile::TempDir::new().unwrap();
        if project_ignoring_scratch(tmp.path()).is_none() {
            return;
        }

        run_fr_ok(
            tmp.path(),
            &[field, "M-001", "add", "scratch/notes.md", "--force"],
        );
        let track = fs::read_to_string(tmp.path().join("frame/tracks/main.md")).unwrap();
        assert!(
            track.contains(&format!("{}: scratch/notes.md", field)),
            "{}",
            track
        );
    }
}

/// Outside a repo frame cannot tell what git would ignore, so it allows.
#[test]
fn test_path_field_gitignore_check_is_a_no_op_outside_a_repo() {
    for field in PATH_FIELDS {
        let tmp = tempfile::TempDir::new().unwrap();
        create_test_project(tmp.path());
        create_ref_targets(tmp.path());
        fs::create_dir_all(tmp.path().join("scratch")).unwrap();
        fs::write(tmp.path().join("scratch/notes.md"), "notes\n").unwrap();
        fs::write(tmp.path().join(".gitignore"), "scratch/\n").unwrap();
        // No `git init`: the `.gitignore` is just a file with no repo to read it.

        run_fr_ok(tmp.path(), &[field, "M-001", "add", "scratch/notes.md"]);
        let track = fs::read_to_string(tmp.path().join("frame/tracks/main.md")).unwrap();
        assert!(
            track.contains(&format!("{}: scratch/notes.md", field)),
            "{}",
            track
        );
    }
}

/// `fr check` applies the same rule to values already in the file — written by
/// `--force`, by an older `fr`, by the TUI, or by hand. As **warnings**: the
/// paths resolve, so nothing here is invalid, and a policy added later must not
/// turn a passing project red.
#[test]
fn test_check_warns_about_refs_that_will_not_travel_without_failing() {
    let tmp = tempfile::TempDir::new().unwrap();
    if project_ignoring_scratch(tmp.path()).is_none() {
        return; // git unavailable
    }
    let absolute = tmp
        .path()
        .join("doc/design.md")
        .to_string_lossy()
        .into_owned();
    for path in [absolute.as_str(), "scratch/notes.md"] {
        run_fr_ok(tmp.path(), &["ref", "M-001", "add", path, "--force"]);
    }

    let (stdout, _, ok) = run_fr(tmp.path(), &["check"]);
    assert!(ok, "warnings must not fail the check: {}", stdout);
    assert!(stdout.contains("is absolute"), "{}", stdout);
    assert!(stdout.contains("ignored by git"), "{}", stdout);

    // And the same two findings in `--json`, since both surfaces must agree.
    let (json, _, ok) = run_fr(tmp.path(), &["check", "--json"]);
    assert!(ok);
    let value: serde_json::Value = serde_json::from_str(&json).unwrap();
    let tags: Vec<&str> = value["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|w| w["type"].as_str())
        .collect();
    assert!(
        tags.contains(&"ref_outside_project") && tags.contains(&"ref_gitignored"),
        "{:?}",
        tags
    );
}

/// A ref that resolves and stays inside says nothing, in a project where the
/// checks are live. Guards the finding against firing on ordinary refs.
#[test]
fn test_check_is_silent_about_an_ordinary_ref() {
    let tmp = tempfile::TempDir::new().unwrap();
    if project_ignoring_scratch(tmp.path()).is_none() {
        return;
    }
    run_fr_ok(tmp.path(), &["ref", "M-001", "add", "doc/design.md"]);

    let (stdout, _, ok) = run_fr(tmp.path(), &["check"]);
    assert!(ok, "{}", stdout);
    assert!(
        !stdout.contains("is absolute") && !stdout.contains("ignored by git"),
        "{}",
        stdout
    );
}

/// `rm` is never refused. An uncontained ref already in a file is exactly what
/// someone needs to be able to take out.
#[test]
fn test_path_field_rm_removes_an_uncontained_path() {
    for field in PATH_FIELDS {
        let tmp = tempfile::TempDir::new().unwrap();
        create_test_project(tmp.path());
        create_ref_targets(tmp.path());
        store_raw_path_value(tmp.path(), field, "../outside.md, /etc/hosts");

        run_fr_ok(tmp.path(), &[field, "M-001", "rm", "../outside.md"]);
        let track = fs::read_to_string(tmp.path().join("frame/tracks/main.md")).unwrap();
        assert!(!track.contains("outside.md"), "{}", track);
        assert!(track.contains("/etc/hosts"), "{}", track);

        run_fr_ok(tmp.path(), &[field, "M-001", "rm", "/etc/hosts"]);
        let track = fs::read_to_string(tmp.path().join("frame/tracks/main.md")).unwrap();
        assert!(!track.contains(&format!("{}:", field)), "{}", track);
    }
}

#[test]
fn test_title() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());

    run_fr_ok(tmp.path(), &["title", "M-001", "Updated title from CLI"]);
    let track = fs::read_to_string(tmp.path().join("frame/tracks/main.md")).unwrap();
    assert!(track.contains("Updated title from CLI"));
    assert!(!track.contains("First task"));
}

#[test]
fn test_mv_top() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());

    run_fr_ok(tmp.path(), &["mv", "M-003", "--top"]);
    let track = fs::read_to_string(tmp.path().join("frame/tracks/main.md")).unwrap();
    let pos_003 = track.find("M-003").unwrap();
    let pos_001 = track.find("M-001").unwrap();
    assert!(pos_003 < pos_001);
}

#[test]
fn test_mv_after() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());

    run_fr_ok(tmp.path(), &["mv", "M-001", "--after", "M-002"]);
    let track = fs::read_to_string(tmp.path().join("frame/tracks/main.md")).unwrap();
    let pos_002 = track.find("M-002").unwrap();
    let pos_001 = track.find("M-001").unwrap();
    assert!(pos_001 > pos_002);
}

#[test]
fn test_mv_done_task_cross_track() {
    // Regression: a completed task lives in the Done section, and `fr mv` used to
    // only scan the Backlog — so moving it cross-track failed with
    // "task not found" even though `fr show` found it. It must move and stay done.
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());

    // M-000 is a done task in the fixture's main track.
    let out = run_fr_ok(tmp.path(), &["mv", "M-000", "--track", "side"]);
    assert!(out.contains("(side)"), "unexpected output: {out}");

    // Gone from the source track entirely.
    let main = fs::read_to_string(tmp.path().join("frame/tracks/main.md")).unwrap();
    assert!(!main.contains("Setup project"), "still in source: {main}");

    // Landed in the *target's* Done section, still checked off and with its
    // resolved date preserved (not reopened into the Backlog).
    let side = fs::read_to_string(tmp.path().join("frame/tracks/side.md")).unwrap();
    let done_pos = side
        .find("## Done")
        .expect("side should have a Done section");
    let task_pos = side.find("Setup project").expect("task should be in side");
    assert!(task_pos > done_pos, "task should be under Done: {side}");
    assert!(side.contains("resolved:"), "resolved date lost: {side}");
    // The moved task line is still a checked-off `[x]` box, not reopened.
    let task_line = side
        .lines()
        .find(|l| l.contains("Setup project"))
        .expect("task line");
    assert!(
        task_line.trim_start().starts_with("- [x]"),
        "task should still be done: {task_line}"
    );
}

#[test]
fn test_mv_parked_task_cross_track() {
    // A parked task (M-010) moves cross-track and stays parked.
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());

    run_fr_ok(tmp.path(), &["mv", "M-010", "--track", "side"]);
    let side = fs::read_to_string(tmp.path().join("frame/tracks/side.md")).unwrap();
    let parked_pos = side
        .find("## Parked")
        .expect("side should gain a Parked section");
    let task_pos = side
        .find("Parked idea")
        .expect("parked task should be in side");
    assert!(task_pos > parked_pos, "task should be under Parked: {side}");
}

#[test]
fn test_commands_understand_actor_token_ids() {
    // Every id-taking command must accept the actor-token id form (e.g. M-b1),
    // not just the legacy bare-number form. Claim token `b`, mint tokened ids,
    // and exercise the id-resolving commands — including the cross-track `mv`
    // that first surfaced the issue.
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());

    run_fr_ok(tmp.path(), &["actor", "set", "b"]);
    // First mint in the `b` namespace → M-b1.
    let add = run_fr_ok(tmp.path(), &["add", "main", "Tokened task"]);
    assert!(add.contains("M-b1"), "expected M-b1, got: {add}");
    run_fr_ok(tmp.path(), &["add", "main", "Second tokened"]); // M-b2

    // Read + metadata commands resolve the tokened id.
    assert!(run_fr_ok(tmp.path(), &["show", "M-b1"]).contains("Tokened task"));
    run_fr_ok(tmp.path(), &["tag", "M-b1", "add", "urgent"]);
    run_fr_ok(tmp.path(), &["dep", "M-b1", "add", "M-b2"]);
    run_fr_ok(tmp.path(), &["note", "M-b1", "a note"]);
    run_fr_ok(tmp.path(), &["title", "M-b1", "Renamed"]);
    run_fr_ok(tmp.path(), &["state", "M-b1", "active"]);
    run_fr_ok(tmp.path(), &["deps", "M-b1"]);

    // Reorder (same-track) and cross-track move both take the tokened id.
    run_fr_ok(tmp.path(), &["mv", "M-b2", "--top"]);
    let out = run_fr_ok(tmp.path(), &["mv", "M-b1", "--track", "side"]);
    assert!(out.contains("(side)"), "cross-track mv failed: {out}");
    // Re-minted into the mover's `b` namespace on the target.
    let side = fs::read_to_string(tmp.path().join("frame/tracks/side.md")).unwrap();
    assert!(side.contains("S-b1") && side.contains("Renamed"), "{side}");
}

#[test]
fn test_inbox_add() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());

    run_fr_ok(tmp.path(), &["inbox", "New inbox item", "--tag", "bug"]);
    let inbox = fs::read_to_string(tmp.path().join("frame/inbox.md")).unwrap();
    assert!(inbox.contains("New inbox item"));
    assert!(inbox.contains("#bug"));
}

#[test]
fn test_triage() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());

    // Triage item 1 (Bug in parser) to main track
    let out = run_fr_ok(tmp.path(), &["triage", "1", "--track", "main"]);
    assert!(out.contains("M-011")); // New task ID

    // Verify it was added to the track
    let track = fs::read_to_string(tmp.path().join("frame/tracks/main.md")).unwrap();
    assert!(track.contains("Bug in parser"));
    assert!(track.contains("M-011"));

    // Verify it was removed from inbox
    let inbox = fs::read_to_string(tmp.path().join("frame/inbox.md")).unwrap();
    assert!(!inbox.contains("Bug in parser"));
}

#[test]
fn test_triage_top() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());

    run_fr_ok(tmp.path(), &["triage", "2", "--track", "main", "--top"]);

    let track = fs::read_to_string(tmp.path().join("frame/tracks/main.md")).unwrap();
    // "Think about design" should be at the top of backlog
    let pos_design = track.find("Think about design").unwrap();
    let pos_001 = track.find("M-001").unwrap();
    assert!(pos_design < pos_001);
}

// ---------------------------------------------------------------------------
// Track management tests
// ---------------------------------------------------------------------------

#[test]
fn test_track_new() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());

    run_fr_ok(tmp.path(), &["track", "new", "feat", "Features"]);

    // Verify the track file was created
    assert!(tmp.path().join("frame/tracks/feat.md").exists());

    // Verify config was updated
    let config = fs::read_to_string(tmp.path().join("frame/project.toml")).unwrap();
    assert!(config.contains("feat"));
    assert!(config.contains("Features"));
}

#[test]
fn test_track_shelve() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());

    run_fr_ok(tmp.path(), &["track", "shelve", "side"]);

    let config = fs::read_to_string(tmp.path().join("frame/project.toml")).unwrap();
    // side track should now be shelved
    assert!(config.contains("\"shelved\""));
}

#[test]
fn test_track_activate() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());

    // First shelve, then re-activate
    run_fr_ok(tmp.path(), &["track", "shelve", "side"]);
    run_fr_ok(tmp.path(), &["track", "activate", "side"]);

    let config = fs::read_to_string(tmp.path().join("frame/project.toml")).unwrap();
    // Count active states — both main and side should be active again
    let active_count = config.matches("\"active\"").count();
    assert_eq!(active_count, 2);
}

#[test]
fn test_add_to_shelved_track_blocked() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());

    run_fr_ok(tmp.path(), &["track", "shelve", "side"]);

    let (_out, err, ok) = run_fr(tmp.path(), &["add", "side", "New task"]);
    assert!(!ok, "adding to a shelved track should fail");
    assert!(
        err.contains("shelved"),
        "error should mention shelved: {err}"
    );
    assert!(
        err.contains("fr track activate side"),
        "error should suggest activating the track: {err}"
    );

    // The task must not have been written.
    let side = fs::read_to_string(tmp.path().join("frame/tracks/side.md")).unwrap();
    assert!(!side.contains("New task"));
}

#[test]
fn test_push_to_shelved_track_blocked() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());

    run_fr_ok(tmp.path(), &["track", "shelve", "side"]);

    let (_out, err, ok) = run_fr(tmp.path(), &["push", "side", "Urgent"]);
    assert!(!ok, "pushing to a shelved track should fail");
    assert!(
        err.contains("shelved"),
        "error should mention shelved: {err}"
    );
}

#[test]
fn test_sub_to_shelved_track_blocked() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());

    run_fr_ok(tmp.path(), &["track", "shelve", "side"]);

    let (_out, err, ok) = run_fr(tmp.path(), &["sub", "S-001", "A subtask"]);
    assert!(!ok, "adding a subtask in a shelved track should fail");
    assert!(
        err.contains("shelved"),
        "error should mention shelved: {err}"
    );
}

#[test]
fn test_triage_to_shelved_track_blocked() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());

    run_fr_ok(tmp.path(), &["track", "shelve", "side"]);

    let (_out, err, ok) = run_fr(tmp.path(), &["triage", "1", "--track", "side"]);
    assert!(!ok, "triaging into a shelved track should fail");
    assert!(
        err.contains("shelved"),
        "error should mention shelved: {err}"
    );
}

#[test]
fn test_mv_into_shelved_track_blocked() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());

    run_fr_ok(tmp.path(), &["track", "shelve", "side"]);

    let (_out, err, ok) = run_fr(tmp.path(), &["mv", "M-001", "--track", "side"]);
    assert!(!ok, "moving a task into a shelved track should fail");
    assert!(
        err.contains("shelved"),
        "error should mention shelved: {err}"
    );
}

#[test]
fn test_import_to_shelved_track_blocked() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());

    let import_file = tmp.path().join("import.md");
    fs::write(&import_file, "- [ ] Imported task\n").unwrap();

    run_fr_ok(tmp.path(), &["track", "shelve", "side"]);

    let (_out, err, ok) = run_fr(
        tmp.path(),
        &["import", import_file.to_str().unwrap(), "--track", "side"],
    );
    assert!(!ok, "importing into a shelved track should fail");
    assert!(
        err.contains("shelved"),
        "error should mention shelved: {err}"
    );
}

#[test]
fn test_state_active_in_shelved_track_blocked() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());

    run_fr_ok(tmp.path(), &["track", "shelve", "side"]);

    let (_out, err, ok) = run_fr(tmp.path(), &["state", "S-001", "active"]);
    assert!(!ok, "activating a task in a shelved track should fail");
    assert!(
        err.contains("shelved"),
        "error should mention shelved: {err}"
    );

    // `fr start` is a thin alias for `state active` and must be blocked too.
    let (_out, err, ok) = run_fr(tmp.path(), &["start", "S-001"]);
    assert!(!ok, "`fr start` in a shelved track should fail");
    assert!(
        err.contains("shelved"),
        "error should mention shelved: {err}"
    );
}

#[test]
fn test_state_non_active_in_shelved_track_allowed() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());

    run_fr_ok(tmp.path(), &["track", "shelve", "side"]);

    // Only *activating* is blocked — you can still close out or re-open work in a
    // shelved track (e.g. mark done, park, or reset to todo).
    run_fr_ok(tmp.path(), &["state", "S-001", "done"]);
    let side = fs::read_to_string(tmp.path().join("frame/tracks/side.md")).unwrap();
    assert!(side.contains("[x] `S-001`"));
}

#[test]
fn test_track_cc_focus() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());

    run_fr_ok(tmp.path(), &["track", "cc-focus", "side"]);

    let config = fs::read_to_string(tmp.path().join("frame/project.toml")).unwrap();
    assert!(config.contains("cc_focus = \"side\""));
}

// ---------------------------------------------------------------------------
// Maintenance tests
// ---------------------------------------------------------------------------

#[test]
fn test_clean() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());

    let out = run_fr_ok(tmp.path(), &["clean"]);
    // Project should be clean (all IDs and dates assigned)
    assert!(out.contains("clean"));
}

#[test]
fn test_clean_dry_run() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());

    let out = run_fr_ok(tmp.path(), &["clean", "--dry-run"]);
    assert!(out.contains("dry run"));
}

#[test]
fn test_import() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());

    // Create an import file
    let import_file = tmp.path().join("import.md");
    fs::write(
        &import_file,
        "\
- [ ] Imported task one #core
- [ ] Imported task two #design
  - [ ] Imported sub
",
    )
    .unwrap();

    let out = run_fr_ok(
        tmp.path(),
        &["import", import_file.to_str().unwrap(), "--track", "main"],
    );
    assert!(out.contains("imported"));
    assert!(out.contains("M-011")); // First imported task ID

    let track = fs::read_to_string(tmp.path().join("frame/tracks/main.md")).unwrap();
    assert!(track.contains("Imported task one"));
    assert!(track.contains("Imported task two"));
    assert!(track.contains("Imported sub"));
}

#[test]
fn test_import_top() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());

    let import_file = tmp.path().join("import.md");
    fs::write(&import_file, "- [ ] Top import\n").unwrap();

    run_fr_ok(
        tmp.path(),
        &[
            "import",
            import_file.to_str().unwrap(),
            "--track",
            "main",
            "--top",
        ],
    );

    let track = fs::read_to_string(tmp.path().join("frame/tracks/main.md")).unwrap();
    let pos_import = track.find("Top import").unwrap();
    let pos_001 = track.find("M-001").unwrap();
    assert!(pos_import < pos_001);
}

// ---------------------------------------------------------------------------
// Error handling tests
// ---------------------------------------------------------------------------

#[test]
fn test_not_a_project() {
    let tmp = tempfile::TempDir::new().unwrap();
    // Don't create project structure
    let (_stdout, stderr, success) = run_fr(tmp.path(), &["list"]);
    assert!(!success);
    assert!(stderr.contains("not a Frame project") || stderr.contains("error"));
}

#[test]
fn test_add_to_nonexistent_track() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());

    let (_stdout, stderr, success) = run_fr(tmp.path(), &["add", "nonexist", "Task"]);
    assert!(!success);
    assert!(stderr.contains("error"));
}

#[test]
fn test_state_invalid() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());

    let (_stdout, stderr, success) = run_fr(tmp.path(), &["state", "M-001", "invalid_state"]);
    assert!(!success);
    assert!(stderr.contains("unknown state"));
}

#[test]
fn test_help() {
    let out = run_fr_ok(Path::new("."), &["--help"]);
    assert!(out.contains("frame"));
    assert!(out.contains("list"));
    assert!(out.contains("add"));
}

// ---------------------------------------------------------------------------
// Combined workflow tests
// ---------------------------------------------------------------------------

#[test]
fn test_add_then_show() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());

    let add_out = run_fr_ok(tmp.path(), &["add", "main", "Workflow test task"]);
    let id = add_out.trim();

    let show_out = run_fr_ok(tmp.path(), &["show", id]);
    assert!(show_out.contains("Workflow test task"));
}

#[test]
fn test_add_then_state_then_show() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());

    let add_out = run_fr_ok(tmp.path(), &["add", "side", "Side workflow"]);
    let id = add_out.trim();

    run_fr_ok(tmp.path(), &["state", id, "active"]);
    let show_out = run_fr_ok(tmp.path(), &["show", id, "--json"]);
    let parsed: serde_json::Value = serde_json::from_str(&show_out).unwrap();
    assert_eq!(parsed["state"], "active");
}

#[test]
fn test_found_from() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());

    let out = run_fr_ok(
        tmp.path(),
        &["add", "main", "Found bug", "--found-from", "M-001"],
    );
    let id = out.trim();

    let show_out = run_fr_ok(tmp.path(), &["show", id]);
    assert!(show_out.contains("Found while working on M-001"));
}

// ---------------------------------------------------------------------------
// Track rename / delete tests
// ---------------------------------------------------------------------------

#[test]
fn test_track_rename_name() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());

    run_fr_ok(
        tmp.path(),
        &["track", "rename", "side", "--name", "New Side"],
    );

    let config = fs::read_to_string(tmp.path().join("frame/project.toml")).unwrap();
    assert!(config.contains("\"New Side\""));

    // Track file header should be updated
    let track_content = fs::read_to_string(tmp.path().join("frame/tracks/side.md")).unwrap();
    assert!(track_content.starts_with("# New Side"));
}

#[test]
fn test_track_rename_id() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());

    run_fr_ok(tmp.path(), &["track", "rename", "side", "--new-id", "aux"]);

    // Old file should be gone, new file exists
    assert!(!tmp.path().join("frame/tracks/side.md").exists());
    assert!(tmp.path().join("frame/tracks/aux.md").exists());

    // Config should reference the new id
    let config = fs::read_to_string(tmp.path().join("frame/project.toml")).unwrap();
    assert!(config.contains("\"aux\""));
    assert!(config.contains("tracks/aux.md"));
}

#[test]
fn test_track_rename_prefix_yes() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());

    let out = run_fr_ok(
        tmp.path(),
        &["track", "rename", "side", "--prefix", "AUX", "--yes"],
    );
    assert!(out.contains("Renaming prefix S → AUX"));

    // Tasks should have new prefix
    let track_content = fs::read_to_string(tmp.path().join("frame/tracks/side.md")).unwrap();
    assert!(track_content.contains("AUX-001"));
    assert!(track_content.contains("AUX-002"));
    assert!(!track_content.contains("`S-001`"));

    // Config should have new prefix
    let config = fs::read_to_string(tmp.path().join("frame/project.toml")).unwrap();
    assert!(config.contains("\"AUX\""));
}

/// A prefix rename has to reach the track's archive, or the archived tasks are
/// left holding a prefix no track owns — invisible, and a collision the moment
/// that prefix is handed to another track.
///
/// It did not. `rename_archive_prefix` walked `TrackNode::Section` in a file
/// that has none, so it renamed nothing and wrote nothing; the impact count read
/// the same way, so the "N archived task IDs" line never printed and the command
/// reported plain success. Both the preview and the rename go through
/// `parse_archive` now.
#[test]
fn test_track_rename_prefix_reaches_the_archive() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());
    fs::create_dir_all(tmp.path().join("frame/archive")).unwrap();
    fs::write(
        tmp.path().join("frame/archive/side.md"),
        "# Archive — side\n\n- [x] `S-050` archived work\n  - resolved: 2025-06-01\n",
    )
    .unwrap();

    let out = run_fr_ok(
        tmp.path(),
        &["track", "rename", "side", "--prefix", "AUX", "--yes"],
    );
    assert!(
        out.contains("1 archived task ID"),
        "the archived id should be counted and reported: {out}"
    );

    let archive = fs::read_to_string(tmp.path().join("frame/archive/side.md")).unwrap();
    assert!(
        archive.contains("`AUX-050`") && !archive.contains("`S-050`"),
        "the archived id keeps the old prefix: {archive}"
    );
    assert!(
        archive.starts_with("# Archive — side"),
        "and the header is still the archive's own: {archive}"
    );
}

#[test]
fn test_track_rename_prefix_dry_run() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());

    let out = run_fr_ok(
        tmp.path(),
        &["track", "rename", "side", "--prefix", "AUX", "--dry-run"],
    );
    assert!(out.contains("dry run"));

    // Tasks should NOT have changed
    let track_content = fs::read_to_string(tmp.path().join("frame/tracks/side.md")).unwrap();
    assert!(track_content.contains("`S-001`"));
    assert!(track_content.contains("`S-002`"));
}

#[test]
fn test_track_delete_empty() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());

    // Create a new empty track, then delete it
    run_fr_ok(tmp.path(), &["track", "new", "empty", "Empty Track"]);
    assert!(tmp.path().join("frame/tracks/empty.md").exists());

    run_fr_ok(tmp.path(), &["track", "delete", "empty"]);

    // Track file should be gone
    assert!(!tmp.path().join("frame/tracks/empty.md").exists());

    // Config should no longer reference it
    let config = fs::read_to_string(tmp.path().join("frame/project.toml")).unwrap();
    assert!(!config.contains("\"empty\""));
}

#[test]
fn test_track_delete_non_empty_fails() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());

    let (_, stderr, success) = run_fr(tmp.path(), &["track", "delete", "main"]);
    assert!(!success);
    assert!(stderr.contains("tasks") || stderr.contains("not empty") || stderr.contains("has"));
}

// ---------------------------------------------------------------------------
// Init tests
// ---------------------------------------------------------------------------

#[test]
fn test_init_with_tracks() {
    let tmp = tempfile::TempDir::new().unwrap();

    let out = run_fr_ok(
        tmp.path(),
        &[
            "init",
            "--name",
            "Test Project",
            "--track",
            "api",
            "API Layer",
        ],
    );
    assert!(out.contains("[>] frame initialized"));
    assert!(out.contains("project.toml"));
    assert!(out.contains("inbox.md"));
    assert!(out.contains("tracks/api.md"));

    // project.toml exists and is valid TOML
    let toml_content = fs::read_to_string(tmp.path().join("frame/project.toml")).unwrap();
    let parsed: toml::Value = toml::from_str(&toml_content).unwrap();
    assert_eq!(parsed["project"]["name"].as_str().unwrap(), "Test Project");

    // Contains expected sections from the template
    assert!(toml_content.contains("[clean]"));
    assert!(toml_content.contains("[ui]"));
    assert!(toml_content.contains("[agent]"));
    assert!(toml_content.contains("[[tracks]]"));
    assert!(toml_content.contains("id = \"api\""));
    assert!(toml_content.contains("[ids.prefixes]"));

    // Track file exists
    assert!(tmp.path().join("frame/tracks/api.md").exists());
    // Inbox exists
    assert!(tmp.path().join("frame/inbox.md").exists());
}

#[test]
fn test_init_already_exists() {
    let tmp = tempfile::TempDir::new().unwrap();
    run_fr_ok(tmp.path(), &["init", "--name", "First"]);

    // Second init without --force should fail
    let (stdout, stderr, success) = run_fr(tmp.path(), &["init", "--name", "Second"]);
    assert!(!success);
    let combined = format!("{}{}", stdout, stderr);
    assert!(combined.contains("frame/ already exists"));
    assert!(combined.contains("--force"));
}

#[test]
fn test_init_force_reinitialize() {
    let tmp = tempfile::TempDir::new().unwrap();
    run_fr_ok(tmp.path(), &["init", "--name", "First"]);

    // --force should succeed
    let out = run_fr_ok(tmp.path(), &["init", "--name", "Second", "--force"]);
    assert!(out.contains("[>] frame initialized"));

    // Verify the config was overwritten
    let toml_content = fs::read_to_string(tmp.path().join("frame/project.toml")).unwrap();
    assert!(toml_content.contains("\"Second\""));
}

/// `fr init` inside a repo configures git the same way `fr git setup` does: the
/// blanket ignore pattern, the merge-driver attributes, and the driver itself.
/// A new project should not need a second command to be mergeable.
#[test]
fn test_init_configures_git() {
    let tmp = tempfile::TempDir::new().unwrap();
    if !git_ok(tmp.path(), &["init", "-q"]) {
        return; // git unavailable
    }

    let out = run_fr_ok(tmp.path(), &["init", "--name", "Git Project"]);
    assert!(
        out.contains("configured git"),
        "summary should say so: {out}"
    );

    let gitignore = fs::read_to_string(tmp.path().join(".gitignore")).unwrap();
    // One blanket pattern, not an entry per file.
    assert!(gitignore.contains("frame/.*"), "{gitignore}");
    assert!(
        !gitignore.contains("frame/.state.json"),
        "should not enumerate individual files: {gitignore}"
    );

    let attrs = fs::read_to_string(tmp.path().join(".gitattributes")).unwrap();
    assert!(
        attrs.contains("frame/tracks/*.md merge=frame"),
        "track files should route to the driver: {attrs}"
    );

    // And the project it just created is clean, driver warning included.
    let checked = run_fr_ok(tmp.path(), &["check"]);
    assert!(
        !checked.contains("merge driver"),
        "a project init just configured should not warn about the driver: {checked}"
    );
}

#[test]
fn test_init_gitignore_no_git() {
    let tmp = tempfile::TempDir::new().unwrap();

    // No .git dir — should not mention .gitignore
    let out = run_fr_ok(tmp.path(), &["init", "--name", "No Git"]);
    assert!(!out.contains(".gitignore"));
}

#[test]
fn test_init_gitignore_already_present() {
    let tmp = tempfile::TempDir::new().unwrap();
    fs::create_dir(tmp.path().join(".git")).unwrap();
    fs::write(
        tmp.path().join(".gitignore"),
        "frame/.state.json\nframe/.lock\nframe/.recovery.log\nframe/.actor\n",
    )
    .unwrap();

    let out = run_fr_ok(tmp.path(), &["init", "--name", "Already"]);
    // Should NOT say it added entries
    assert!(!out.contains("added frame/.state.json"));
}

/// A `.gitignore` that enumerates local-only files predates the blanket pattern.
/// The enumerated lines are *collapsed* into it rather than left beside it: they
/// are exactly what the pattern covers, and leaving both means the next local
/// file added to `frame/` looks covered when it is not.
#[test]
fn test_init_collapses_an_enumerated_gitignore() {
    let tmp = tempfile::TempDir::new().unwrap();
    if !git_ok(tmp.path(), &["init", "-q"]) {
        return; // git unavailable
    }
    fs::write(
        tmp.path().join(".gitignore"),
        "*.log\nframe/.lock\nframe/.actor\n",
    )
    .unwrap();

    run_fr_ok(tmp.path(), &["init", "--name", "Partial"]);

    let gitignore = fs::read_to_string(tmp.path().join(".gitignore")).unwrap();
    assert!(gitignore.contains("frame/.*"), "{gitignore}");
    assert!(
        !gitignore.lines().any(|l| l.trim() == "frame/.lock"),
        "the pattern covers it, so the line should be gone: {gitignore}"
    );
    assert!(
        !gitignore.lines().any(|l| l.trim() == "frame/.actor"),
        "same: {gitignore}"
    );
    // Lines that are none of frame's business are untouched.
    assert!(
        gitignore.lines().any(|l| l.trim() == "*.log"),
        "unrelated entries must survive: {gitignore}"
    );
}

// ---------------------------------------------------------------------------
// Reparent tests (fr mv --promote / --parent)
// ---------------------------------------------------------------------------

#[test]
fn test_mv_promote() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());

    // Promote M-003.1 to top-level
    let out = run_fr_ok(tmp.path(), &["mv", "M-003.1", "--promote"]);
    // Output should mention the old and new ID
    assert!(out.contains("M-003.1"));

    // The promoted task should now be a top-level task with a new ID
    let list_out = run_fr_ok(tmp.path(), &["list", "main", "--json"]);
    // M-003 should now have only one subtask
    assert!(list_out.contains("Sub two"));
    // The promoted task ("Sub one") should be top-level
    assert!(list_out.contains("Sub one"));
}

#[test]
fn test_mv_parent() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());

    // Reparent M-001 under M-002
    let out = run_fr_ok(tmp.path(), &["mv", "M-001", "--parent", "M-002"]);
    assert!(out.contains("M-001"));

    // M-001 should now be a subtask of M-002
    let show_out = run_fr_ok(tmp.path(), &["show", "M-002"]);
    assert!(show_out.contains("First task"));
}

/// `--parent` with neither placement flag appends as the last child. This is
/// the pre-existing default, pinned here because `--top`/`--after` now change it.
#[test]
fn test_mv_parent_default_appends_last() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());

    run_fr_ok(tmp.path(), &["mv", "M-001", "--parent", "M-003"]);

    let track = fs::read_to_string(tmp.path().join("frame/tracks/main.md")).unwrap();
    let first = track.find("First task").unwrap();
    let sub_one = track.find("Sub one").unwrap();
    let sub_two = track.find("Sub two").unwrap();
    assert!(sub_one < sub_two && sub_two < first, "expected last child");
}

#[test]
fn test_mv_parent_top() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());

    run_fr_ok(tmp.path(), &["mv", "M-001", "--parent", "M-003", "--top"]);

    let track = fs::read_to_string(tmp.path().join("frame/tracks/main.md")).unwrap();
    let first = track.find("First task").unwrap();
    let sub_one = track.find("Sub one").unwrap();
    let sub_two = track.find("Sub two").unwrap();
    assert!(first < sub_one && sub_one < sub_two, "expected first child");
}

#[test]
fn test_mv_parent_after() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());

    run_fr_ok(
        tmp.path(),
        &["mv", "M-001", "--parent", "M-003", "--after", "M-003.1"],
    );

    let track = fs::read_to_string(tmp.path().join("frame/tracks/main.md")).unwrap();
    let first = track.find("First task").unwrap();
    let sub_one = track.find("Sub one").unwrap();
    let sub_two = track.find("Sub two").unwrap();
    assert!(
        sub_one < first && first < sub_two,
        "expected placement between the two existing children"
    );
}

/// An unresolvable `--after` anchor aborts before anything is written.
#[test]
fn test_mv_parent_after_not_found() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());

    let (_, stderr, success) = run_fr(
        tmp.path(),
        &["mv", "M-001", "--parent", "M-003", "--after", "M-999"],
    );
    assert!(!success);
    assert!(stderr.contains("M-999"), "stderr should name the anchor");

    // Nothing moved: M-001 is still top-level, ID intact.
    let track = fs::read_to_string(tmp.path().join("frame/tracks/main.md")).unwrap();
    assert!(track.contains("- [ ] `M-001` First task"));
}

/// The anchor is resolved in the section the task lives in, not the Backlog —
/// promote works from any section and re-inserts into the one it came from.
#[test]
fn test_mv_promote_after_in_parked_section() {
    let tmp = tempfile::TempDir::new().unwrap();
    let frame_dir = tmp.path().join("frame");
    fs::create_dir_all(frame_dir.join("tracks")).unwrap();
    fs::write(frame_dir.join(".actor"), "null\n").unwrap();

    fs::write(
        frame_dir.join("project.toml"),
        r#"[project]
name = "parked-test"

[[tracks]]
id = "parked"
name = "Parked Track"
state = "active"
file = "tracks/parked.md"

[ids.prefixes]
parked = "P"
"#,
    )
    .unwrap();

    fs::write(
        frame_dir.join("tracks/parked.md"),
        "\
# Parked Track

## Backlog

- [ ] `P-005` A backlog task

## Parked

- [~] `P-001` Parked parent
  - [~] `P-001.1` Parked child
- [~] `P-002` Another parked
",
    )
    .unwrap();

    fs::write(frame_dir.join("inbox.md"), "# Inbox\n").unwrap();

    run_fr_ok(
        tmp.path(),
        &["mv", "P-001.1", "--promote", "--after", "P-002"],
    );

    let track = fs::read_to_string(tmp.path().join("frame/tracks/parked.md")).unwrap();
    let parent = track.find("Parked parent").unwrap();
    let another = track.find("Another parked").unwrap();
    let child = track.find("Parked child").unwrap();
    assert!(
        parent < another && another < child,
        "promoted task should follow its Parked-section anchor:\n{track}"
    );
}

/// `--track` alongside either reparent flag is rejected rather than ignored.
#[test]
fn test_mv_track_conflicts_with_reparent() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());

    for argv in [
        &["mv", "M-003.1", "--promote", "--track", "side"][..],
        &["mv", "M-001", "--parent", "M-003", "--track", "side"][..],
    ] {
        let (_, stderr, success) = run_fr(tmp.path(), argv);
        assert!(!success, "{argv:?} should be rejected");
        assert!(
            stderr.contains("cannot be used with") || stderr.contains("conflict"),
            "{argv:?} stderr: {stderr}"
        );
    }

    // Neither attempt wrote anything.
    let track = fs::read_to_string(tmp.path().join("frame/tracks/main.md")).unwrap();
    assert!(track.contains("- [ ] `M-001` First task"));
    assert!(track.contains("`M-003.1` Sub one"));
}

#[test]
fn test_mv_promote_top_level_error() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());

    // M-001 is already top-level — promote should fail
    let (_, stderr, success) = run_fr(tmp.path(), &["mv", "M-001", "--promote"]);
    assert!(!success);
    assert!(stderr.contains("already top-level") || stderr.contains("AlreadyTopLevel"));
}

#[test]
fn test_mv_parent_cycle_error() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());

    // Try to reparent M-003 under its own child M-003.1 — should fail
    let (_, stderr, success) = run_fr(tmp.path(), &["mv", "M-003", "--parent", "M-003.1"]);
    assert!(!success);
    assert!(stderr.contains("cycle") || stderr.contains("CycleDetected"));
}

#[test]
fn test_mv_promote_parent_conflict() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());

    // --promote and --parent together should fail
    let (_, stderr, success) = run_fr(
        tmp.path(),
        &["mv", "M-003.1", "--promote", "--parent", "M-001"],
    );
    assert!(!success);
    assert!(
        stderr.contains("cannot be used with")
            || stderr.contains("conflict")
            || stderr.contains("the argument")
    );
}

#[test]
fn test_mv_parent_depth_exceeded() {
    let tmp = tempfile::TempDir::new().unwrap();
    let frame_dir = tmp.path().join("frame");
    fs::create_dir_all(frame_dir.join("tracks")).unwrap();

    fs::write(
        frame_dir.join("project.toml"),
        r#"[project]
name = "depth-test"

[[tracks]]
id = "deep"
name = "Deep Track"
state = "active"
file = "tracks/deep.md"

[ids.prefixes]
deep = "D"
"#,
    )
    .unwrap();

    fs::write(
        frame_dir.join("tracks/deep.md"),
        "\
# Deep Track

## Backlog

- [ ] `D-001` Root
  - [ ] `D-001.1` Child
    - [ ] `D-001.1.1` Grandchild
- [ ] `D-002` Another root

## Done
",
    )
    .unwrap();

    fs::write(frame_dir.join("inbox.md"), "# Inbox\n").unwrap();

    // Try to reparent D-002 under D-001.1.1 (would exceed depth 2)
    let (_, stderr, success) = run_fr(tmp.path(), &["mv", "D-002", "--parent", "D-001.1.1"]);
    assert!(!success);
    assert!(
        stderr.contains("depth") || stderr.contains("DepthExceeded") || stderr.contains("nesting")
    );
}

// ---------------------------------------------------------------------------
// Show --context tests
// ---------------------------------------------------------------------------

#[test]
fn test_show_context_subtask() {
    let tmp = tempfile::tempdir().unwrap();
    create_test_project(tmp.path());

    let (stdout, _, success) = run_fr(tmp.path(), &["show", "M-003.1", "--context"]);
    assert!(success);

    // Should have parent separator
    assert!(stdout.contains("── Parent ── M-003"));
    // Should have task separator
    assert!(stdout.contains("── Task ── M-003.1"));
    // Parent fields should be present
    assert!(stdout.contains("state: todo"));
}

#[test]
fn test_show_context_top_level() {
    let tmp = tempfile::tempdir().unwrap();
    create_test_project(tmp.path());

    // Top-level task with --context: should show "── Task ──" separator but no parents
    let (stdout, _, success) = run_fr(tmp.path(), &["show", "M-003", "--context"]);
    assert!(success);

    assert!(!stdout.contains("── Parent ──"));
    assert!(stdout.contains("── Task ── M-003"));
}

#[test]
fn test_show_no_context_unchanged() {
    let tmp = tempfile::tempdir().unwrap();
    create_test_project(tmp.path());

    // Without --context, output should not have separators
    let (stdout, _, success) = run_fr(tmp.path(), &["show", "M-003.1"]);
    assert!(success);
    assert!(!stdout.contains("── Parent ──"));
    assert!(!stdout.contains("── Task ──"));
}

#[test]
fn test_show_json_always_has_ancestors() {
    let tmp = tempfile::tempdir().unwrap();
    create_test_project(tmp.path());

    // JSON output always includes ancestors, even without --context
    let (stdout, _, success) = run_fr(tmp.path(), &["show", "M-003.1", "--json"]);
    assert!(success);

    let json: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    let ancestors = json["ancestors"].as_array().unwrap();
    assert_eq!(ancestors.len(), 1);
    assert_eq!(ancestors[0]["id"], "M-003");
    assert_eq!(ancestors[0]["title"], "Third task with subtasks");
}

#[test]
fn test_show_json_top_level_empty_ancestors() {
    let tmp = tempfile::tempdir().unwrap();
    create_test_project(tmp.path());

    // Top-level task JSON should have empty ancestors (omitted by skip_serializing_if)
    let (stdout, _, success) = run_fr(tmp.path(), &["show", "M-003", "--json"]);
    assert!(success);

    let json: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    // ancestors should be absent (empty vec is skipped) or empty array
    assert!(json.get("ancestors").is_none() || json["ancestors"].as_array().unwrap().is_empty());
}

// ---------------------------------------------------------------------------
// Recovery command tests
// ---------------------------------------------------------------------------

#[test]
fn test_recovery_empty() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());

    // No recovery log exists — should succeed with empty output
    let out = run_fr_ok(tmp.path(), &["recovery"]);
    assert!(out.contains("No recovery log entries") || out.is_empty() || out.contains("recovery"));
}

#[test]
fn test_recovery_path() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());

    let out = run_fr_ok(tmp.path(), &["recovery", "path"]);
    assert!(out.contains(".recovery.log"));
    assert!(out.contains("frame"));
}

#[test]
fn test_recovery_prune_all_empty() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());

    // Prune on empty project should succeed
    let out = run_fr_ok(tmp.path(), &["recovery", "prune", "--all"]);
    assert!(out.contains("0") || out.contains("pruned") || out.contains("No"));
}

#[test]
fn test_recovery_with_entries() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());

    // Write a recovery log entry manually
    let recovery_path = tmp.path().join("frame/.recovery.log");
    let ts = "2026-02-10T12:00:00Z";
    let content = format!(
        "<!-- frame recovery log — append-only error recovery data\n     This file captures data that Frame couldn't save normally.\n     If something went missing, check here.\n     View with: fr recovery\n     Prune old entries: fr recovery prune\n     Safe to delete if empty or stale. -->\n\n---\n## {} — write: test failure\n\nSource: tracks/main.md\n\n```text\nlost content here\n```\n\n---\n",
        ts
    );
    fs::write(&recovery_path, content).unwrap();

    let out = run_fr_ok(tmp.path(), &["recovery"]);
    assert!(out.contains("write: test failure") || out.contains("test failure"));
}

#[test]
fn test_recovery_json() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());

    // Write a recovery log entry
    let recovery_path = tmp.path().join("frame/.recovery.log");
    let ts = "2026-02-10T12:00:00Z";
    let content = format!(
        "<!-- frame recovery log — append-only error recovery data\n     This file captures data that Frame couldn't save normally.\n     If something went missing, check here.\n     View with: fr recovery\n     Prune old entries: fr recovery prune\n     Safe to delete if empty or stale. -->\n\n---\n## {} — parser: dropped lines\n\nSource: inbox.md\n\n```text\nstray line\n```\n\n---\n",
        ts
    );
    fs::write(&recovery_path, content).unwrap();

    let out = run_fr_ok(tmp.path(), &["recovery", "--json"]);
    let parsed: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert!(parsed.is_array());
    let arr = parsed.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["category"], "parser");
    assert_eq!(arr[0]["description"], "dropped lines");
}

#[test]
fn test_recovery_prune_all_with_entries() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());

    // Write a recovery log entry
    let recovery_path = tmp.path().join("frame/.recovery.log");
    let ts = "2026-02-10T12:00:00Z";
    let content = format!(
        "<!-- frame recovery log — append-only error recovery data\n     This file captures data that Frame couldn't save normally.\n     If something went missing, check here.\n     View with: fr recovery\n     Prune old entries: fr recovery prune\n     Safe to delete if empty or stale. -->\n\n---\n## {} — write: failure\n\n---\n",
        ts
    );
    fs::write(&recovery_path, content).unwrap();

    let out = run_fr_ok(tmp.path(), &["recovery", "prune", "--all"]);
    assert!(out.contains("1") || out.contains("pruned"));

    // After prune, recovery should show no entries
    let out2 = run_fr_ok(tmp.path(), &["recovery"]);
    assert!(out2.contains("No recovery log entries") || !out2.contains("write: failure"));
}

#[test]
fn test_recovery_limit() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());

    // Write two recovery log entries
    let recovery_path = tmp.path().join("frame/.recovery.log");
    let content = "\
<!-- frame recovery log — append-only error recovery data
     This file captures data that Frame couldn't save normally.
     If something went missing, check here.
     View with: fr recovery
     Prune old entries: fr recovery prune
     Safe to delete if empty or stale. -->

---
## 2026-02-10T11:00:00Z — parser: first entry

---
## 2026-02-10T12:00:00Z — write: second entry

---
";
    fs::write(&recovery_path, content).unwrap();

    let out = run_fr_ok(tmp.path(), &["recovery", "--limit", "1"]);
    // Should only show the most recent entry
    assert!(out.contains("second entry"));
    assert!(!out.contains("first entry"));
}

// ---------------------------------------------------------------------------
// `fr recovery` surfacing: what it hides, and how to find one entry
//
// `fr check` sends people straight here to find one specific entry. With the
// default limit of 10 against a longer log, a silent truncation is
// indistinguishable from an empty tail — which is how a real diagnosis
// concluded frame had never written an entry that was simply on page two.
// ---------------------------------------------------------------------------

/// Write a log holding `count` entries, each naming task `M-<i>`.
fn write_numbered_log(root: &Path, count: usize) {
    let mut content = String::from(
        "\
<!-- frame recovery log — append-only error recovery data
     This file captures data that Frame couldn't save normally.
     If something went missing, check here.
     View with: fr recovery
     Prune old entries: fr recovery prune
     Safe to delete if empty or stale. -->

---
",
    );
    for i in 0..count {
        content.push_str(&format!(
            "## 2026-02-10T{:02}:00:00Z — delete: task M-{} deleted\n\nTrack: main\n\n---\n",
            i % 24,
            i
        ));
    }
    fs::write(root.join("frame/.recovery.log"), content).unwrap();
}

// ---------------------------------------------------------------------------
// `fr merge` says where the discarded side went — by absolute path
//
// The marker it leaves in the file is committed and travels; the log is not.
// So the reader may well follow this from a different working copy, and
// "see `fr recovery`" is not an answer they can act on.
// ---------------------------------------------------------------------------

/// A track file holding one conflicting task, in a directory of its own.
fn write_conflict_sides(dir: &Path) -> (PathBuf, PathBuf, PathBuf) {
    fs::create_dir_all(dir).unwrap();
    let shell = |task: &str| format!("# Main Track\n\n## Backlog\n\n{task}\n## Done\n");
    let base = dir.join("base.md");
    let ours = dir.join("ours.md");
    let theirs = dir.join("theirs.md");
    fs::write(&base, shell("- [ ] `M-001` Original\n")).unwrap();
    fs::write(&ours, shell("- [ ] `M-001` Our edit\n")).unwrap();
    fs::write(&theirs, shell("- [ ] `M-001` Their edit\n")).unwrap();
    (base, ours, theirs)
}

fn run_merge(cwd: &Path, base: &Path, ours: &Path, theirs: &Path) -> (String, String, bool) {
    run_fr(
        cwd,
        &[
            "merge",
            "--kind",
            "track",
            "--base",
            base.to_str().unwrap(),
            "--ours",
            ours.to_str().unwrap(),
            "--theirs",
            theirs.to_str().unwrap(),
        ],
    )
}

#[test]
fn merge_names_the_recovery_log_it_wrote_to_by_absolute_path() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());
    let (base, ours, theirs) = write_conflict_sides(&tmp.path().join("work"));

    let (_, stderr, ok) = run_merge(tmp.path(), &base, &ours, &theirs);
    assert!(!ok, "a conflict exits non-zero");
    assert!(
        stderr.contains("is in the recovery log"),
        "it did log, so it should say so:\n{stderr}"
    );
    assert!(
        stderr.contains("fr recovery --for M-001"),
        "and name the lookup that retrieves it:\n{stderr}"
    );
    // The path is printed, absolute, on its own line.
    let named = stderr
        .lines()
        .map(str::trim)
        .find(|l| l.ends_with(".recovery.log") || l.ends_with("frame-recovery.log"))
        .unwrap_or_else(|| panic!("no log path in:\n{stderr}"));
    assert!(
        Path::new(named).is_absolute(),
        "the reader may be anywhere: {named}"
    );
    assert!(Path::new(named).exists(), "and it is really there: {named}");
}

/// The accident that found this: merging files in a scratch directory wrote the
/// discarded side into whatever project happened to sit above the working
/// directory. The merged file's project is the only one entitled to it.
#[test]
fn merge_declines_to_log_into_a_project_that_does_not_hold_the_merged_file() {
    let bystander = tempfile::TempDir::new().unwrap();
    create_test_project(bystander.path());

    let elsewhere = tempfile::TempDir::new().unwrap();
    let (base, ours, theirs) = write_conflict_sides(elsewhere.path());

    // cwd is inside the bystander project; the files being merged are not.
    let (_, stderr, ok) = run_merge(bystander.path(), &base, &ours, &theirs);
    assert!(!ok);
    assert!(
        stderr.contains("NOT recorded"),
        "it must say the other side went nowhere:\n{stderr}"
    );
    assert!(
        stderr.contains("recover theirs from version control"),
        "and where to look instead:\n{stderr}"
    );
    assert!(
        !bystander.path().join("frame/.recovery.log").exists(),
        "an unrelated project's log must not be written to"
    );
    let listed = run_fr_ok(bystander.path(), &["recovery"]);
    assert!(
        !listed.contains("M-001"),
        "nor its listing polluted:\n{listed}"
    );
}

/// Run from inside the project the files belong to — the ordinary case, and the
/// one a VCS produces, since it invokes the driver from the worktree root.
#[test]
fn merge_logs_into_the_project_holding_the_merged_file() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());
    let (base, ours, theirs) = write_conflict_sides(&tmp.path().join("work"));

    // Even run from somewhere else entirely, the file's own project wins.
    let outside = tempfile::TempDir::new().unwrap();
    let (_, stderr, _) = run_merge(outside.path(), &base, &ours, &theirs);
    assert!(stderr.contains("is in the recovery log"), "{stderr}");

    let listed = run_fr_ok(tmp.path(), &["recovery", "--for", "M-001"]);
    assert!(
        listed.contains("Their edit"),
        "their version should be retrievable from the project that owns the file:\n{listed}"
    );
}

/// The rescue directory is named once, in an exit message, on a terminal that
/// is about to be closed. After that nothing mentioned it again — so the copies
/// sat there being the only version of that work with nobody looking.
#[test]
fn check_reports_rescue_copies_nobody_has_dealt_with() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());
    let rescue = tmp.path().join("frame/.rescue");
    fs::create_dir_all(&rescue).unwrap();
    fs::write(rescue.join("main.md"), "# Main Track\n").unwrap();

    let (out, _, ok) = run_fr(tmp.path(), &["check"]);
    assert!(
        ok,
        "a waiting rescue copy is a warning, not a broken project:\n{out}"
    );
    assert!(out.contains("rescue"), "{out}");
    assert!(
        out.contains("main.md"),
        "it should say what is waiting:\n{out}"
    );
    let named = out
        .lines()
        .map(str::trim)
        .find(|l| l.ends_with("frame/.rescue"))
        .unwrap_or_else(|| panic!("check should name the directory:\n{out}"));
    assert!(Path::new(named).is_absolute(), "{named}");

    // Clearing the directory clears the warning — there is no `--fix`, because
    // which copy wins is the user's call.
    fs::remove_dir_all(&rescue).unwrap();
    let (after, _, _) = run_fr(tmp.path(), &["check"]);
    assert!(!after.contains("rescue"), "{after}");
}

#[test]
fn check_names_the_recovery_log_it_summarises() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());
    // M-002 carries `dep: M-001`, so deleting it also leaves a dangling dep and
    // check exits non-zero. Irrelevant here — the summary prints either way.
    run_fr_ok(tmp.path(), &["delete", "M-001", "--yes"]);

    let (out, _, _) = run_fr(tmp.path(), &["check"]);
    assert!(out.contains("Recovery log:"), "{out}");
    let named = out
        .lines()
        .map(str::trim)
        .find(|l| l.ends_with(".recovery.log") || l.ends_with("frame-recovery.log"))
        .unwrap_or_else(|| panic!("check should name the log it counted:\n{out}"));
    assert!(Path::new(named).is_absolute(), "{named}");
}

// ---------------------------------------------------------------------------
// The recovery log is shared by every worktree of a clone
//
// The bug this closes: `fr check` was run in the main working tree, the
// investigation ran in a linked worktree, and the conflict entry the message
// pointed at was in neither place the investigator looked — so it was reported
// as never written. The log also died with the worktree: `git worktree remove`
// deletes ignored files silently, exit 0, no prompt.
//
// Both halves are asserted below.
// ---------------------------------------------------------------------------

/// A git repo holding a frame project, with a linked worktree at `../wt`.
/// Returns false when git is unavailable.
fn repo_with_worktree(root: &Path) -> bool {
    if !git_ok(root, &["init", "-q"]) {
        return false;
    }
    git_ok(root, &["config", "user.email", "test@example.com"]);
    git_ok(root, &["config", "user.name", "Test"]);
    create_test_project(root);
    fs::write(root.join(".gitignore"), "frame/.*\n.xdg-config/\n").unwrap();
    git_ok(root, &["add", "-A"]);
    git_ok(root, &["commit", "-qm", "base"]);
    git_ok(root, &["worktree", "add", "-q", "../wt"])
}

#[test]
fn a_linked_worktree_reads_the_entries_the_main_tree_wrote() {
    let tmp = tempfile::TempDir::new().unwrap();
    let main = tmp.path().join("main");
    fs::create_dir_all(&main).unwrap();
    if !repo_with_worktree(&main) {
        return; // git unavailable
    }
    let wt = tmp.path().join("wt");

    // Something irreplaceable, written from the main working tree.
    run_fr_ok(&main, &["delete", "M-001", "--yes"]);

    // The linked worktree can see it — the case that failed.
    let listed = run_fr_ok(&wt, &["recovery"]);
    assert!(
        listed.contains("M-001"),
        "an entry written next door must be visible here:\n{listed}"
    );
    assert!(
        listed.contains("from 2 working trees") || listed.contains("Origin:"),
        "and it must say where it came from:\n{listed}"
    );

    // And the reverse direction.
    run_fr_ok(&wt, &["add", "main", "From the worktree"]);
    run_fr_ok(&wt, &["delete", "M-002", "--yes"]);
    let from_main = run_fr_ok(&main, &["recovery"]);
    assert!(
        from_main.contains("M-002"),
        "and the main tree sees the worktree's entries:\n{from_main}"
    );
}

#[test]
fn here_narrows_the_listing_to_this_working_tree() {
    let tmp = tempfile::TempDir::new().unwrap();
    let main = tmp.path().join("main");
    fs::create_dir_all(&main).unwrap();
    if !repo_with_worktree(&main) {
        return;
    }
    let wt = tmp.path().join("wt");

    run_fr_ok(&main, &["delete", "M-001", "--yes"]);
    run_fr_ok(&wt, &["add", "main", "Worktree task"]);
    run_fr_ok(&wt, &["delete", "M-002", "--yes"]);

    // Matched on the description rather than the bare id: the fixture's M-002
    // carries `dep: M-001`, so its preserved body names M-001 legitimately.
    let all = run_fr_ok(&wt, &["recovery"]);
    assert!(
        all.contains("task M-001 deleted") && all.contains("task M-002 deleted"),
        "{all}"
    );

    let here = run_fr_ok(&wt, &["recovery", "--here"]);
    assert!(
        here.contains("task M-002 deleted"),
        "its own entry stays:\n{here}"
    );
    assert!(
        !here.contains("task M-001 deleted"),
        "the other tree's entry is filtered out:\n{here}"
    );
}

/// The direct inverse of the measurement that motivated this: an ignored file
/// in a worktree is deleted by `git worktree remove`, silently and with exit 0.
/// The log must not be in that category any more.
#[test]
fn the_log_survives_the_removal_of_the_worktree_that_wrote_it() {
    let tmp = tempfile::TempDir::new().unwrap();
    let main = tmp.path().join("main");
    fs::create_dir_all(&main).unwrap();
    if !repo_with_worktree(&main) {
        return;
    }
    let wt = tmp.path().join("wt");

    run_fr_ok(&wt, &["delete", "M-001", "--yes"]);
    assert!(run_fr_ok(&wt, &["recovery"]).contains("M-001"));

    // Nothing gitignored is left in the worktree to be destroyed with it.
    assert!(
        !wt.join("frame/.recovery.log").exists(),
        "the log should not be living in the worktree at all"
    );

    assert!(
        git_ok(&main, &["worktree", "remove", "--force", "../wt"]),
        "git worktree remove should succeed"
    );
    assert!(!wt.exists(), "the worktree is gone");

    let survived = run_fr_ok(&main, &["recovery"]);
    assert!(
        survived.contains("M-001"),
        "the only copy of a deleted task must outlive the worktree that deleted it:\n{survived}"
    );
}

/// Migration: a log written by an older frame is read from the moment it is
/// found, and moved into the shared one by the next write.
#[test]
fn a_per_worktree_log_from_an_older_frame_is_absorbed() {
    let tmp = tempfile::TempDir::new().unwrap();
    let main = tmp.path().join("main");
    fs::create_dir_all(&main).unwrap();
    if !repo_with_worktree(&main) {
        return;
    }

    write_numbered_log(&main, 3);
    let legacy = main.join("frame/.recovery.log");
    assert!(legacy.exists());

    // Visible before anything writes.
    let before = run_fr_ok(&main, &["recovery"]);
    assert!(before.contains("task M-2 deleted"), "{before}");
    assert!(legacy.exists(), "a read must not move it");

    // The next write brings it across and takes the old file away.
    run_fr_ok(&main, &["delete", "M-001", "--yes"]);
    assert!(!legacy.exists(), "absorbed");

    let after = run_fr_ok(&main, &["recovery"]);
    for expected in ["task M-0 deleted", "task M-2 deleted", "M-001"] {
        assert!(after.contains(expected), "{expected} missing:\n{after}");
    }

    // And a second write does not re-add anything.
    run_fr_ok(&main, &["add", "main", "Another"]);
    run_fr_ok(&main, &["delete", "M-003", "--yes"]);
    let again = run_fr_ok(&main, &["recovery", "--limit", "50"]);
    assert_eq!(
        again.matches("task M-0 deleted").count(),
        1,
        "absorbed entries must not duplicate:\n{again}"
    );
}

// ---------------------------------------------------------------------------
// Where the recovery log lives: `[recovery] path` and `FRAME_RECOVERY_LOG`
//
// The environment cases run through a real subprocess deliberately. Cargo runs
// tests in parallel threads inside one process, so `std::env::set_var` is a
// race against every other test, not a fixture.
// ---------------------------------------------------------------------------

/// Append a `[recovery]` section to the fixture's `project.toml`.
fn set_recovery_config(root: &Path, body: &str) {
    let path = root.join("frame/project.toml");
    let mut text = fs::read_to_string(&path).unwrap();
    text.push_str(&format!("\n[recovery]\n{body}"));
    fs::write(&path, text).unwrap();
}

#[test]
fn recovery_path_reports_the_default_location() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());

    let out = run_fr_ok(tmp.path(), &["recovery", "path"]);
    assert!(
        out.trim().ends_with("frame/.recovery.log"),
        "unexpected default: {out}"
    );
    assert!(
        Path::new(out.trim()).is_absolute(),
        "the path must be absolute — it is quoted to people standing elsewhere: {out}"
    );
}

#[test]
fn a_configured_relative_path_moves_the_log_and_recovery_path_says_so() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());
    set_recovery_config(tmp.path(), "path = \"logs/frame.log\"\n");

    let reported = run_fr_ok(tmp.path(), &["recovery", "path"]);
    assert!(
        reported.trim().ends_with("/logs/frame.log"),
        "a relative path resolves against the project root: {reported}"
    );

    // And a real write lands there rather than in frame/. (Compared by
    // existence rather than by string: macOS resolves the temp dir through
    // /private, so the reported path and the fixture's differ harmlessly.)
    run_fr_ok(tmp.path(), &["delete", "M-001", "--yes"]);
    assert!(tmp.path().join("logs/frame.log").exists());
    assert!(!tmp.path().join("frame/.recovery.log").exists());

    let listed = run_fr_ok(tmp.path(), &["recovery"]);
    assert!(listed.contains("M-001"), "{listed}");
}

#[test]
fn the_environment_overrides_the_configured_path() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());
    set_recovery_config(tmp.path(), "path = \"logs/from-config.log\"\n");

    let elsewhere = tmp.path().join("elsewhere/from-env.log");
    let env = [(
        "FRAME_RECOVERY_LOG",
        elsewhere.to_str().expect("utf-8 temp path"),
    )];

    let (reported, _, ok) = run_fr_env(tmp.path(), &["recovery", "path"], &env);
    assert!(ok);
    assert_eq!(Path::new(reported.trim()), elsewhere);

    let (_, _, ok) = run_fr_env(tmp.path(), &["delete", "M-001", "--yes"], &env);
    assert!(ok);
    assert!(elsewhere.exists(), "the entry went where the env said");
    assert!(!tmp.path().join("logs/from-config.log").exists());
}

#[test]
fn a_configured_size_and_age_are_accepted_by_the_real_binary() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());
    set_recovery_config(tmp.path(), "max_size = \"64KB\"\nprune_age_days = 7\n");

    // Any command that loads the config would fail on a bad value.
    run_fr_ok(tmp.path(), &["check"]);
    run_fr_ok(tmp.path(), &["recovery"]);
}

/// A bad size is a config error, and it belongs on the strict path — every
/// command that reads `project.toml` — not swallowed by the log writer.
#[test]
fn an_unparseable_size_is_reported_rather_than_ignored() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());
    set_recovery_config(tmp.path(), "max_size = \"5 megabytes\"\n");

    let (_, stderr, ok) = run_fr(tmp.path(), &["check"]);
    assert!(!ok, "a malformed size must not pass silently");
    assert!(
        stderr.contains("size") || stderr.contains("max_size"),
        "the error should name the setting: {stderr}"
    );
}

// ---------------------------------------------------------------------------
// `fr check` verifies the recovery-log pointer before printing it
//
// The `conflict:` marker is written into the track file and committed, so it
// travels to every clone and every worktree. The recovery log holding the other
// side is working-copy-local and does not. So a marker can perfectly well
// arrive somewhere the entry never will, and telling the reader "their version
// is in the recovery log" then sends them looking for something that is not
// there — which is exactly what happened.
// ---------------------------------------------------------------------------

/// Put a `conflict:` marker on M-001, stamped `2026-08-06T06:18:30Z`.
fn mark_conflicted(root: &Path) {
    let path = root.join("frame/tracks/main.md");
    let before = fs::read_to_string(&path).unwrap();
    let after = before.replace(
        "- [ ] `M-001` First task #core\n",
        "- [ ] `M-001` First task #core\n  - conflict: both-edited 2026-08-06T06:18:30Z\n",
    );
    assert_ne!(before, after, "fixture shape changed");
    fs::write(&path, after).unwrap();
}

/// A log holding one conflict entry stamped to match `mark_conflicted`.
fn write_matching_conflict_entry(root: &Path) {
    fs::write(
        root.join("frame/.recovery.log"),
        "\
<!-- frame recovery log — append-only error recovery data
     Safe to delete if empty or stale. -->

---
## 2026-08-06T06:18:30Z — conflict: merge conflict on #M-001 in tracks/main.md — kept our version

Reason: both sides edited it differently; kept ours

```text
- [ ] `M-001` Their version of the first task
```

---
",
    )
    .unwrap();
}

#[test]
fn check_points_at_the_recovery_entry_when_it_is_actually_here() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());
    mark_conflicted(tmp.path());
    write_matching_conflict_entry(tmp.path());

    let (out, _, ok) = run_fr(tmp.path(), &["check"]);
    assert!(!ok, "an unresolved conflict is still an error");
    assert!(
        out.contains("their version is in the recovery log (`fr recovery --for M-001`)"),
        "the pointer should name the lookup that finds it:\n{out}"
    );
}

#[test]
fn check_says_so_when_the_recovery_entry_is_not_here() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());
    mark_conflicted(tmp.path());
    // No recovery log at all: the marker came from someone else's merge.

    let (out, _, ok) = run_fr(tmp.path(), &["check"]);
    assert!(!ok, "an unresolved conflict is still an error");
    assert!(
        out.contains("NOT in this working copy's recovery log"),
        "a pointer frame cannot honour must not be printed as fact:\n{out}"
    );
    assert!(
        out.contains("recover it from version control"),
        "and the reader needs somewhere else to go:\n{out}"
    );
    assert!(
        !out.contains("their version is in the recovery log"),
        "the two messages must not both appear:\n{out}"
    );
}

/// A pruned log is the same situation as a foreign one: the entry is gone, and
/// the message has to stop claiming otherwise.
#[test]
fn check_stops_pointing_at_the_log_once_the_entry_is_pruned() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());
    mark_conflicted(tmp.path());
    write_matching_conflict_entry(tmp.path());

    let (before, _, _) = run_fr(tmp.path(), &["check"]);
    assert!(
        before.contains("their version is in the recovery log"),
        "{before}"
    );

    run_fr_ok(tmp.path(), &["recovery", "prune", "--all"]);

    let (after, _, _) = run_fr(tmp.path(), &["check"]);
    assert!(
        after.contains("NOT in this working copy's recovery log"),
        "after a prune the entry is gone and the message must say so:\n{after}"
    );
}

#[test]
fn check_json_reports_whether_the_conflict_evidence_is_here() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());
    mark_conflicted(tmp.path());

    let (out, _, _) = run_fr(tmp.path(), &["check", "--json"]);
    let parsed: serde_json::Value = serde_json::from_str(&out).expect("valid JSON");
    let conflict = parsed["errors"]
        .as_array()
        .expect("errors array")
        .iter()
        .find(|e| e["type"] == "unresolved_merge_conflict")
        .expect("the conflict error");
    assert_eq!(conflict["evidence"], serde_json::json!(false));

    write_matching_conflict_entry(tmp.path());
    let (out, _, _) = run_fr(tmp.path(), &["check", "--json"]);
    let parsed: serde_json::Value = serde_json::from_str(&out).expect("valid JSON");
    let conflict = parsed["errors"]
        .as_array()
        .expect("errors array")
        .iter()
        .find(|e| e["type"] == "unresolved_merge_conflict")
        .expect("the conflict error");
    assert_eq!(conflict["evidence"], serde_json::json!(true));
}

/// The fallback path: an entry whose timestamp no longer matches the marker
/// (a hand edit, a rewritten log) still counts when its content names the task.
#[test]
fn check_finds_the_evidence_by_task_id_when_the_timestamp_does_not_match() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());
    mark_conflicted(tmp.path());
    fs::write(
        tmp.path().join("frame/.recovery.log"),
        "\
<!-- frame recovery log -->

---
## 2026-08-06T09:99:99Z — conflict: merge conflict on #M-001 in tracks/main.md — kept our version

Reason: both sides edited it differently; kept ours

---
"
        .replace("09:99:99", "09:41:02"),
    )
    .unwrap();

    let (out, _, _) = run_fr(tmp.path(), &["check"]);
    assert!(
        out.contains("their version is in the recovery log"),
        "an entry naming the task counts even when the stamp moved:\n{out}"
    );
}

/// One merge run stamps every task it marks and every entry it logs with the
/// same instant, so a marker's stamp cannot say whether *this* task's version is
/// in the log — only that the run logged something. When the two come apart (a
/// per-entry log write that failed partway through the run, a second driver
/// invocation in the same second that found no project to log into), the
/// unlogged task must not borrow its sibling's stamp and claim evidence.
#[test]
fn check_does_not_let_one_conflict_vouch_for_its_merge_siblings() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());

    // Two tasks marked by one run: same reason, same stamp, as `fr merge` writes.
    let path = tmp.path().join("frame/tracks/main.md");
    let before = fs::read_to_string(&path).unwrap();
    let after = before
        .replace(
            "- [ ] `M-001` First task #core\n",
            "- [ ] `M-001` First task #core\n  - conflict: both-edited 2026-08-06T06:18:30Z\n",
        )
        .replace(
            "- [>] `M-002` Second task #core #cc\n",
            "- [>] `M-002` Second task #core #cc\n  - conflict: both-edited 2026-08-06T06:18:30Z\n",
        );
    assert_ne!(before, after, "fixture shape changed");
    fs::write(&path, after).unwrap();

    // Only M-001's version reached the log.
    write_matching_conflict_entry(tmp.path());

    let (out, _, _) = run_fr(tmp.path(), &["check"]);
    let line = |id: &str| {
        out.lines()
            .find(|l| l.contains(&format!("{id} has an unresolved merge conflict")))
            .unwrap_or_else(|| panic!("no conflict line for {id}:\n{out}"))
            .to_string()
    };

    assert!(
        line("M-001").contains("their version is in the recovery log"),
        "the logged task still has its pointer:\n{}",
        line("M-001")
    );
    assert!(
        line("M-002").contains("NOT in this working copy's recovery log"),
        "a shared stamp is not evidence for a task the log never names:\n{}",
        line("M-002")
    );
}

#[test]
fn recovery_says_how_many_entries_it_is_hiding() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());
    write_numbered_log(tmp.path(), 18);

    let out = run_fr_ok(tmp.path(), &["recovery"]);
    assert!(
        out.contains("showing 10 of 18"),
        "a truncated listing must say so:\n{out}"
    );
    assert!(
        out.contains("--limit 18"),
        "and must say how to see the rest:\n{out}"
    );
}

#[test]
fn recovery_says_nothing_about_truncation_when_nothing_is_truncated() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());
    write_numbered_log(tmp.path(), 8);

    let out = run_fr_ok(tmp.path(), &["recovery"]);
    assert!(
        !out.contains("showing"),
        "a complete listing must not imply there is more:\n{out}"
    );
}

#[test]
fn recovery_for_an_id_finds_the_entry_the_default_limit_hides() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());
    write_numbered_log(tmp.path(), 18);

    // M-0 is the oldest, so the default listing cannot reach it.
    let listed = run_fr_ok(tmp.path(), &["recovery"]);
    assert!(!listed.contains("task M-0 deleted"), "{listed}");

    let found = run_fr_ok(tmp.path(), &["recovery", "--for", "M-0"]);
    assert!(
        found.contains("task M-0 deleted"),
        "--for must reach past the default limit:\n{found}"
    );
    // Boundary-aware: M-0 is not M-10.
    assert!(
        !found.contains("task M-10 deleted"),
        "--for must not match a longer id that starts the same:\n{found}"
    );
}

#[test]
fn recovery_for_a_missing_id_says_so_by_name() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());
    write_numbered_log(tmp.path(), 3);

    let out = run_fr_ok(tmp.path(), &["recovery", "--for", "M-999"]);
    assert!(
        out.contains("No recovery log entries for M-999"),
        "an empty result must name what was looked for:\n{out}"
    );
}

#[test]
fn recovery_for_a_conflict_marker_timestamp_selects_that_entry() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());
    write_numbered_log(tmp.path(), 3);

    // The form a `conflict:` marker carries, per doc/format.md.
    let out = run_fr_ok(tmp.path(), &["recovery", "--for", "2026-02-10T01:00:00Z"]);
    assert!(out.contains("task M-1 deleted"), "{out}");
    assert!(!out.contains("task M-2 deleted"), "{out}");
}

#[test]
fn recovery_json_carries_the_entries_and_not_the_notice() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());
    write_numbered_log(tmp.path(), 18);

    let out = run_fr_ok(tmp.path(), &["recovery", "--json"]);
    let parsed: serde_json::Value = serde_json::from_str(&out).expect("valid JSON");
    assert_eq!(
        parsed.as_array().map(|a| a.len()),
        Some(10),
        "the array must hold exactly what the notice counts as shown"
    );
    assert!(
        !out.contains("showing 10 of 18"),
        "the notice is human-only; it must not corrupt the JSON payload:\n{out}"
    );
}

#[test]
fn test_check_with_lost_task() {
    let tmp = tempfile::TempDir::new().unwrap();
    let frame_dir = tmp.path().join("frame");
    fs::create_dir_all(frame_dir.join("tracks")).unwrap();

    fs::write(
        frame_dir.join("project.toml"),
        r#"[project]
name = "test-project"

[[tracks]]
id = "main"
name = "Main Track"
state = "active"
file = "tracks/main.md"

[ids.prefixes]
main = "M"
"#,
    )
    .unwrap();

    fs::write(
        frame_dir.join("tracks/main.md"),
        "\
# Main Track

## Backlog

- [!] `M-001` Recovered task #lost
  - added: 2025-05-01

## Done
",
    )
    .unwrap();

    fs::write(frame_dir.join("inbox.md"), "# Inbox\n").unwrap();

    let out = run_fr_ok(tmp.path(), &["check"]);
    assert!(out.contains("#lost") || out.contains("lost"));
}

#[test]
fn test_check_json_with_recovery_log() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());

    // Create a recovery log entry
    let recovery_path = tmp.path().join("frame/.recovery.log");
    let content = "\
<!-- frame recovery log — append-only error recovery data
     This file captures data that Frame couldn't save normally.
     If something went missing, check here.
     View with: fr recovery
     Prune old entries: fr recovery prune
     Safe to delete if empty or stale. -->

---
## 2026-02-10T12:00:00Z — write: test

---
";
    fs::write(&recovery_path, content).unwrap();

    let out = run_fr_ok(tmp.path(), &["check", "--json"]);
    let parsed: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert!(parsed["info"].is_array());
    let info = parsed["info"].as_array().unwrap();
    assert!(info.iter().any(|i| i["type"] == "recovery_log"));
}

// ---------------------------------------------------------------------------
// Actor token tests
// ---------------------------------------------------------------------------

#[test]
fn test_init_claims_null_and_writes_both_files() {
    let tmp = tempfile::TempDir::new().unwrap();
    run_fr_ok(tmp.path(), &["init", "--name", "Tokened"]);

    // actors.toml exists with null claimed active
    let actors = fs::read_to_string(tmp.path().join("frame/actors.toml")).unwrap();
    let parsed: toml::Value = toml::from_str(&actors).unwrap();
    assert_eq!(
        parsed["actors"]["null"]["state"].as_str().unwrap(),
        "active"
    );

    // .actor points to null
    let actor = fs::read_to_string(tmp.path().join("frame/.actor")).unwrap();
    assert_eq!(actor.trim(), "null");
}

#[test]
fn test_init_force_does_not_clobber_actors() {
    let tmp = tempfile::TempDir::new().unwrap();
    run_fr_ok(tmp.path(), &["init", "--name", "First"]);

    // Mutate the registry, then reinit with --force.
    run_fr_ok(tmp.path(), &["actor", "set", "a", "--name", "mine"]);
    run_fr_ok(tmp.path(), &["init", "--name", "Second", "--force"]);

    // The registry survived the reinit.
    let actors = fs::read_to_string(tmp.path().join("frame/actors.toml")).unwrap();
    assert!(
        actors.contains("\na = {"),
        "actors.toml clobbered: {actors}"
    );
    assert!(actors.contains("mine"));
}

#[test]
fn test_actor_status_missing_registry_reports_unclaimed() {
    // The migration case: no actors.toml and no `.actor`. Remove the primary
    // `.actor` the helper writes to model a pre-actors legacy project.
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());
    fs::remove_file(tmp.path().join("frame/.actor")).unwrap();

    let (stdout, _stderr, success) = run_fr(tmp.path(), &["actor"]);
    assert!(
        success,
        "fr actor should not error on a registry-less project"
    );
    assert!(stdout.contains("unclaimed"), "stdout: {stdout}");
    // No file was created by a read-only status check.
    assert!(!tmp.path().join("frame/actors.toml").exists());
}

#[test]
fn test_first_mint_auto_claims_token() {
    // A fresh clone of an existing project has no `.actor`; the first `fr add`
    // auto-claims a letter token, announces it once, and mints in that namespace.
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());
    fs::remove_file(tmp.path().join("frame/.actor")).unwrap();

    let (stdout, stderr, success) = run_fr(tmp.path(), &["add", "main", "First in fresh clone"]);
    assert!(success, "stderr: {stderr}");

    // The minted ID is tokened (e.g. `M-e1`), not a bare null-namespace number.
    let id = stdout.trim();
    assert!(
        id.starts_with("M-") && id.chars().nth(2).is_some_and(|c| c.is_ascii_alphabetic()),
        "expected a tokened id, got {id}"
    );
    // Announced exactly once, to stderr (stdout stays clean for the id).
    assert!(stderr.contains("Claimed actor token"), "stderr: {stderr}");

    // `.actor` and the registry row were persisted.
    let token = fs::read_to_string(tmp.path().join("frame/.actor"))
        .unwrap()
        .trim()
        .to_string();
    assert_ne!(token, "null");
    assert_eq!(id, format!("M-{token}1"));
    let registry = fs::read_to_string(tmp.path().join("frame/actors.toml")).unwrap();
    assert!(registry.contains(&format!("\n{token} = {{")), "{registry}");

    // A second mint does not re-announce (token already claimed).
    let (_stdout2, stderr2, success2) = run_fr(tmp.path(), &["add", "main", "Second"]);
    assert!(success2);
    assert!(
        !stderr2.contains("Claimed actor token"),
        "stderr2: {stderr2}"
    );
}

#[test]
fn test_dry_run_clean_on_unclaimed_clone_claims_nothing() {
    // Strict null policy on an unclaimed clone, under `fr clean --dry-run`: the
    // token is not claimed and no ID reaches a file.
    //
    // **What it previews changed, and deliberately.** This used to compute the
    // preview with `IdScope::Unclaimed` — so it reported no IDs assigned, while
    // the real `fr clean` on the same clone claims a letter token and assigns
    // them. A preview of a *different* command's behaviour is worse than none,
    // and it was the same missing distinction that let the old dry run advance
    // the ID frontier for real. It now computes exactly what the real run would,
    // and the write barrier is what keeps that off the disk. The ID is minted in
    // a claimed letter namespace, never the null one, which is the policy.
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());
    fs::remove_file(tmp.path().join("frame/.actor")).unwrap();

    // Give the track an ID-less task.
    let main_path = tmp.path().join("frame/tracks/main.md");
    let main = fs::read_to_string(&main_path).unwrap();
    let before = main.replace("## Backlog\n", "## Backlog\n\n- [ ] Task with no id\n");
    fs::write(&main_path, &before).unwrap();

    let (stdout, _stderr, success) = run_fr(tmp.path(), &["clean", "--dry-run"]);
    assert!(success);

    // The ID it would assign is in a letter namespace, not the null one it does
    // not own.
    assert!(stdout.contains("IDs assigned"), "{stdout}");
    assert!(
        !stdout.contains("→ \"Task with no id\"") || !stdout.contains("[main] M-1 "),
        "an unclaimed clone must never preview a null-namespace id: {stdout}"
    );

    // And none of it happened: no claim, no registry, no rewritten track.
    assert!(!tmp.path().join("frame/.actor").exists());
    assert!(!tmp.path().join("frame/actors.toml").exists());
    assert!(!tmp.path().join("frame/.ids.toml").exists());
    assert_eq!(fs::read_to_string(&main_path).unwrap(), before);
}

#[test]
fn test_mint_errors_when_frontier_empty_and_unclaimed() {
    // No `.actor`, and every safe token is already taken: a mint must fail with
    // the routing message and create nothing.
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());
    fs::remove_file(tmp.path().join("frame/.actor")).unwrap();

    // Fill the entire safe alphabet in the registry so the frontier is empty.
    let alphabet = [
        "a", "b", "c", "d", "e", "f", "g", "h", "j", "k", "m", "n", "p", "q", "r", "s", "t", "u",
        "v", "w", "x", "y", "z",
    ];
    let mut registry = String::new();
    for t in alphabet {
        registry.push_str(&format!(
            "[actors.{t}]\nname = \"other\"\nstate = \"active\"\nclaimed = \"2026-01-01\"\n\n"
        ));
    }
    fs::write(tmp.path().join("frame/actors.toml"), registry).unwrap();

    let track_before = fs::read_to_string(tmp.path().join("frame/tracks/main.md")).unwrap();
    let (_stdout, stderr, success) = run_fr(tmp.path(), &["add", "main", "Should not be created"]);
    assert!(!success, "mint should fail when no token can be claimed");
    assert!(stderr.contains("fr actor set"), "stderr: {stderr}");

    // Nothing was created or claimed.
    let track_after = fs::read_to_string(tmp.path().join("frame/tracks/main.md")).unwrap();
    assert_eq!(track_before, track_after);
    assert!(!track_after.contains("Should not be created"));
    assert!(!tmp.path().join("frame/.actor").exists());
}

#[test]
fn test_mv_cross_track_mints_in_movers_namespace() {
    // A clone that has claimed token c moves M-001 to the side track. The new id
    // is scanned in c's namespace on the target (which holds only null ids), so
    // it lands as S-c1 — not S-003 and not a token belonging to another actor.
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());
    run_fr_ok(tmp.path(), &["actor", "set", "c"]);

    let out = run_fr_ok(tmp.path(), &["mv", "M-001", "--track", "side"]);
    assert!(out.contains("S-c1"), "out: {out}");

    let side = fs::read_to_string(tmp.path().join("frame/tracks/side.md")).unwrap();
    assert!(side.contains("S-c1"), "side: {side}");
    let main = fs::read_to_string(tmp.path().join("frame/tracks/main.md")).unwrap();
    assert!(!main.contains("First task"), "M-001 should have moved out");
}

#[test]
fn test_mv_promote_mints_in_movers_namespace() {
    // A clone that has claimed token c promotes M-003.1 to top-level. The new
    // top-level id is minted in c's namespace: M-c1.
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());
    run_fr_ok(tmp.path(), &["actor", "set", "c"]);

    let out = run_fr_ok(tmp.path(), &["mv", "M-003.1", "--promote"]);
    assert!(out.contains("M-c1"), "out: {out}");
}

#[test]
fn test_cross_track_move_aborts_when_frontier_empty_and_unclaimed() {
    // An unclaimed clone with an exhausted frontier attempting a cross-track move
    // must fail with the routing message and leave BOTH source and target tracks
    // unchanged (no partial mutation).
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());
    fs::remove_file(tmp.path().join("frame/.actor")).unwrap();

    // Fill the entire safe alphabet so the frontier is empty.
    let alphabet = [
        "a", "b", "c", "d", "e", "f", "g", "h", "j", "k", "m", "n", "p", "q", "r", "s", "t", "u",
        "v", "w", "x", "y", "z",
    ];
    let mut registry = String::new();
    for t in alphabet {
        registry.push_str(&format!(
            "[actors.{t}]\nname = \"other\"\nstate = \"active\"\nclaimed = \"2026-01-01\"\n\n"
        ));
    }
    fs::write(tmp.path().join("frame/actors.toml"), registry).unwrap();

    let main_before = fs::read_to_string(tmp.path().join("frame/tracks/main.md")).unwrap();
    let side_before = fs::read_to_string(tmp.path().join("frame/tracks/side.md")).unwrap();

    let (_stdout, stderr, success) = run_fr(tmp.path(), &["mv", "M-001", "--track", "side"]);
    assert!(
        !success,
        "cross-track move should fail when no token is claimable"
    );
    assert!(stderr.contains("fr actor set"), "stderr: {stderr}");

    // Neither track changed, and nothing was claimed.
    assert_eq!(
        main_before,
        fs::read_to_string(tmp.path().join("frame/tracks/main.md")).unwrap()
    );
    assert_eq!(
        side_before,
        fs::read_to_string(tmp.path().join("frame/tracks/side.md")).unwrap()
    );
    assert!(!tmp.path().join("frame/.actor").exists());
}

// ---------------------------------------------------------------------------
// CLI cross-track move updates cross-track dependency references
// ---------------------------------------------------------------------------

/// Project with four tracks and cross-track deps pointing at A-005 (the task we
/// move). `alpha` also holds A-0050, a decoy id that resembles A-005 but must not
/// be touched by a whole-id dep rewrite. Records the primary (null) actor so
/// mints stay in the legacy namespace unless a test claims a token first.
fn create_dep_project(root: &Path) {
    let frame_dir = root.join("frame");
    fs::create_dir_all(frame_dir.join("tracks")).unwrap();
    fs::write(frame_dir.join(".actor"), "null\n").unwrap();

    fs::write(
        frame_dir.join("project.toml"),
        r#"[project]
name = "dep-project"

[[tracks]]
id = "alpha"
name = "Alpha"
state = "active"
file = "tracks/alpha.md"

[[tracks]]
id = "beta"
name = "Beta"
state = "active"
file = "tracks/beta.md"

[[tracks]]
id = "gamma"
name = "Gamma"
state = "active"
file = "tracks/gamma.md"

[[tracks]]
id = "delta"
name = "Delta"
state = "active"
file = "tracks/delta.md"

[ids.prefixes]
alpha = "A"
beta = "B"
gamma = "C"
delta = "D"
"#,
    )
    .unwrap();

    fs::write(
        frame_dir.join("tracks/alpha.md"),
        "\
# Alpha

## Backlog

- [ ] `A-001` First alpha
  - added: 2025-05-01
- [ ] `A-005` Movable task
  - added: 2025-05-02
- [ ] `A-0050` Decoy with a similar id
  - added: 2025-05-03

## Done
",
    )
    .unwrap();

    fs::write(
        frame_dir.join("tracks/beta.md"),
        "\
# Beta

## Backlog

- [ ] `B-001` Depends on the movable task
  - added: 2025-05-01
  - dep: A-005
- [ ] `B-002` Depends on the decoy
  - added: 2025-05-02
  - dep: A-0050

## Done
",
    )
    .unwrap();

    fs::write(
        frame_dir.join("tracks/gamma.md"),
        "\
# Gamma

## Backlog

## Done
",
    )
    .unwrap();

    fs::write(
        frame_dir.join("tracks/delta.md"),
        "\
# Delta

## Backlog

- [ ] `D-001` Also depends on the movable task
  - added: 2025-05-01
  - dep: A-005

## Done
",
    )
    .unwrap();
}

#[test]
fn test_mv_cross_track_updates_dep_reference() {
    // Previously broken: moving A-005 to gamma re-keyed it to C-001 but left
    // B-001's `dep: A-005` dangling. The dependent must now be rewritten.
    let tmp = tempfile::TempDir::new().unwrap();
    create_dep_project(tmp.path());

    let out = run_fr_ok(tmp.path(), &["mv", "A-005", "--track", "gamma"]);
    assert!(out.contains("A-005 → C-001"), "out: {out}");

    let beta = fs::read_to_string(tmp.path().join("frame/tracks/beta.md")).unwrap();
    assert!(beta.contains("dep: C-001"), "beta: {beta}");
    assert!(!beta.contains("dep: A-005\n"), "stale dep remained: {beta}");

    let gamma = fs::read_to_string(tmp.path().join("frame/tracks/gamma.md")).unwrap();
    assert!(gamma.contains("`C-001`"), "gamma: {gamma}");
}

/// Give `A-005` a subtree, and point a dep at one of its children from each of
/// the three places a dep can live relative to a cross-track move.
fn create_subtree_dep_project(root: &Path) {
    create_dep_project(root);
    let frame_dir = root.join("frame");

    // `A-001` is in the track the subtree leaves; `A-005.2` is a sibling that
    // travels inside the subtree alongside the task it depends on.
    fs::write(
        frame_dir.join("tracks/alpha.md"),
        "\
# Alpha

## Backlog

- [ ] `A-001` Same-track dependent
  - added: 2025-05-01
  - dep: A-005.1
- [ ] `A-005` Movable parent
  - added: 2025-05-02
  - [ ] `A-005.1` Movable child
    - added: 2025-05-02
  - [ ] `A-005.2` Movable sibling
    - added: 2025-05-02
    - dep: A-005.1

## Done
",
    )
    .unwrap();

    fs::write(
        frame_dir.join("tracks/beta.md"),
        "\
# Beta

## Backlog

- [ ] `B-001` Other-track dependent
  - added: 2025-05-01
  - dep: A-005.1

## Done
",
    )
    .unwrap();
}

/// A cross-track move renumbers the whole subtree, so a dep on a *descendant*
/// has to follow it. It did not: the move rewrote the root rename and nothing
/// else, and left three dangling deps behind — one per position a dep can hold.
///
/// The third is the one with no defence: `A-005.2` depends on its own sibling
/// `A-005.1`, and both move together in the one operation.
///
/// End-to-end through `fr check` rather than by reading the files alone,
/// because `check` is what reported the damage before and `fr check --fix`
/// deliberately will not repair a dangling dep — dropping one discards intent.
/// Getting it right during the move is the only chance there is.
#[test]
fn test_mv_cross_track_rewrites_deps_on_moved_descendants() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_subtree_dep_project(tmp.path());

    // Nothing is dangling to begin with, or the check below proves nothing.
    let (before, _, ok) = run_fr(tmp.path(), &["check"]);
    assert!(ok, "fixture starts clean: {before}");
    assert!(!before.contains("dangling dep"), "fixture: {before}");

    let out = run_fr_ok(tmp.path(), &["mv", "A-005", "--track", "gamma"]);
    assert!(out.contains("A-005 → C-001"), "out: {out}");

    // 1. Another track.
    let beta = fs::read_to_string(tmp.path().join("frame/tracks/beta.md")).unwrap();
    assert!(beta.contains("dep: C-001.1"), "beta: {beta}");
    // 2. The track the subtree left.
    let alpha = fs::read_to_string(tmp.path().join("frame/tracks/alpha.md")).unwrap();
    assert!(alpha.contains("dep: C-001.1"), "alpha: {alpha}");
    // 3. Inside the moved subtree itself.
    let gamma = fs::read_to_string(tmp.path().join("frame/tracks/gamma.md")).unwrap();
    assert!(gamma.contains("`C-001.1`"), "gamma: {gamma}");
    assert!(gamma.contains("dep: C-001.1"), "gamma: {gamma}");

    assert!(
        !alpha.contains("A-005") && !beta.contains("A-005") && !gamma.contains("A-005"),
        "a retired id survived somewhere:\nalpha: {alpha}\nbeta: {beta}\ngamma: {gamma}"
    );

    let (after, _, ok) = run_fr(tmp.path(), &["check"]);
    assert!(ok, "check should pass after the move: {after}");
    assert!(!after.contains("dangling dep"), "after: {after}");
}

#[test]
fn test_mv_cross_track_updates_dep_in_movers_namespace() {
    // The dependent is rewritten to the fully tokened new id when a tokened clone
    // performs the move: A-005 → C-c1, and B-001's dep follows.
    let tmp = tempfile::TempDir::new().unwrap();
    create_dep_project(tmp.path());
    run_fr_ok(tmp.path(), &["actor", "set", "c"]);

    let out = run_fr_ok(tmp.path(), &["mv", "A-005", "--track", "gamma"]);
    assert!(out.contains("A-005 → C-c1"), "out: {out}");

    let beta = fs::read_to_string(tmp.path().join("frame/tracks/beta.md")).unwrap();
    assert!(beta.contains("dep: C-c1"), "beta: {beta}");
}

#[test]
fn test_mv_cross_track_updates_multiple_dependents() {
    // Two dependents in different tracks (beta and delta) both pointing at the
    // moved id are both updated.
    let tmp = tempfile::TempDir::new().unwrap();
    create_dep_project(tmp.path());

    run_fr_ok(tmp.path(), &["mv", "A-005", "--track", "gamma"]);

    let beta = fs::read_to_string(tmp.path().join("frame/tracks/beta.md")).unwrap();
    let delta = fs::read_to_string(tmp.path().join("frame/tracks/delta.md")).unwrap();
    assert!(beta.contains("dep: C-001"), "beta: {beta}");
    assert!(delta.contains("dep: C-001"), "delta: {delta}");
}

#[test]
fn test_mv_cross_track_no_false_dep_rewrite() {
    // A dep that merely resembles the old id (A-0050 vs A-005) must be left
    // untouched — the rewrite matches whole ids, not substrings.
    let tmp = tempfile::TempDir::new().unwrap();
    create_dep_project(tmp.path());

    run_fr_ok(tmp.path(), &["mv", "A-005", "--track", "gamma"]);

    let beta = fs::read_to_string(tmp.path().join("frame/tracks/beta.md")).unwrap();
    // B-001 (dep: A-005) was rewritten; B-002 (dep: A-0050) was not.
    assert!(beta.contains("dep: C-001"), "beta: {beta}");
    assert!(
        beta.contains("dep: A-0050"),
        "decoy dep was wrongly rewritten: {beta}"
    );
}

#[test]
fn test_mv_cross_track_then_check_clean() {
    // End-to-end guard: after the move, `fr check` reports no dangling dependency.
    let tmp = tempfile::TempDir::new().unwrap();
    create_dep_project(tmp.path());

    run_fr_ok(tmp.path(), &["mv", "A-005", "--track", "gamma"]);

    let check = run_fr_ok(tmp.path(), &["check"]);
    assert!(check.contains("✓ project is valid"), "check: {check}");
    assert!(
        !check.contains("dangling"),
        "check reported a dangling dep: {check}"
    );
}

#[test]
fn test_actor_set_null_creates_registry_on_legacy_project() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());

    run_fr_ok(tmp.path(), &["actor", "set", "null"]);

    assert!(tmp.path().join("frame/actors.toml").exists());
    let actor = fs::read_to_string(tmp.path().join("frame/.actor")).unwrap();
    assert_eq!(actor.trim(), "null");
}

#[test]
fn test_actor_claim_picks_a_token() {
    let tmp = tempfile::TempDir::new().unwrap();
    run_fr_ok(tmp.path(), &["init", "--name", "Claimer"]);

    let out = run_fr_ok(tmp.path(), &["actor", "claim"]);
    assert!(out.contains("claimed token"), "out: {out}");

    // .actor now holds a single safe letter (not null anymore).
    let actor = fs::read_to_string(tmp.path().join("frame/.actor"))
        .unwrap()
        .trim()
        .to_string();
    assert_ne!(actor, "null");
    assert_eq!(actor.len(), 1);
}

#[test]
fn test_actor_set_rejects_invalid_token() {
    let tmp = tempfile::TempDir::new().unwrap();
    run_fr_ok(tmp.path(), &["init", "--name", "Strict"]);

    // Uppercase rejected.
    let (_o, _e, ok_upper) = run_fr(tmp.path(), &["actor", "set", "A"]);
    assert!(!ok_upper);
    // Single 'i' rejected (not in safe alphabet).
    let (_o, _e, ok_i) = run_fr(tmp.path(), &["actor", "set", "i"]);
    assert!(!ok_i);
}

#[test]
fn test_actor_retire_then_reclaim() {
    let tmp = tempfile::TempDir::new().unwrap();
    run_fr_ok(tmp.path(), &["init", "--name", "Retirer"]);
    run_fr_ok(tmp.path(), &["actor", "set", "a"]);

    run_fr_ok(tmp.path(), &["actor", "retire", "a"]);
    let listing = run_fr_ok(tmp.path(), &["actor", "list"]);
    assert!(listing.contains("retired"), "list: {listing}");

    // Reclaim flips it back to active.
    let out = run_fr_ok(tmp.path(), &["actor", "set", "a"]);
    assert!(out.contains("reclaimed"), "out: {out}");
}

#[test]
fn test_actor_list_json() {
    let tmp = tempfile::TempDir::new().unwrap();
    run_fr_ok(tmp.path(), &["init", "--name", "Lister"]);

    let out = run_fr_ok(tmp.path(), &["actor", "list", "--json"]);
    let parsed: serde_json::Value = serde_json::from_str(&out).unwrap();
    let rows = parsed.as_array().unwrap();
    assert!(rows.iter().any(|r| r["token"] == "null"));
}

#[test]
fn test_actor_set_owned_by_another_refused() {
    let tmp = tempfile::TempDir::new().unwrap();
    run_fr_ok(tmp.path(), &["init", "--name", "Owner"]);

    // Hand-build a registry where 'a' is active but owned by a different clone
    // (this clone's .actor is null, not a).
    let actors_path = tmp.path().join("frame/actors.toml");
    let mut content = fs::read_to_string(&actors_path).unwrap();
    content.push_str(
        "\n[actors.a]\nname = \"other-machine\"\nstate = \"active\"\nclaimed = \"2026-06-01\"\n",
    );
    fs::write(&actors_path, content).unwrap();

    let (stdout, stderr, success) = run_fr(tmp.path(), &["actor", "set", "a"]);
    assert!(!success);
    let combined = format!("{stdout}{stderr}");
    assert!(combined.contains("already claimed"), "combined: {combined}");
}

// ---------------------------------------------------------------------------
// `fr info` tests
// ---------------------------------------------------------------------------

#[test]
fn test_info_human_primary() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path()); // .actor = null

    let out = run_fr_ok(tmp.path(), &["info"]);
    assert!(out.contains("version"), "out: {out}");
    assert!(out.contains(env!("CARGO_PKG_VERSION")), "out: {out}");
    assert!(out.contains("test-project"), "out: {out}");
    // null renders as the human-friendly "primary".
    assert!(out.contains("actor"), "out: {out}");
    assert!(out.contains("primary"), "out: {out}");
    assert!(
        !out.contains("null"),
        "human output should not show literal null: {out}"
    );
    // Two active tracks (main, side).
    assert!(out.contains("tracks"), "out: {out}");
    assert!(out.contains('2'), "out: {out}");
}

/// `fr info` says outright which working copy it is in. The project name cannot
/// — it is committed, so every worktree of a clone reports the same one — and
/// `frame_dir` only implies it.
#[test]
fn test_info_names_the_worktree_and_its_main_tree() {
    let base = tempfile::TempDir::new().unwrap();
    let main = base.path().join("main");
    let Some(()) = repo_project(&main) else {
        return; // git unavailable
    };
    let wt = base.path().join("wt-info");
    assert!(git(
        &main,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "info-branch",
            wt.to_str().unwrap()
        ]
    ));

    let human = run_fr_ok(&wt, &["info"]);
    assert!(
        human.contains("worktree   info-branch"),
        "names the branch: {human}"
    );
    assert!(
        human.contains("linked worktree; main tree"),
        "and where the clone's shared state lives: {human}"
    );

    let json: serde_json::Value =
        serde_json::from_str(&run_fr_ok(&wt, &["info", "--json"])).unwrap();
    assert_eq!(json["worktree"], "info-branch");
    let main_tree = json["main_worktree"].as_str().unwrap();
    assert!(
        Path::new(main_tree).ends_with("main"),
        "main_worktree: {main_tree}"
    );

    // The main working tree has nothing to distinguish, so it says nothing —
    // and the JSON keys are present-but-null rather than absent, so a consumer
    // can tell "main tree" from "old frame that did not report this".
    let from_main = run_fr_ok(&main, &["info"]);
    assert!(
        !from_main.contains("worktree"),
        "no worktree line in the main tree: {from_main}"
    );
    let main_json: serde_json::Value =
        serde_json::from_str(&run_fr_ok(&main, &["info", "--json"])).unwrap();
    assert!(main_json["worktree"].is_null(), "{main_json}");
    assert!(main_json["main_worktree"].is_null(), "{main_json}");
}

/// A detached worktree has no branch to name, so it falls back to its directory
/// name — which still differs between the worktrees of one clone.
#[test]
fn test_info_labels_a_detached_worktree_by_directory() {
    let base = tempfile::TempDir::new().unwrap();
    let main = base.path().join("main");
    let Some(()) = repo_project(&main) else {
        return; // git unavailable
    };
    let wt = base.path().join("wt-detached");
    assert!(git(
        &main,
        &["worktree", "add", "-q", "--detach", wt.to_str().unwrap()]
    ));

    let json: serde_json::Value =
        serde_json::from_str(&run_fr_ok(&wt, &["info", "--json"])).unwrap();
    assert_eq!(json["worktree"], "wt-detached");
}

#[test]
fn test_info_json_primary() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path()); // .actor = null

    let out = run_fr_ok(tmp.path(), &["info", "--json"]);
    let parsed: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(parsed["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(parsed["project"], "test-project");
    assert_eq!(parsed["actor"], "null"); // primary is the literal string "null"
    assert_eq!(parsed["tracks"], 2);
    let frame_dir = parsed["frame_dir"].as_str().unwrap();
    assert!(frame_dir.ends_with("frame"), "frame_dir: {frame_dir}");
    assert!(
        Path::new(frame_dir).is_absolute(),
        "frame_dir should be absolute: {frame_dir}"
    );
}

#[test]
fn test_info_json_tokened() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());
    fs::write(tmp.path().join("frame/.actor"), "a\n").unwrap();

    let out = run_fr_ok(tmp.path(), &["info", "--json"]);
    let parsed: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert_eq!(parsed["actor"], "a");
}

#[test]
fn test_info_json_unclaimed_is_read_only() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());
    // Remove the .actor file so the clone is unclaimed.
    fs::remove_file(tmp.path().join("frame/.actor")).unwrap();
    assert!(!tmp.path().join("frame/actors.toml").exists());

    let out = run_fr_ok(tmp.path(), &["info", "--json"]);
    let parsed: serde_json::Value = serde_json::from_str(&out).unwrap();
    // Unclaimed distinguishes as JSON null.
    assert!(
        parsed["actor"].is_null(),
        "actor should be JSON null: {out}"
    );

    // Read-only invariant: running info must not claim a token.
    assert!(
        !tmp.path().join("frame/.actor").exists(),
        "fr info must not create .actor"
    );
    assert!(
        !tmp.path().join("frame/actors.toml").exists(),
        "fr info must not create actors.toml"
    );
}

#[test]
fn test_info_human_unclaimed() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());
    fs::remove_file(tmp.path().join("frame/.actor")).unwrap();

    let out = run_fr_ok(tmp.path(), &["info"]);
    assert!(out.contains("unclaimed"), "out: {out}");
}

// ---------------------------------------------------------------------------
// ID frontier — durable and shared across worktrees of a clone
// ---------------------------------------------------------------------------

fn git(dir: &Path, args: &[&str]) -> bool {
    Command::new("git")
        .current_dir(dir)
        .args(args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// A clone whose frame project is committed, plus a linked worktree of it —
/// the shape that used to mint duplicate IDs. `.actor` stays gitignored, so the
/// worktree inherits the main tree's `null` token and both mint in one namespace.
/// `None` when git is unavailable.
fn clone_with_worktree(tmp: &Path) -> Option<(PathBuf, PathBuf)> {
    let main = tmp.join("main");
    fs::create_dir_all(&main).unwrap();
    create_test_project(&main);
    let ignore: String = frame::io::project_io::LOCAL_ONLY_FRAME_FILES
        .iter()
        .map(|name| format!("frame/{}\n", name))
        .collect();
    fs::write(main.join(".gitignore"), ignore).unwrap();

    if !git(&main, &["init", "-q"]) {
        return None;
    }
    git(&main, &["add", "-A"]);
    let committed = git(
        &main,
        &[
            "-c",
            "user.name=frame-test",
            "-c",
            "user.email=frame@test.invalid",
            "commit",
            "-q",
            "-m",
            "init",
        ],
    );
    if !committed {
        return None;
    }
    let worktree = tmp.join("wt");
    if !git(
        &main,
        &["worktree", "add", "-q", "--detach", worktree.to_str()?],
    ) {
        return None;
    }
    Some((main, worktree))
}

/// The reported bug: two worktrees of one clone, each scanning only its own
/// working copy, minting the same ID. The clone-shared frontier prevents it even
/// though the first task is still uncommitted and invisible to the second tree.
#[test]
fn test_worktrees_of_one_clone_do_not_mint_the_same_id() {
    let tmp = tempfile::TempDir::new().unwrap();
    let Some((main, worktree)) = clone_with_worktree(tmp.path()) else {
        return; // git unavailable
    };

    // The fixture's highest main-track number is M-010.
    let first = run_fr_ok(&main, &["add", "main", "from main"]);
    assert_eq!(first.trim(), "M-011");

    // The worktree's copy of tracks/main.md predates that add, so its own scan
    // still tops out at M-010 — it must not reissue M-011.
    let second = run_fr_ok(&worktree, &["add", "main", "from worktree"]);
    assert_eq!(
        second.trim(),
        "M-012",
        "worktree reissued a number the main tree already handed out"
    );

    // Back in the main tree, still monotonic.
    let third = run_fr_ok(&main, &["add", "main", "from main again"]);
    assert_eq!(third.trim(), "M-013");

    // The frontier lives under .git/, shared by both trees and uncommittable.
    assert!(main.join(".git/frame-ids.toml").is_file());
    let (status, _, ok) = run_fr(&main, &["--json", "check"]);
    assert!(ok, "check should pass: {status}");
}

/// The collision the clone-shared frontier deliberately does **not** cover.
///
/// Subtask numbers are allocated per parent by scanning that parent's children,
/// not from the frontier, so two worktrees of one clone adding a subtask to the
/// same parent both mint the same child id. Prevention was weighed and declined
/// (see `src/ops/ids.rs`): a child number means nothing outside its parent, so
/// renumbering one is mechanical — unlike a top-level id, whose reissue `fr
/// check` reports as unrepairable. What has to hold instead is that the
/// collision is detected and repaired.
///
/// This pins the whole path: collide, merge, detect, repair, converge.
#[test]
fn test_colliding_subtask_ids_are_detected_and_repaired() {
    let tmp = tempfile::TempDir::new().unwrap();
    let Some((main, worktree)) = clone_with_worktree(tmp.path()) else {
        return; // git unavailable
    };

    // M-003 already has .1 and .2 in the fixture. Each tree scans only its own
    // copy, sees the same two children, and hands out the same next number.
    let mine = run_fr_ok(&main, &["sub", "M-003", "from main"]);
    let theirs = run_fr_ok(&worktree, &["sub", "M-003", "from worktree"]);
    assert_eq!(
        mine.trim(),
        theirs.trim(),
        "the known open collision; if this ever stops holding, \
         subtask minting grew a frontier and this test is the wrong shape"
    );
    assert_eq!(mine.trim(), "M-003.3");

    // Merge the two working copies: both subtasks survive, carrying one id.
    let merged = fs::read_to_string(main.join("frame/tracks/main.md"))
        .unwrap()
        .replace(
            "  - [ ] `M-003.3` from main\n",
            "  - [ ] `M-003.3` from main\n  - [ ] `M-003.3` from worktree\n",
        );
    fs::write(main.join("frame/tracks/main.md"), &merged).unwrap();
    assert_eq!(merged.matches("`M-003.3`").count(), 2, "merge staged");

    // Detected — the generic duplicate-id error covers subtasks.
    let (out, _, _) = run_fr(&main, &["check"]);
    assert!(
        out.contains("M-003.3 is duplicated"),
        "should report the collision: {out}"
    );

    // Repaired, under the parent — not promoted to a top-level number.
    run_fr_ok(&main, &["clean"]);
    let after = fs::read_to_string(main.join("frame/tracks/main.md")).unwrap();
    assert!(
        after.contains("`M-003.3` from main") && after.contains("`M-003.4` from worktree"),
        "the second copy should take the next child number: {after}"
    );

    // Converged: no duplicate left, and nothing escaped its parent.
    let (recheck, _, _) = run_fr(&main, &["check"]);
    assert!(
        !recheck.contains("duplicated"),
        "duplicate survived: {recheck}"
    );
    assert!(
        !recheck.contains("doesn't extend"),
        "the repair must not misparent anything: {recheck}"
    );
}

/// Archived tasks keep their numbers: `fr clean` moving a done task out of the
/// live track must not free its number for reuse.
#[test]
fn test_archived_ids_are_not_reissued() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());
    fs::create_dir_all(tmp.path().join("frame/archive")).unwrap();
    fs::write(
        tmp.path().join("frame/archive/main.md"),
        "# Archive — main\n\n- [x] `M-050` archived task\n  - resolved: 2025-06-01\n",
    )
    .unwrap();

    let out = run_fr_ok(tmp.path(), &["add", "main", "after archiving"]);
    assert_eq!(out.trim(), "M-051", "mint ignored the archive");
}

/// The same guarantee, with one ordinary markdown bullet in the archive header.
///
/// Every archive reader used to find the task list with
/// `line.starts_with("- [")`, which a link bullet matches, so `- [notes](x.md)`
/// above the tasks read as the first task line — and `parse_tasks` starting on a
/// line that is not a task line stops at once. The archive then held no tasks as
/// far as the mint scan, `fr check` and search were concerned, while the TUI's
/// Recent view, which asked the stricter question, still listed every one of
/// them.
///
/// There is no local frontier store here, which is the state of any fresh clone
/// — `.ids.toml` is working-copy-local and never committed. The archive scan is
/// the only thing that remembers the number was spent, so hiding the archive
/// hands it straight back out, and `fr check` calls the result valid.
#[test]
fn test_a_link_in_the_archive_header_does_not_hide_its_ids() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());
    assert!(
        !tmp.path().join("frame/.ids.toml").exists(),
        "fixture must have no frontier store, or the archive scan is not what is under test"
    );
    fs::create_dir_all(tmp.path().join("frame/archive")).unwrap();
    fs::write(
        tmp.path().join("frame/archive/main.md"),
        "# Archive — main\n- [context](notes.md)\n\n- [x] `M-050` archived task\n  - resolved: 2025-06-01\n",
    )
    .unwrap();

    let out = run_fr_ok(tmp.path(), &["add", "main", "after archiving"]);
    assert_eq!(
        out.trim(),
        "M-051",
        "an archived number was reissued because a link bullet hid the archive"
    );

    // And the link is still a header line, not something a rewrite relocated.
    let archive = fs::read_to_string(tmp.path().join("frame/archive/main.md")).unwrap();
    assert!(archive.contains("- [context](notes.md)"), "{archive}");
}

/// A deleted task's number stays spent — the frontier never walks backwards.
#[test]
fn test_deleted_ids_are_not_reissued() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());

    let first = run_fr_ok(tmp.path(), &["add", "main", "doomed"]);
    assert_eq!(first.trim(), "M-011");
    run_fr_ok(tmp.path(), &["delete", "M-011", "--yes"]);

    let second = run_fr_ok(tmp.path(), &["add", "main", "next"]);
    assert_eq!(second.trim(), "M-012", "deleted number was reissued");
}

/// A reissued number that `fr clean` can't see: one holder live, one archived.
/// Surfaced by `fr check` in human and JSON output.
#[test]
fn test_check_flags_a_live_id_colliding_with_an_archived_one() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());
    fs::create_dir_all(tmp.path().join("frame/archive")).unwrap();
    // M-001 is live in the fixture's main track.
    fs::write(
        tmp.path().join("frame/archive/main.md"),
        "# Archive — main\n\n- [x] `M-001` archived work\n  - resolved: 2025-06-01\n",
    )
    .unwrap();

    let human = run_fr_ok(tmp.path(), &["check"]);
    assert!(
        human.contains("M-001 is live in main but is also archived in archive/main.md"),
        "check should flag the reissue: {human}"
    );
    assert!(human.contains("the number was reissued"), "{human}");

    let json = run_fr_ok(tmp.path(), &["check", "--json"]);
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    let warnings = parsed["warnings"].as_array().unwrap();
    let reissue = warnings
        .iter()
        .find(|w| w["type"] == "id_reissued_after_archive")
        .expect("id_reissued_after_archive warning");
    assert_eq!(reissue["task_id"], "M-001");
    assert_eq!(reissue["tracks"][0], "main");
    assert_eq!(reissue["archives"][0], "archive/main.md");
    // A warning, not an error — there is no automatic repair, and this fires on
    // data that predates the durable frontier.
    assert_eq!(parsed["valid"], true);
}

/// The Lace shape: one archive file holding the same task twice, no live task
/// involved. Reported as duplicated *history* — not as a reissued number, and
/// without naming the same file twice as if two files were involved.
#[test]
fn test_check_flags_a_duplicated_archive_entry() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());
    fs::create_dir_all(tmp.path().join("frame/archive")).unwrap();
    // M-900 is in no live track; the archive holds it twice.
    fs::write(
        tmp.path().join("frame/archive/main.md"),
        "# Archive — main\n\n- [x] `M-900` archived work\n  - resolved: 2025-06-01\n- [x] `M-900` archived work\n  - resolved: 2025-06-01\n",
    )
    .unwrap();

    let human = run_fr_ok(tmp.path(), &["check"]);
    assert!(
        human.contains("M-900 appears 2 times in archive/main.md and in no live track"),
        "check should flag duplicated history: {human}"
    );
    assert!(
        human.contains("no number was reissued"),
        "and must not claim a reissue: {human}"
    );

    let json = run_fr_ok(tmp.path(), &["check", "--json"]);
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    let warnings = parsed["warnings"].as_array().unwrap();
    let duplicate = warnings
        .iter()
        .find(|w| w["type"] == "duplicate_archived_id")
        .expect("duplicate_archived_id warning");
    assert_eq!(duplicate["task_id"], "M-900");
    assert_eq!(duplicate["total"], 2);
    // One file, named once.
    assert_eq!(duplicate["archives"].as_array().unwrap().len(), 1);
    assert!(
        !warnings
            .iter()
            .any(|w| w["type"] == "id_reissued_after_archive"),
        "must not also report a reissue: {json}"
    );
}

/// A frontier store that doesn't parse is reported, not silently reset.
#[test]
fn test_check_flags_an_unreadable_id_frontier() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());
    // Outside git, the store is working-copy-local.
    let store = tmp.path().join("frame/.ids.toml");
    fs::write(&store, "not toml {{{").unwrap();

    let human = run_fr_ok(tmp.path(), &["check"]);
    assert!(
        human.contains("is unreadable"),
        "check should flag the store: {human}"
    );
    assert!(store.is_file(), "check must not reset the store");

    // The next mint does reset it, leaving the .bak that check then reports.
    run_fr_ok(tmp.path(), &["add", "main", "after reset"]);
    let human = run_fr_ok(tmp.path(), &["check"]);
    assert!(
        human.contains("ID frontier was reset"),
        "check should flag the leftover .bak: {human}"
    );
}

// ---------------------------------------------------------------------------
// `fr check --fix`
// ---------------------------------------------------------------------------

/// A task note and an inbox body that each leave a code fence open.
fn project_with_open_fences(root: &Path) {
    create_test_project(root);
    fs::write(
        root.join("frame/tracks/main.md"),
        "\
# Main Track

## Backlog

- [ ] `M-001` Task with an open fence
  - added: 2026-07-31
  - note:
    Example:
    ```rust
    let x = 1;

## Done
",
    )
    .unwrap();
    fs::write(
        root.join("frame/inbox.md"),
        "# Inbox\n\n- Item with an open body fence\n  ```lace\n  perform Ask()\n",
    )
    .unwrap();
}

#[test]
fn test_check_stays_read_only_without_fix() {
    let tmp = tempfile::TempDir::new().unwrap();
    project_with_open_fences(tmp.path());

    let before = fs::read_to_string(tmp.path().join("frame/tracks/main.md")).unwrap();
    let out = run_fr_ok(tmp.path(), &["check"]);
    let after = fs::read_to_string(tmp.path().join("frame/tracks/main.md")).unwrap();

    assert!(out.contains("code fence open"), "should report it: {out}");
    assert_eq!(before, after, "bare `fr check` must not write");
}

#[test]
fn test_check_fix_dry_run_writes_nothing() {
    let tmp = tempfile::TempDir::new().unwrap();
    project_with_open_fences(tmp.path());

    let before = fs::read_to_string(tmp.path().join("frame/tracks/main.md")).unwrap();
    let out = run_fr_ok(tmp.path(), &["check", "--fix", "--dry-run"]);
    let after = fs::read_to_string(tmp.path().join("frame/tracks/main.md")).unwrap();

    assert!(out.contains("close note fence"), "should plan it: {out}");
    assert!(out.contains("dry run"), "should say so: {out}");
    assert_eq!(before, after, "--dry-run must not write");
}

#[test]
fn test_check_fix_closes_note_and_inbox_fences() {
    let tmp = tempfile::TempDir::new().unwrap();
    project_with_open_fences(tmp.path());

    run_fr_ok(tmp.path(), &["check", "--fix"]);

    let track = fs::read_to_string(tmp.path().join("frame/tracks/main.md")).unwrap();
    let inbox = fs::read_to_string(tmp.path().join("frame/inbox.md")).unwrap();
    assert!(track.contains("let x = 1;"), "content preserved: {track}");
    assert!(
        track.matches("```").count() >= 2,
        "note fence closed: {track}"
    );
    assert!(
        inbox.matches("```").count() >= 2,
        "inbox fence closed: {inbox}"
    );

    // The findings are gone, and a second run has nothing to do.
    let recheck = run_fr_ok(tmp.path(), &["check"]);
    assert!(
        !recheck.contains("code fence open"),
        "fences should be balanced now: {recheck}"
    );
    let again = run_fr_ok(tmp.path(), &["check", "--fix"]);
    assert!(
        again.contains("nothing to repair"),
        "--fix must be idempotent: {again}"
    );
}

/// An archive holding one task twice, in the shape **`fr clean` actually
/// writes**: a `# Archive — <track>` title and then bare task lines, with no
/// `## Section` header.
///
/// The shape is the point. Both `--fix` tests below used to invent a `## Done`
/// section, which exercised a path no real archive takes and let the dedupe
/// repair ship not working at all — it walked `TrackNode::Section`, found
/// nothing, and reported the task "no longer appears in the archives".
/// `tests/damaged_corpus.rs` caught it by building the archive the way clean
/// does.
const DUPLICATED_ARCHIVE: &str = "\
# Archive — main

- [x] `M-900` Archived twice
  - resolved: 2026-01-01
- [x] `M-900` Archived twice
  - resolved: 2026-01-01
- [x] `M-901` Archived once
  - resolved: 2026-01-02

<!-- kept by hand -->
";

/// A repair that deletes must not run without consent. With stdin closed the
/// prompt reads nothing, which is not `y`, so the run cancels — the behaviour a
/// non-interactive caller (CI, an agent) gets by default.
#[test]
fn test_check_fix_cancels_deleting_repair_without_yes() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());
    fs::create_dir_all(tmp.path().join("frame/archive")).unwrap();
    let archive = DUPLICATED_ARCHIVE;
    fs::write(tmp.path().join("frame/archive/main.md"), archive).unwrap();

    let (stdout, stderr, ok) = run_fr(tmp.path(), &["check", "--fix"]);
    assert!(ok, "should exit cleanly: {stderr}");
    assert!(
        stdout.contains("duplicate archive") || stdout.contains("delete"),
        "should describe the deleting repair: {stdout}"
    );
    assert!(stderr.contains("cancelled"), "should cancel: {stderr}");

    let after = fs::read_to_string(tmp.path().join("frame/archive/main.md")).unwrap();
    assert_eq!(after, archive, "archive must be untouched after cancelling");
}

#[test]
fn test_check_fix_yes_dedupes_archive_and_logs_recovery() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());
    fs::create_dir_all(tmp.path().join("frame/archive")).unwrap();
    fs::write(tmp.path().join("frame/archive/main.md"), DUPLICATED_ARCHIVE).unwrap();

    run_fr_ok(tmp.path(), &["check", "--fix", "--yes"]);

    let after = fs::read_to_string(tmp.path().join("frame/archive/main.md")).unwrap();
    assert_eq!(
        after.matches("`M-900`").count(),
        1,
        "one copy should remain: {after}"
    );
    assert!(after.contains("`M-901`"), "other tasks untouched: {after}");
    assert!(
        after.starts_with("# Archive — main"),
        "the archive header is carried verbatim: {after}"
    );
    // And so is anything below the last task. The rewrite was header-plus-tasks,
    // built from the index the reader started at, and discarded the index the
    // parser stopped at — so a note at the bottom of an archive was in neither
    // piece and this repair deleted it.
    assert!(
        after.contains("<!-- kept by hand -->"),
        "content below the last task was dropped: {after}"
    );

    // What was removed is recoverable, not gone.
    let log = run_fr_ok(tmp.path(), &["recovery"]);
    assert!(
        log.contains("M-900"),
        "removed copy should be in the recovery log: {log}"
    );
}

/// The stale-prefix repair renames archived ids onto the track's current prefix
/// — unless one of them would land on an id that already exists.
///
/// A partial rename is the bad outcome here: the archive would hold two prefixes
/// with nothing recording which tasks moved, and the finding that explains it
/// would be half-cleared. So one collision refuses the whole file and the
/// warning stays.
#[test]
fn test_check_fix_refuses_a_colliding_archived_prefix_rename() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());
    fs::create_dir_all(tmp.path().join("frame/archive")).unwrap();
    // M-001 is live in the fixture, so OLD-001 has nowhere to go.
    let archive = "# Archive — main\n\n- [x] `OLD-001` archived under a dead prefix\n  - resolved: 2025-12-01\n";
    fs::write(tmp.path().join("frame/archive/main.md"), archive).unwrap();

    let out = run_fr_ok(tmp.path(), &["check", "--fix", "--yes"]);
    assert!(
        out.contains("skipped") && out.contains("already exists"),
        "the repair should refuse and say why: {out}"
    );
    assert_eq!(
        fs::read_to_string(tmp.path().join("frame/archive/main.md")).unwrap(),
        archive,
        "a refused repair must not half-rename the file"
    );
    assert!(
        run_fr_ok(tmp.path(), &["check"]).contains("still use the prefix OLD-"),
        "and the warning stays"
    );
}

/// The damage `fr clean` used to leave behind: a duplicated *subtask* id
/// resolved with a top-level number, so the task ends up as `M-020` nested under
/// `M-003`. Projects cleaned before that was fixed still hold it on disk, which
/// is what this repair is for.
#[test]
fn test_check_fix_renumbers_a_subtask_that_escaped_its_parent() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());
    fs::write(
        tmp.path().join("frame/tracks/main.md"),
        "\
# Main Track

## Backlog

- [ ] `M-003` Parent
  - added: 2025-05-03
  - [ ] `M-003.1` Sub one
    - added: 2025-05-03
  - [ ] `M-020` Escaped, with a child of its own
    - added: 2025-05-03
    - [ ] `M-020.1` Deep
      - added: 2025-05-03
- [ ] `M-004` Waiting on the escapee
  - added: 2025-05-03
  - dep: M-020

## Done
",
    )
    .unwrap();

    let reported = run_fr_ok(tmp.path(), &["check"]);
    assert!(
        reported.contains("M-020 is nested under M-003 but its id doesn't extend it"),
        "should report it: {reported}"
    );

    // It rewrites an id out of existence, so it needs consent.
    let planned = run_fr_ok(tmp.path(), &["check", "--fix", "--dry-run"]);
    assert!(
        planned.contains("renumber under its parent M-003"),
        "should plan it: {planned}"
    );

    run_fr_ok(tmp.path(), &["check", "--fix", "--yes"]);

    let track = fs::read_to_string(tmp.path().join("frame/tracks/main.md")).unwrap();
    assert!(
        track.contains("`M-003.2` Escaped"),
        "should take the next free child number: {track}"
    );
    assert!(
        track.contains("`M-003.2.1` Deep"),
        "descendants follow: {track}"
    );
    assert!(
        track.contains("dep: M-003.2"),
        "deps follow the rekey: {track}"
    );

    let recheck = run_fr_ok(tmp.path(), &["check"]);
    assert!(
        !recheck.contains("doesn't extend"),
        "finding should be gone: {recheck}"
    );
    let again = run_fr_ok(tmp.path(), &["check", "--fix"]);
    assert!(
        again.contains("nothing to repair"),
        "--fix must be idempotent: {again}"
    );
}

/// `fr git setup` adds the blanket pattern, once, however many local files were
/// reported — it covers all of them and the next one added to `frame/` too,
/// which enumerating never could.
///
/// `fr check --fix` deliberately does **not**: git readiness is one surface with
/// one owner, so a user can predict what `--fix` touches. Both halves are
/// asserted here, because the split is the point.
#[test]
fn test_git_setup_adds_the_gitignore_pattern() {
    let tmp = tempfile::TempDir::new().unwrap();
    if !std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(tmp.path())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
    {
        return; // no git available
    }
    create_test_project(tmp.path());
    fs::write(tmp.path().join(".gitignore"), "target/\n").unwrap();

    // --fix reports the leak but repairs nothing about it.
    run_fr_ok(tmp.path(), &["check", "--fix"]);
    let untouched = fs::read_to_string(tmp.path().join(".gitignore")).unwrap();
    assert!(
        !untouched.contains("frame/"),
        "--fix must leave git readiness to `fr git setup`: {untouched}"
    );

    run_fr_ok(tmp.path(), &["git", "setup"]);

    let gitignore = fs::read_to_string(tmp.path().join(".gitignore")).unwrap();
    assert!(
        gitignore.contains("frame/.*"),
        "the pattern should be added: {gitignore}"
    );
    assert_eq!(
        gitignore.lines().filter(|l| l.trim() == "frame/.*").count(),
        1,
        "exactly once, however many files were reported: {gitignore}"
    );
    // And it really does cover them.
    for name in frame::io::project_io::LOCAL_ONLY_FRAME_FILES {
        let ok = std::process::Command::new("git")
            .args(["check-ignore", "-q", &format!("frame/{name}")])
            .current_dir(tmp.path())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        assert!(ok, "frame/{name} should be ignored by the pattern");
    }
}

// ---------------------------------------------------------------------------
// `fr git setup` where the project is not at the repo root
//
// Both files it writes contain patterns, and a git pattern with a slash in it is
// relative to the directory of the file holding it — never to the repository
// root. Setup wrote the frame directory's path relative to the *git toplevel*
// instead, so a project in `sub/` got `sub/frame/archive/*.md` written into
// `sub/.gitattributes`, where it means `sub/sub/frame/...`. Nothing routed to
// the merge driver and nothing was ignored, in a file that looked right.
//
// So these assert on what git resolves, never on what the file says.
// ---------------------------------------------------------------------------

/// The `merge` attribute git resolves for `path`, asked from `dir`.
fn merge_attr(dir: &Path, path: &str) -> String {
    let out = std::process::Command::new("git")
        .args(["check-attr", "merge", "--", path])
        .current_dir(dir)
        .output()
        .expect("git check-attr runs");
    // `<path>: merge: <value>`
    String::from_utf8_lossy(&out.stdout)
        .rsplit(':')
        .next()
        .unwrap_or_default()
        .trim()
        .to_string()
}

/// Assert that every routed shape reaches the driver, and that the blanket
/// ignore pattern really covers a local file — both asked of git.
fn assert_git_ready(project_root: &Path, label: &str) {
    for path in frame::ops::git_setup::routed_paths() {
        assert_eq!(
            merge_attr(project_root, &path),
            "frame",
            "{label}: {path} must route to the frame merge driver"
        );
    }
    assert!(
        git_ok(project_root, &["check-ignore", "-q", "frame/.actor"]),
        "{label}: frame/.actor must be ignored"
    );
}

/// A project at the repo root, and one below it, must both end up genuinely
/// configured — and a second `fr git setup` must change nothing.
#[test]
fn git_setup_routes_and_ignores_at_the_root_and_below_it() {
    for sub in ["", "sub"] {
        let tmp = tempfile::TempDir::new().unwrap();
        if !git_ok(tmp.path(), &["init", "-q"]) {
            return; // no git available
        }
        let root = if sub.is_empty() {
            tmp.path().to_path_buf()
        } else {
            let p = tmp.path().join(sub);
            fs::create_dir_all(&p).unwrap();
            p
        };
        create_test_project(&root);
        let label = if sub.is_empty() {
            "at root"
        } else {
            "below root"
        };

        let first = run_fr_ok(&root, &["git", "setup"]);
        assert!(first.contains("configured"), "{label}: {first}");
        assert_git_ready(&root, label);

        // Idempotent: the second run reports nothing to do and rewrites nothing.
        let before_attrs = fs::read_to_string(root.join(".gitattributes")).unwrap();
        let before_ignore = fs::read_to_string(root.join(".gitignore")).unwrap();
        let second = run_fr_ok(&root, &["git", "setup"]);
        assert_eq!(
            fs::read_to_string(root.join(".gitattributes")).unwrap(),
            before_attrs,
            "{label}: second `fr git setup` rewrote .gitattributes:\n{second}"
        );
        assert_eq!(
            fs::read_to_string(root.join(".gitignore")).unwrap(),
            before_ignore,
            "{label}: second `fr git setup` rewrote .gitignore:\n{second}"
        );
        assert_git_ready(&root, label);

        // And `fr check` is satisfied — the routing warning is the real test of
        // the same thing, so the two must agree.
        let checked = run_fr_ok(&root, &["check"]);
        assert!(
            !checked.contains("gitattributes"),
            "{label}: check should report no routing problem: {checked}"
        );
    }
}

/// A project already carrying the dead prefixed lines an older `fr git setup`
/// wrote: setup replaces them with working ones and takes the dead ones out.
#[test]
fn git_setup_cleans_up_the_dead_lines_it_used_to_write() {
    let tmp = tempfile::TempDir::new().unwrap();
    if !git_ok(tmp.path(), &["init", "-q"]) {
        return;
    }
    let root = tmp.path().join("sub");
    fs::create_dir_all(&root).unwrap();
    create_test_project(&root);

    // Exactly what the old computation produced, in the file it produced it in.
    fs::write(
        root.join(".gitattributes"),
        "# frame — merge track and inbox files by task identity\n\
         sub/frame/tracks/*.md merge=frame\n\
         sub/frame/archive/*.md merge=frame\n\
         sub/frame/archive/_tracks/*.md merge=frame\n\
         sub/frame/inbox.md merge=frame\n\
         *.png binary\n",
    )
    .unwrap();
    fs::write(root.join(".gitignore"), "target/\nsub/frame/.*\n").unwrap();

    // Before: the patterns are all present, and none of them do anything.
    let broken = run_fr_ok(&root, &["check"]);
    assert!(
        broken.contains("gitattributes"),
        "check must notice that present patterns route nothing: {broken}"
    );

    run_fr_ok(&root, &["git", "setup"]);
    assert_git_ready(&root, "after cleanup");

    let attrs = fs::read_to_string(root.join(".gitattributes")).unwrap();
    assert!(
        !attrs.contains("sub/frame/"),
        "the dead lines should be gone: {attrs}"
    );
    assert!(
        attrs.contains("*.png binary"),
        "an unrelated line is not ours to remove: {attrs}"
    );
    let ignore = fs::read_to_string(root.join(".gitignore")).unwrap();
    assert!(!ignore.contains("sub/frame/.*"), "{ignore}");
    assert!(ignore.contains("frame/.*"), "{ignore}");
    assert!(ignore.contains("target/"), "unrelated line kept: {ignore}");

    // Still idempotent afterwards.
    let before = fs::read_to_string(root.join(".gitattributes")).unwrap();
    run_fr_ok(&root, &["git", "setup"]);
    assert_eq!(
        fs::read_to_string(root.join(".gitattributes")).unwrap(),
        before
    );
}

// ---------------------------------------------------------------------------
// Crash injection: multi-file write sequences
//
// Single-file writes are atomic (temp file + rename), so the exposure is
// operations that are only complete after two or more files are written. These
// use `FRAME_FAIL_WRITE` (see `src/io/fault.rs`) to cut one specific write of a
// real sequence and then assert on **files on disk** — not on the recovery log,
// which only catches a write that returns an error and would be skipped
// entirely by an abrupt death.
// ---------------------------------------------------------------------------

/// Two tracks, one task in the first, ready to be moved.
fn two_track_project(root: &Path) {
    run_fr_ok(
        root,
        &[
            "init", "--name", "p", "--track", "a", "A", "--track", "b", "B",
        ],
    );
    run_fr_ok(root, &["add", "a", "the task to move"]);
}

/// Which track files still mention the task.
fn tracks_holding(root: &Path, needle: &str) -> Vec<String> {
    let mut out = Vec::new();
    let dir = root.join("frame/tracks");
    let mut entries: Vec<_> = fs::read_dir(&dir)
        .expect("tracks dir")
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .collect();
    entries.sort();
    for path in entries {
        if fs::read_to_string(&path)
            .map(|c| c.contains(needle))
            .unwrap_or(false)
        {
            out.push(path.file_name().unwrap().to_string_lossy().into_owned());
        }
    }
    out
}

/// A cross-track move writes the target before the source, so an interruption
/// between the two cannot leave the task in neither track.
///
/// Failing the *target* write is the case that used to lose data: the source had
/// already been saved with the task removed, and a task in no track is
/// indistinguishable from one that never existed, so nothing could detect or
/// repair it.
#[test]
fn test_cross_track_move_survives_target_write_failure() {
    let tmp = tempfile::TempDir::new().unwrap();
    two_track_project(tmp.path());

    let (_, _, ok) = run_fr_env(
        tmp.path(),
        &["mv", "A-001", "--track", "b"],
        &[("FRAME_FAIL_WRITE", "tracks/b.md")],
    );
    assert!(!ok, "the injected failure should fail the command");

    assert_eq!(
        tracks_holding(tmp.path(), "the task to move"),
        vec!["a.md"],
        "task must remain in the source track when the target write is cut"
    );
}

/// The other window: the target landed, the source write is cut. The task is now
/// in both tracks — under different ids, since the move re-mints into the mover's
/// namespace. That is a duplicate for a human to reconcile, which is the price of
/// never losing it.
#[test]
fn test_cross_track_move_survives_source_write_failure() {
    let tmp = tempfile::TempDir::new().unwrap();
    two_track_project(tmp.path());

    let (_, _, ok) = run_fr_env(
        tmp.path(),
        &["mv", "A-001", "--track", "b"],
        &[("FRAME_FAIL_WRITE", "tracks/a.md")],
    );
    assert!(!ok, "the injected failure should fail the command");

    let holding = tracks_holding(tmp.path(), "the task to move");
    assert!(
        holding.contains(&"b.md".to_string()),
        "target should hold the moved task: {holding:?}"
    );
    assert!(
        !holding.is_empty(),
        "the task must survive somewhere, whichever write is cut"
    );
}

/// Archiving a track is two steps — mark it archived in config, then move the
/// file into `archive/_tracks/`. Interrupted between them the config says
/// archived while the file is still in `tracks/`; re-running must finish the job
/// rather than wedge.
#[test]
fn test_track_archive_recovers_from_interrupted_file_move() {
    let tmp = tempfile::TempDir::new().unwrap();
    two_track_project(tmp.path());

    let (_, _, ok) = run_fr_env(
        tmp.path(),
        &["track", "archive", "a"],
        &[("FRAME_FAIL_WRITE", "tracks/a.md")],
    );
    assert!(!ok, "the injected failure should fail the command");

    // The half-applied state: config updated, file not yet moved.
    let config = fs::read_to_string(tmp.path().join("frame/project.toml")).unwrap();
    assert!(config.contains("archived"), "config was written first");
    assert!(
        tmp.path().join("frame/tracks/a.md").exists(),
        "the file move is what was cut"
    );

    // Any following write command recovers it — no need to re-run the archive.
    run_fr_ok(tmp.path(), &["add", "b", "unrelated"]);
    assert!(
        !tmp.path().join("frame/tracks/a.md").exists(),
        "recovery should move the file out of tracks/"
    );
    assert!(
        tmp.path().join("frame/archive/_tracks/a.md").exists(),
        "and into archive/_tracks/"
    );
}

/// `fr check --fix` applies a plan of independent repairs. Cutting one must not
/// prevent a re-run from applying the rest, and must not apply anything twice.
#[test]
fn test_check_fix_converges_after_a_partial_application() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());
    // Two tracks with an unclosed fence each, so the plan has two writes.
    for (file, id) in [("main.md", "M-500"), ("side.md", "S-500")] {
        fs::write(
            tmp.path().join("frame/tracks").join(file),
            format!(
                "# T\n\n## Backlog\n\n- [ ] `{id}` Open fence\n  - note:\n    ```rust\n    let x = 1;\n\n## Done\n"
            ),
        )
        .unwrap();
    }

    let (_, _, ok) = run_fr_env(
        tmp.path(),
        &["check", "--fix"],
        &[("FRAME_FAIL_WRITE", "tracks/side.md")],
    );
    assert!(!ok, "the injected failure should fail the command");

    // Re-run without injection: whatever is left gets repaired.
    run_fr_ok(tmp.path(), &["check", "--fix"]);

    let recheck = run_fr_ok(tmp.path(), &["check"]);
    assert!(
        !recheck.contains("code fence open"),
        "both fences should be closed after the re-run: {recheck}"
    );

    // Nothing was applied twice: each note carries exactly one closing fence, so
    // the opener plus the closer is two markers, not three.
    for file in ["main.md", "side.md"] {
        let content = fs::read_to_string(tmp.path().join("frame/tracks").join(file)).unwrap();
        assert_eq!(
            content.matches("```").count(),
            2,
            "{file} should have one opener and one closer:\n{content}"
        );
    }
}

/// Fault injection must be inert unless asked for — otherwise every other test
/// in the suite would be running against a rigged write path.
#[test]
fn test_fault_injection_is_off_by_default() {
    let tmp = tempfile::TempDir::new().unwrap();
    two_track_project(tmp.path());

    run_fr_ok(tmp.path(), &["mv", "A-001", "--track", "b"]);
    assert_eq!(
        tracks_holding(tmp.path(), "the task to move"),
        vec!["b.md"],
        "an uninjected move should complete normally"
    );
}

/// `fr actor merge` renumbers ids across tracks and archives, then retires the
/// source tokens in `actors.toml`. Cutting the registry write leaves the ids
/// remapped while the source token is still active — the state where a naive
/// implementation would wedge, because there are no source-namespace ids left to
/// find on a retry. Re-running must still finish the retirement.
#[test]
fn test_actor_merge_converges_when_the_registry_write_is_cut() {
    let tmp = tempfile::TempDir::new().unwrap();
    run_fr_ok(tmp.path(), &["init", "--name", "p", "--track", "a", "A"]);

    run_fr_ok(tmp.path(), &["actor", "set", "x"]);
    run_fr_ok(tmp.path(), &["add", "a", "from x"]);
    run_fr_ok(tmp.path(), &["actor", "set", "y"]);
    run_fr_ok(tmp.path(), &["add", "a", "from y"]);

    let (_, _, ok) = run_fr_env(
        tmp.path(),
        &["actor", "merge", "x", "--into", "y"],
        &[("FRAME_FAIL_WRITE", "actors.toml")],
    );
    assert!(!ok, "the injected failure should fail the command");

    // Ids were remapped before the registry write was attempted.
    let track = fs::read_to_string(tmp.path().join("frame/tracks/a.md")).unwrap();
    assert!(
        !track.contains("`A-x"),
        "no id should remain in the merged-away namespace: {track}"
    );
    // But the token is still active.
    let listing = run_fr_ok(tmp.path(), &["actor", "list"]);
    assert!(
        listing
            .lines()
            .any(|l| l.contains(" x ") && l.contains("active")),
        "x should still be active after the cut: {listing}"
    );

    // Re-running completes the retirement rather than wedging.
    run_fr_ok(tmp.path(), &["actor", "merge", "x", "--into", "y"]);
    let listing = run_fr_ok(tmp.path(), &["actor", "list"]);
    assert!(
        listing
            .lines()
            .any(|l| l.contains(" x ") && l.contains("retired")),
        "x should be retired after the re-run: {listing}"
    );

    assert!(
        run_fr_ok(tmp.path(), &["check"]).contains("valid"),
        "project should be consistent after recovery"
    );
}

/// Two different shapes live under `frame/archive/`, and `fr actor merge` read
/// both as the bare task list only one of them is. An archived *whole track*
/// still has its `## Section` headers, so the task-list reader stopped at the
/// first header below the tasks and the rewrite dropped everything under it — a
/// `## Done` section with completed tasks in it, deleted by a command that only
/// claimed to renumber ids. The ids down there were missing from the census the
/// remap's collision-freedom rests on, too, so they stayed in a namespace the
/// same command then retired.
#[test]
fn test_actor_merge_keeps_every_section_of_an_archived_track() {
    let tmp = tempfile::TempDir::new().unwrap();
    run_fr_ok(tmp.path(), &["init", "--name", "p", "--track", "a", "A"]);
    run_fr_ok(tmp.path(), &["track", "new", "side", "Side"]);

    run_fr_ok(tmp.path(), &["actor", "set", "x"]);
    let open = run_fr_ok(tmp.path(), &["add", "side", "still open"])
        .trim()
        .to_string();
    let done = run_fr_ok(tmp.path(), &["add", "side", "already finished"])
        .trim()
        .to_string();
    run_fr_ok(tmp.path(), &["state", &done, "done"]);
    run_fr_ok(tmp.path(), &["track", "archive", "side"]);

    let archived = tmp.path().join("frame/archive/_tracks/side.md");
    let before = fs::read_to_string(&archived).unwrap();
    assert!(
        before.contains("## Done") && before.contains("already finished"),
        "fixture should have a populated Done section below the Backlog: {before}"
    );

    run_fr_ok(tmp.path(), &["actor", "set", "y"]);
    run_fr_ok(tmp.path(), &["actor", "merge", "x", "--into", "y"]);

    let after = fs::read_to_string(&archived).unwrap();
    assert!(
        after.contains("## Done") && after.contains("already finished"),
        "the Done section and its tasks must survive a merge: {after}"
    );
    assert!(
        after.contains("## Parked"),
        "so must the empty section between them: {after}"
    );
    // And both ids moved — the one below the section header was invisible to the
    // id census, so it used to survive in a namespace that had just been retired.
    assert!(
        !after.contains(&open) && !after.contains(&done),
        "every id should have left the merged-away namespace: {after}"
    );
}

// ---------------------------------------------------------------------------
// In-flight marker and automatic recovery
// ---------------------------------------------------------------------------

/// The state after a cut cross-track move is undetectable by any check — the
/// task is in both tracks under different IDs, which is a legitimate shape. The
/// marker is what makes it knowable, and the next write command completes the
/// move rather than leaving a human to work out which copy to delete.
#[test]
fn test_interrupted_cross_track_move_is_recovered_by_the_next_write() {
    let tmp = tempfile::TempDir::new().unwrap();
    two_track_project(tmp.path());

    let (_, _, ok) = run_fr_env(
        tmp.path(),
        &["mv", "A-001", "--track", "b"],
        &[("FRAME_FAIL_WRITE", "tracks/a.md")],
    );
    assert!(!ok);

    // The interrupted state: in both tracks, and a marker recording the intent.
    assert_eq!(
        tracks_holding(tmp.path(), "the task to move"),
        vec!["a.md", "b.md"],
    );
    assert!(
        tmp.path().join("frame/.inflight").exists(),
        "marker written"
    );

    // Check reports it, where it would otherwise say the project is valid.
    let checked = run_fr_ok(tmp.path(), &["check"]);
    assert!(
        checked.contains("did not finish"),
        "check should report the interrupted operation: {checked}"
    );

    // Any write command completes the move.
    let (_, stderr, _) = run_fr(tmp.path(), &["add", "a", "something else"]);
    assert!(
        stderr.contains("recovered an interrupted"),
        "recovery should be announced: {stderr}"
    );

    assert_eq!(
        tracks_holding(tmp.path(), "the task to move"),
        vec!["b.md"],
        "the move should now be complete — target only"
    );
    assert!(
        !tmp.path().join("frame/.inflight").exists(),
        "marker cleared after recovery"
    );
    assert!(run_fr_ok(tmp.path(), &["check"]).contains("valid"));
}

/// Triage writes the task, then the inbox. Cut in between, the item is both a
/// task and still in the inbox; recovery drops the inbox copy.
#[test]
fn test_interrupted_triage_is_recovered() {
    let tmp = tempfile::TempDir::new().unwrap();
    two_track_project(tmp.path());
    run_fr_ok(tmp.path(), &["inbox", "an idea worth keeping"]);

    let (_, _, ok) = run_fr_env(
        tmp.path(),
        &["triage", "1", "--track", "b"],
        &[("FRAME_FAIL_WRITE", "inbox.md")],
    );
    assert!(!ok);

    let inbox = fs::read_to_string(tmp.path().join("frame/inbox.md")).unwrap();
    assert!(
        inbox.contains("an idea worth keeping"),
        "still in the inbox"
    );

    run_fr_ok(tmp.path(), &["add", "a", "something else"]);

    let inbox = fs::read_to_string(tmp.path().join("frame/inbox.md")).unwrap();
    assert!(
        !inbox.contains("an idea worth keeping"),
        "recovery should remove the inbox copy: {inbox}"
    );
    assert!(
        tracks_holding(tmp.path(), "an idea worth keeping") == vec!["b.md"],
        "and the task should remain"
    );
}

/// `fr actor merge` renumbers before it writes the registry. Cut in between, the
/// source token is still active with no source-namespace ids left to find — the
/// case a naive retry cannot work out. The marker records which tokens to retire.
#[test]
fn test_interrupted_actor_merge_is_recovered() {
    let tmp = tempfile::TempDir::new().unwrap();
    run_fr_ok(tmp.path(), &["init", "--name", "p", "--track", "a", "A"]);
    run_fr_ok(tmp.path(), &["actor", "set", "x"]);
    run_fr_ok(tmp.path(), &["add", "a", "from x"]);
    run_fr_ok(tmp.path(), &["actor", "set", "y"]);
    run_fr_ok(tmp.path(), &["add", "a", "from y"]);

    let (_, _, ok) = run_fr_env(
        tmp.path(),
        &["actor", "merge", "x", "--into", "y"],
        &[("FRAME_FAIL_WRITE", "actors.toml")],
    );
    assert!(!ok);

    run_fr_ok(tmp.path(), &["add", "a", "something else"]);

    let listing = run_fr_ok(tmp.path(), &["actor", "list"]);
    assert!(
        listing
            .lines()
            .any(|l| l.contains(" x ") && l.contains("retired")),
        "recovery should retire the merged-away token: {listing}"
    );
}

/// A completed operation leaves nothing behind — otherwise every command would
/// be doing recovery work and the warning would be permanent noise.
#[test]
fn test_a_completed_operation_leaves_no_marker() {
    let tmp = tempfile::TempDir::new().unwrap();
    two_track_project(tmp.path());

    run_fr_ok(tmp.path(), &["mv", "A-001", "--track", "b"]);
    assert!(
        !tmp.path().join("frame/.inflight").exists(),
        "marker should be cleared on success"
    );
    assert!(run_fr_ok(tmp.path(), &["check"]).contains("valid"));
}

/// When a precondition no longer holds, recovery must not guess. It leaves
/// everything alone and keeps the marker, so the warning stands until a human
/// acknowledges it with `fr check --fix --yes`.
#[test]
fn test_recovery_declines_when_a_precondition_fails() {
    let tmp = tempfile::TempDir::new().unwrap();
    two_track_project(tmp.path());
    run_fr_ok(tmp.path(), &["inbox", "an orphan idea"]);

    // A marker claiming a triage completed, for a task that does not exist.
    // Reachable when the world changed between the crash and the recovery.
    fs::write(
        tmp.path().join("frame/.inflight"),
        "command = \"fr triage 1 --track b\"\n\
         started = \"2026-07-31T00:00:00Z\"\n\
         kind = \"triage\"\n\
         index = 1\n\
         title = \"an orphan idea\"\n\
         track_id = \"b\"\n",
    )
    .unwrap();

    let (_, stderr, _) = run_fr(tmp.path(), &["add", "a", "something else"]);
    assert!(
        stderr.contains("could not be completed automatically"),
        "should decline and say so: {stderr}"
    );

    let inbox = fs::read_to_string(tmp.path().join("frame/inbox.md")).unwrap();
    assert!(
        inbox.contains("an orphan idea"),
        "the inbox item must not be dropped when the task never landed: {inbox}"
    );
    assert!(
        tmp.path().join("frame/.inflight").exists(),
        "marker kept so the warning stands"
    );

    // The escape hatch: acknowledge it.
    run_fr_ok(tmp.path(), &["check", "--fix", "--yes"]);
    assert!(
        !tmp.path().join("frame/.inflight").exists(),
        "--fix --yes should clear a marker recovery declined to act on"
    );
}

/// The in-flight marker exists only between a crash and the next write command,
/// so an existence check almost never catches it — and `fr check --fix` never
/// can, because it recovers first, which removes the marker before the repair
/// plan is computed. Without an exception a project created before the marker
/// existed could never acquire its `.gitignore` line, and a `git add -A` in that
/// window would commit it.
#[test]
fn test_inflight_gitignore_entry_is_reported_even_when_absent() {
    let tmp = tempfile::TempDir::new().unwrap();
    if !std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(tmp.path())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
    {
        return; // no git available
    }
    create_test_project(tmp.path());

    // A .gitignore as a project predating the marker would have it: everything
    // else covered, `.inflight` missing.
    fs::write(
        tmp.path().join(".gitignore"),
        "frame/.state.json\nframe/.lock\nframe/.recovery.log\nframe/.actor\n\
         frame/.ids.toml\nframe/.ids.lock\n",
    )
    .unwrap();
    assert!(
        !tmp.path().join("frame/.inflight").exists(),
        "no operation is in flight — that is the point"
    );

    let checked = run_fr_ok(tmp.path(), &["check"]);
    assert!(
        checked.contains("frame/.inflight"),
        "should be reported even though the file is absent: {checked}"
    );

    run_fr_ok(tmp.path(), &["git", "setup"]);
    let gitignore = fs::read_to_string(tmp.path().join(".gitignore")).unwrap();
    assert!(
        gitignore.contains("frame/.*"),
        "`fr git setup` should add the pattern, which covers it: {gitignore}"
    );

    // The persistent files keep the existence gate: `.ids.toml` never appears
    // inside a git repo, so warning about it would be noise.
    fs::write(tmp.path().join(".gitignore"), "frame/.state.json\n").unwrap();
    let checked = run_fr_ok(tmp.path(), &["check"]);
    assert!(
        !checked.contains("frame/.ids.toml"),
        "an absent persistent file should stay unreported: {checked}"
    );
}

// ---------------------------------------------------------------------------
// The merge driver, through real git
//
// The unit tests in `ops::merge_files` pin the merge algorithm. These pin the
// part that only a real repository can show: that `.gitattributes` plus the
// registered driver actually route a merge through frame, that the exit status
// stops the operation when it should, and that git's own view of the path
// agrees with the file frame left behind.
// ---------------------------------------------------------------------------

/// A git repo with a frame project, the driver registered, and one commit.
///
/// Returns `false` when git is unavailable, so the caller can skip.
fn merge_repo(root: &Path) -> bool {
    if !git_ok(root, &["init", "-q"]) {
        return false;
    }
    git_ok(root, &["config", "user.email", "test@example.com"]);
    git_ok(root, &["config", "user.name", "Test"]);
    create_test_project(root);

    // The driver is registered against the *test* binary. In real use it is a
    // bare `fr`, which `fr git setup` writes; here PATH cannot be relied on.
    let driver = format!(
        "{} merge --base %O --ours %A --theirs %B --path %P",
        fr_bin().display()
    );
    git_ok(root, &["config", "merge.frame.driver", &driver]);
    git_ok(root, &["config", "merge.frame.recursive", "binary"]);
    fs::write(
        root.join(".gitattributes"),
        "frame/tracks/*.md merge=frame\nframe/inbox.md merge=frame\n",
    )
    .unwrap();
    // The harness points XDG_CONFIG_HOME inside the working directory, so the
    // sandbox registry would otherwise be committed and conflict on every merge.
    fs::write(root.join(".gitignore"), "frame/.*\n.xdg-config/\n").unwrap();

    if !(git_ok(root, &["add", "-A"]) && git_ok(root, &["commit", "-qm", "base"])) {
        return false;
    }
    // `init.defaultBranch` is not `main` everywhere — CI runs git's own default,
    // which is `master`. Name the branch here so a later `checkout main` cannot
    // fail silently and leave the test merging a branch into itself.
    git_must(root, &["branch", "-M", "main"]);
    true
}

fn git_ok(dir: &Path, args: &[&str]) -> bool {
    Command::new("git")
        .current_dir(dir)
        .args(args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// A git step whose failure would make the test that follows meaningless.
fn git_must(dir: &Path, args: &[&str]) {
    assert!(git_ok(dir, args), "git {args:?} failed");
}

fn git_out(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .current_dir(dir)
        .args(args)
        .output()
        .expect("git runs");
    String::from_utf8_lossy(&out.stdout).to_string()
}

/// The incident this whole feature exists for.
///
/// One branch finishes a task, which *relocates* it from `## Backlog` to
/// `## Done`. The other appends a new task. A line-based merge reads the
/// relocation as a delete plus an add and conflicts; resolving it by keeping
/// both sides yields two copies of the same id, one `[ ]` and one `[x]`, and the
/// hand-editing that follows is what ate a `## Parked` header.
///
/// Through the driver it is not a conflict at all.
#[test]
fn test_merge_driver_handles_a_relocation_and_an_append() {
    let tmp = tempfile::TempDir::new().unwrap();
    if !merge_repo(tmp.path()) {
        return; // git unavailable
    }
    let root = tmp.path();

    git_must(root, &["checkout", "-q", "-b", "theirs"]);
    run_fr_ok(root, &["add", "main", "Third thing"]);
    git_must(root, &["add", "-A"]);
    git_must(root, &["commit", "-qm", "append"]);

    git_must(root, &["checkout", "-q", "main"]);
    run_fr_ok(root, &["state", "M-001", "done"]);
    git_must(root, &["add", "-A"]);
    git_must(root, &["commit", "-qm", "done"]);

    let merged = Command::new("git")
        .current_dir(root)
        .args(["merge", "theirs"])
        .output()
        .expect("git merge runs");
    assert!(
        merged.status.success(),
        "the driver should merge this cleanly:\n{}\n{}",
        String::from_utf8_lossy(&merged.stdout),
        String::from_utf8_lossy(&merged.stderr)
    );

    let track = fs::read_to_string(root.join("frame/tracks/main.md")).unwrap();
    assert!(!track.contains("<<<<<<<"), "no markers: {track}");
    // Backticked, so a `dep: M-001` reference elsewhere is not counted as a
    // second copy of the task.
    assert_eq!(
        track.matches("`M-001`").count(),
        1,
        "the relocated task must not be duplicated: {track}"
    );
    // It is finished, and it is in the section that says so.
    let (_, done) = track.split_once("## Done").unwrap();
    assert!(
        done.contains("[x] `M-001`"),
        "M-001 belongs in Done, done: {track}"
    );
    // Their addition survived.
    assert!(track.contains("Third thing"), "theirs was dropped: {track}");
    // Every section header is still there — the failure that started this.
    for header in ["## Backlog", "## Parked", "## Done"] {
        assert!(track.contains(header), "{header} was lost: {track}");
    }

    let checked = run_fr_ok(root, &["check"]);
    assert!(
        checked.contains("valid"),
        "the merged project should be valid: {checked}"
    );
}

/// Two sides editing one task differently has no right answer, so the merge
/// stops — but it stops with a *readable* file rather than one full of markers,
/// and it leaves a record that a decision is outstanding.
#[test]
fn test_merge_driver_conflict_leaves_a_valid_file_and_a_marker() {
    let tmp = tempfile::TempDir::new().unwrap();
    if !merge_repo(tmp.path()) {
        return; // git unavailable
    }
    let root = tmp.path();

    git_must(root, &["checkout", "-q", "-b", "theirs"]);
    run_fr_ok(root, &["note", "M-001", "note from them"]);
    git_must(root, &["add", "-A"]);
    git_must(root, &["commit", "-qm", "their note"]);

    git_must(root, &["checkout", "-q", "main"]);
    run_fr_ok(root, &["note", "M-001", "note from us"]);
    git_must(root, &["add", "-A"]);
    git_must(root, &["commit", "-qm", "our note"]);

    let merged = Command::new("git")
        .current_dir(root)
        .args(["merge", "theirs"])
        .output()
        .expect("git merge runs");
    assert!(
        !merged.status.success(),
        "an undecidable merge must stop the operation"
    );

    // Git knows the path is unmerged even though the file holds no markers.
    let unmerged = git_out(root, &["ls-files", "-u", "frame/tracks/main.md"]);
    assert!(
        !unmerged.trim().is_empty(),
        "the path should be staged as conflicted"
    );

    let track = fs::read_to_string(root.join("frame/tracks/main.md")).unwrap();
    assert!(
        !track.contains("<<<<<<<") && !track.contains(">>>>>>>"),
        "conflict markers would make the file unreadable to every frame tool: {track}"
    );
    assert!(track.contains("note from us"), "ours is kept: {track}");
    assert!(
        track.contains("- conflict: both-edited"),
        "the file has to record that a decision is outstanding: {track}"
    );

    // Their version is recoverable rather than lost.
    let recovery = run_fr_ok(root, &["recovery"]);
    assert!(
        recovery.contains("note from them"),
        "their version should be in the recovery log: {recovery}"
    );

    // And check reports it as an error until someone decides.
    let (checked, _, ok) = run_fr(root, &["check"]);
    assert!(
        checked.contains("unresolved merge conflict"),
        "check should report it: {checked}"
    );
    assert!(
        !checked.contains("project is valid"),
        "an unresolved conflict is not a valid project: {checked}"
    );
    assert!(
        !ok,
        "and the status should say so, so a hook or CI step can key off it"
    );

    // Resolving clears the marker and nothing else.
    run_fr_ok(root, &["merge", "--resolve", "M-001"]);
    let track = fs::read_to_string(root.join("frame/tracks/main.md")).unwrap();
    assert!(
        !track.contains("conflict:"),
        "marker should be gone: {track}"
    );
    assert!(track.contains("note from us"), "content untouched: {track}");
    let checked = run_fr_ok(root, &["check"]);
    assert!(checked.contains("valid"), "and check is happy: {checked}");
}

/// The driver must decline anything that is not a frame markdown file, so git
/// keeps using its own merge for it. `project.toml` line-merges perfectly well;
/// a driver that tried to parse it as markdown would break a working merge.
#[test]
fn test_merge_declines_a_file_it_does_not_understand() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());
    let dir = tmp.path();
    fs::write(dir.join("base.toml"), "a = 1\n").unwrap();
    fs::write(dir.join("ours.toml"), "a = 2\n").unwrap();
    fs::write(dir.join("theirs.toml"), "a = 3\n").unwrap();

    let (_, stderr, ok) = run_fr(
        dir,
        &[
            "merge",
            "--base",
            "base.toml",
            "--ours",
            "ours.toml",
            "--theirs",
            "theirs.toml",
            "--path",
            "frame/project.toml",
        ],
    );

    assert!(!ok, "declining is a non-zero status");
    assert!(
        stderr.contains("declining"),
        "it should say so plainly: {stderr}"
    );
    // Ours is left exactly as it was for git to merge itself.
    assert_eq!(
        fs::read_to_string(dir.join("ours.toml")).unwrap(),
        "a = 2\n"
    );
}

// ---------------------------------------------------------------------------
// `frame/actors.toml` under a real `git merge`
//
// The registry is committed and is deliberately *not* routed to frame's merge
// driver, so git merges it as plain text and the file's shape decides what that
// merge does. Two clones claiming tokens concurrently both add a row, which used
// to mean: always a conflict, and a conflict whose natural resolution produced an
// actor with no `name` and a registry that no longer parsed.
//
// One line per actor makes the conflict safe to resolve; sorted order makes it
// rarer. These pin both, through git itself rather than `git merge-file`.
// ---------------------------------------------------------------------------

/// Sorted order pays off once a project has actors: two claims that an existing
/// token sorts between are two insertions at *different* anchors, which git
/// merges with no conflict and no driver involved.
#[test]
fn actors_registry_merges_cleanly_when_a_token_sorts_between_two_claims() {
    let tmp = tempfile::TempDir::new().unwrap();
    if !merge_repo(tmp.path()) {
        return; // git unavailable
    }
    let root = tmp.path();

    // A project that has been around: the primary plus one more actor, with `m`
    // sitting between the two tokens that are about to be claimed.
    run_fr_ok(root, &["actor", "set", "null"]);
    run_fr_ok(root, &["actor", "set", "m"]);
    git_must(root, &["add", "-A"]);
    git_must(root, &["commit", "-qm", "two actors"]);

    git_must(root, &["checkout", "-q", "-b", "theirs"]);
    run_fr_ok(root, &["actor", "set", "t"]);
    git_must(root, &["add", "-A"]);
    git_must(root, &["commit", "-qm", "their claim"]);

    git_must(root, &["checkout", "-q", "main"]);
    run_fr_ok(root, &["actor", "set", "a"]);
    git_must(root, &["add", "-A"]);
    git_must(root, &["commit", "-qm", "our claim"]);

    let merged = Command::new("git")
        .current_dir(root)
        .args(["merge", "theirs"])
        .output()
        .expect("git merge runs");
    assert!(
        merged.status.success(),
        "a claim either side of `m` should merge cleanly:\n{}\n{}",
        String::from_utf8_lossy(&merged.stdout),
        String::from_utf8_lossy(&merged.stderr)
    );

    let registry = fs::read_to_string(root.join("frame/actors.toml")).unwrap();
    assert!(!registry.contains("<<<<<<<"), "no markers: {registry}");
    // Every actor survived, and the file still parses.
    let listed = run_fr_ok(root, &["actor", "list"]);
    for token in ["null", "a", "m", "t"] {
        assert!(
            registry.contains(&format!("\n{} = {{", token)),
            "{token} is missing from the merged registry: {registry}"
        );
        assert!(listed.contains(token), "{token} not listed: {listed}");
    }
}

/// The case sorting cannot help — two claims with nothing between them, which is
/// every project whose registry holds only the primary. It still conflicts; what
/// matters is that the conflict is one whole row per side, so the resolution
/// anyone would reach for (keep both lines) leaves a registry that parses with
/// both actors intact.
///
/// The shape this replaced conflicted on the `[actors.<token>]` header alone —
/// keeping both sides there produced an empty table and `cannot parse
/// actors.toml: missing field 'name'`.
#[test]
fn actors_registry_conflict_between_adjacent_claims_resolves_by_keeping_both() {
    let tmp = tempfile::TempDir::new().unwrap();
    if !merge_repo(tmp.path()) {
        return; // git unavailable
    }
    let root = tmp.path();

    run_fr_ok(root, &["actor", "set", "null"]);
    git_must(root, &["add", "-A"]);
    git_must(root, &["commit", "-qm", "primary only"]);

    git_must(root, &["checkout", "-q", "-b", "theirs"]);
    run_fr_ok(root, &["actor", "set", "d"]);
    git_must(root, &["add", "-A"]);
    git_must(root, &["commit", "-qm", "their claim"]);

    git_must(root, &["checkout", "-q", "main"]);
    run_fr_ok(root, &["actor", "set", "e"]);
    git_must(root, &["add", "-A"]);
    git_must(root, &["commit", "-qm", "our claim"]);

    let merged = Command::new("git")
        .current_dir(root)
        .args(["merge", "theirs"])
        .output()
        .expect("git merge runs");
    assert!(
        !merged.status.success(),
        "two claims with nothing sorting between them still conflict — \
         this pins that, so a future ordering change is a deliberate one"
    );

    // Each side of the conflict is a whole row: token *and* provenance, never a
    // bare header that would orphan the fields below it.
    let conflicted = fs::read_to_string(root.join("frame/actors.toml")).unwrap();
    for token in ["d", "e"] {
        let row = conflicted
            .lines()
            .find(|l| l.starts_with(&format!("{} = ", token)))
            .unwrap_or_else(|| panic!("no row for {token}: {conflicted}"));
        assert!(
            row.contains("name = ") && row.contains("state = ") && row.ends_with('}'),
            "{token}'s row must carry its own fields: {row}"
        );
    }

    // The resolution a human reaches for: drop the marker lines, keep both sides.
    let resolved: String = conflicted
        .lines()
        .filter(|l| {
            !l.starts_with("<<<<<<<") && !l.starts_with("=======") && !l.starts_with(">>>>>>>")
        })
        .map(|l| format!("{}\n", l))
        .collect();
    fs::write(root.join("frame/actors.toml"), &resolved).unwrap();

    let listed = run_fr_ok(root, &["actor", "list"]);
    for token in ["null", "d", "e"] {
        assert!(
            listed.contains(token),
            "keeping both sides must keep both actors: {listed}\n{resolved}"
        );
    }
    // And neither actor lost its provenance in the resolution.
    assert_eq!(
        resolved.matches("name = ").count(),
        3,
        "every row keeps its name: {resolved}"
    );
}

// ---------------------------------------------------------------------------
// Lock contention: a command that waited for the lock must not write a stale view
//
// A CLI command loads the project and *then* acquires `frame/.lock`, so
// whatever landed on disk while it waited is in the files but not in its copy.
// Writing that copy back erases it, silently and with no recovery entry. The
// window is widest exactly when contention exists — the case `ed273b2` called
// the dangerous one for the TUI, which the CLI never got.
//
// These hold the lock on a concurrent writer's behalf while a real `fr`
// subprocess blocks on it, land a write in the gap, then release.
// ---------------------------------------------------------------------------

/// Spawn `fr` while `frame/.lock` is held, let it reach the lock, run
/// `concurrent` in the gap, then release and wait for it to finish.
fn write_while_the_lock_is_held(root: &Path, args: &[&str], concurrent: impl FnOnce()) {
    let lock = frame::io::lock::FileLock::acquire_default(&root.join("frame"))
        .expect("test could not take the project lock");

    let child = Command::new(fr_bin())
        .args(args)
        .current_dir(root)
        .env("XDG_CONFIG_HOME", root.join(".xdg-config"))
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("failed to spawn fr");

    // Long enough for the child to load the project and block on the lock,
    // short enough to stay under the 5s acquire timeout.
    std::thread::sleep(std::time::Duration::from_millis(500));

    concurrent();

    drop(lock);

    let out = child.wait_with_output().expect("failed to wait for fr");
    assert!(
        out.status.success(),
        "fr {:?} failed:\nstdout: {}\nstderr: {}",
        args,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
}

#[test]
fn a_concurrent_track_write_survives_a_command_that_waited_for_the_lock() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path();
    create_test_project(root);

    let track = root.join("frame/tracks/main.md");

    write_while_the_lock_is_held(root, &["add", "main", "Added while blocked"], || {
        let before = fs::read_to_string(&track).unwrap();
        let after = before.replace(
            "- [ ] `M-001` First task #core\n",
            "- [ ] `M-001` First task #core\n\
             - [ ] `M-900` Landed while the lock was held\n  - added: 2025-05-04\n",
        );
        assert_ne!(before, after, "fixture shape changed");
        fs::write(&track, after).unwrap();
    });

    let body = fs::read_to_string(&track).unwrap();
    assert!(
        body.contains("Added while blocked"),
        "the command's own write is missing:\n{body}"
    );
    assert!(
        body.contains("M-900"),
        "the concurrent write was erased:\n{body}"
    );
}

#[test]
fn a_concurrent_inbox_capture_survives_a_command_that_waited_for_the_lock() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path();
    create_test_project(root);

    let inbox = root.join("frame/inbox.md");

    write_while_the_lock_is_held(root, &["inbox", "Captured while blocked"], || {
        let before = fs::read_to_string(&inbox).unwrap();
        fs::write(&inbox, format!("{before}\n- Captured elsewhere first\n")).unwrap();
    });

    let body = fs::read_to_string(&inbox).unwrap();
    assert!(
        body.contains("Captured while blocked"),
        "the command's own capture is missing:\n{body}"
    );
    assert!(
        body.contains("Captured elsewhere first"),
        "the concurrent capture was erased:\n{body}"
    );
}

// ---------------------------------------------------------------------------
// The recovery log survives its own rewrites
//
// The log is the copy of last resort: everything in it reached nowhere else.
// Both places that shrink it used to truncate in place — `File::create` in the
// inline trim, `fs::write` in the prune — so an interruption between the
// truncate and the write destroyed the file. Both go through `atomic_write`
// now, which is what makes `FRAME_FAIL_WRITE` able to cut them and these tests
// able to say what an interruption leaves behind.
// ---------------------------------------------------------------------------

/// Write a recovery log holding `count` entries, all old enough to be prunable.
///
/// A fixed past timestamp rather than a computed one: the prune cutoff is 30
/// days, and 2020 will still be older than it.
fn seed_recovery_log(root: &Path, count: usize, body: &str) {
    const TS: &str = "2020-01-01T00:00:00Z";
    let mut content = String::from(
        "<!-- frame recovery log — append-only error recovery data\n     \
         This file captures data that Frame couldn't save normally.\n     \
         If something went missing, check here.\n     View with: fr recovery\n     \
         Prune old entries: fr recovery prune\n     \
         Safe to delete if empty or stale. -->\n\n---\n",
    );
    for i in 0..count {
        content.push_str(&format!(
            "## {TS} — write: seeded {i}\n\nSource: tracks/main.md\n\n```text\n{body}\n```\n\n---\n"
        ));
    }
    fs::write(root.join("frame/.recovery.log"), content).unwrap();
}

/// `fr recovery prune --all` whose rewrite fails must leave every entry where
/// it was. Truncating in place emptied the log and reported the error after.
#[test]
fn a_failed_prune_leaves_the_recovery_log_whole() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path();
    create_test_project(root);
    seed_recovery_log(root, 3, "content that exists nowhere else");

    let log = root.join("frame/.recovery.log");
    let before = fs::read_to_string(&log).unwrap();

    let (_, stderr, ok) = run_fr_env(
        root,
        &["recovery", "prune", "--all"],
        &[("FRAME_FAIL_WRITE", ".recovery.log")],
    );
    assert!(!ok, "the injected failure must surface: {stderr}");

    assert_eq!(
        fs::read_to_string(&log).unwrap(),
        before,
        "a prune that could not write must not have removed anything"
    );
}

/// Same for the age-based prune, which takes the other branch.
#[test]
fn a_failed_dated_prune_leaves_the_recovery_log_whole() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path();
    create_test_project(root);
    seed_recovery_log(root, 2, "content that exists nowhere else");

    let log = root.join("frame/.recovery.log");
    let before = fs::read_to_string(&log).unwrap();

    let (_, stderr, ok) = run_fr_env(
        root,
        &["recovery", "prune"],
        &[("FRAME_FAIL_WRITE", ".recovery.log")],
    );
    assert!(!ok, "the injected failure must surface: {stderr}");
    assert_eq!(fs::read_to_string(&log).unwrap(), before);
}

/// The inline trim fires from inside `log_recovery` once the log passes 1 MB —
/// in the middle of an operation that is already logging because something went
/// wrong. A failure there must cost nothing: the old entries stay, and the new
/// entry, which is the whole reason we were here, still lands.
///
/// Only the trim goes through `atomic_write`; the append is an `O_APPEND`
/// write, so the injected failure cuts the trim and leaves the append alone.
#[test]
fn a_failed_inline_trim_keeps_the_log_and_still_appends() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path();
    create_test_project(root);

    // Past MAX_LOG_SIZE (1 MB), old enough that the trim would remove it.
    let filler = "x".repeat(2048);
    seed_recovery_log(root, 600, &filler);
    let log = root.join("frame/.recovery.log");
    assert!(fs::metadata(&log).unwrap().len() > 1_048_576);
    let before = fs::read_to_string(&log).unwrap();

    // `fr delete` logs the task's source text to the recovery log.
    let (_, stderr, ok) = run_fr_env(
        root,
        &["delete", "M-001", "--yes"],
        &[("FRAME_FAIL_WRITE", ".recovery.log")],
    );
    assert!(ok, "the delete itself must still succeed: {stderr}");

    let after = fs::read_to_string(&log).unwrap();
    assert!(
        after.starts_with(&before),
        "the trim failed, so nothing should have been removed"
    );
    assert!(
        after.len() > before.len(),
        "and the entry that mattered still had to land"
    );
    assert!(after.contains("M-001"), "with the deleted task in it");
}

/// `fr track rename --id` moves the track file, moves the archive, then writes
/// the config — with the whole `--prefix` block in between. Cut the file move
/// and the config keeps naming a file that is gone; `load_project` skips such a
/// track, so it and every task in it drop out of `fr list`, the TUI, and every
/// other check. `fr check` reported none of that until `track_file_missing`.
#[test]
fn test_track_rename_recovers_from_an_interrupted_file_move() {
    let tmp = tempfile::TempDir::new().unwrap();
    two_track_project(tmp.path());
    let root = tmp.path();

    // The rename moves tracks/a.md; cut it after the marker is written.
    let (_, _, ok) = run_fr_env(
        root,
        &["track", "rename", "a", "--new-id", "alpha"],
        &[("FRAME_FAIL_WRITE", "tracks/a.md")],
    );
    assert!(!ok, "the injected failure should fail the command");

    // Nothing moved and the config is untouched, so the project is still whole.
    assert!(root.join("frame/tracks/a.md").exists());
    assert!(
        root.join("frame/.inflight").exists(),
        "the intent is recorded"
    );

    // Any following write command completes the rename.
    run_fr_ok(root, &["add", "b", "unrelated"]);

    assert!(
        root.join("frame/tracks/alpha.md").exists(),
        "recovery should finish the file move"
    );
    assert!(!root.join("frame/tracks/a.md").exists());
    let config = fs::read_to_string(root.join("frame/project.toml")).unwrap();
    assert!(
        config.contains("id = \"alpha\""),
        "and the config entry with it: {config}"
    );
    assert!(
        !root.join("frame/.inflight").exists(),
        "the marker is cleared once the operation is complete"
    );

    // The whole point: the track is visible again, tasks intact.
    let out = run_fr_ok(root, &["list"]);
    assert!(
        out.contains("the task to move"),
        "tasks are back in view: {out}"
    );
    let check = run_fr_ok(root, &["check"]);
    assert!(check.contains("valid"), "and the project is clean: {check}");
}

/// The half-applied state `fr check` used to call valid. Reached here by hand
/// rather than by a crash, because that is the point: a merge that took one
/// side's `project.toml` and the other's file layout, a manual `mv`, or an
/// editor's "rename file" all land in it.
#[test]
fn a_track_file_renamed_out_from_under_config_is_reported() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path();
    create_test_project(root);

    fs::rename(
        root.join("frame/tracks/main.md"),
        root.join("frame/tracks/renamed.md"),
    )
    .unwrap();

    let (stdout, _, ok) = run_fr(root, &["check"]);
    assert!(
        !ok && stdout.contains("project has errors"),
        "a track nobody can see is not a clean bill: {stdout}"
    );
    assert!(
        stdout.contains("track file is missing") && stdout.contains("tracks/main.md"),
        "the dangling config entry: {stdout}"
    );
    assert!(
        stdout.contains("not listed in project.toml") && stdout.contains("tracks/renamed.md"),
        "and the file nothing points at: {stdout}"
    );
}

/// An archived track keeps `file = "tracks/<id>.md"` in config while the file
/// itself lives in `archive/_tracks/`. That is the expected state, not damage,
/// and reporting it would fire on every project that has ever archived a track.
#[test]
fn an_archived_track_is_not_reported_as_missing() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path();
    two_track_project(root);

    run_fr_ok(root, &["track", "archive", "a"]);

    let (stdout, _, _) = run_fr(root, &["check"]);
    assert!(
        stdout.contains("valid"),
        "archiving is not damage: {stdout}"
    );
    assert!(!stdout.contains("track file is missing"), "{stdout}");
}

/// A project `fr clean` will actually archive from: two done tasks over a
/// threshold of one. The shared fixture's threshold is 100, so clean does
/// nothing there and the crash window never opens.
fn clean_ready_project(root: &Path) {
    let frame_dir = root.join("frame");
    fs::create_dir_all(frame_dir.join("tracks")).unwrap();
    fs::write(frame_dir.join(".actor"), "null\n").unwrap();
    fs::write(
        frame_dir.join("project.toml"),
        "[project]\nname = \"clean-test\"\n\n\
         [clean]\ndone_threshold = 1\ndone_retain = 0\n\n\
         [[tracks]]\nid = \"main\"\nname = \"Main\"\nstate = \"active\"\n\
         file = \"tracks/main.md\"\n\n[ids.prefixes]\nmain = \"M\"\n",
    )
    .unwrap();
    fs::write(
        frame_dir.join("tracks/main.md"),
        "# Main\n\n## Backlog\n\n\
         - [ ] `M-005` Still open\n  - added: 2026-01-01\n\n\
         ## Done\n\n\
         - [x] `M-001` Archive me\n  - added: 2026-01-01\n  - resolved: 2026-01-02\n\
         - [x] `M-002` Archive me too\n  - added: 2026-01-01\n  - resolved: 2026-01-02\n",
    )
    .unwrap();
    fs::write(frame_dir.join("inbox.md"), "# Inbox\n").unwrap();
}

/// `fr clean` appends to the archive before removing from the track, so an
/// interruption duplicates rather than loses — and the duplicate is
/// self-healing (`9e183a8`). `src/io/fault.rs` names this as the ordering the
/// harness exists to verify; nothing verified it until now.
#[test]
fn test_clean_keeps_the_task_when_the_archive_write_is_cut() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path();
    clean_ready_project(root);

    let (_, stderr, ok) = run_fr_env(root, &["clean"], &[("FRAME_FAIL_WRITE", "archive/main.md")]);
    // Clean degrades rather than aborting: it skips the track whose archive it
    // could not write and says so. That is the ordering doing its job — there
    // is nothing to roll back, because nothing was removed yet.
    assert!(
        ok,
        "clean should skip the track, not fail outright: {stderr}"
    );
    assert!(
        stderr.contains("could not write archive"),
        "and it should say which track it skipped: {stderr}"
    );

    // Append-before-remove means the tasks are still in the track: the archive
    // write is the one that was cut, so nothing was removed on its strength.
    let track = fs::read_to_string(root.join("frame/tracks/main.md")).unwrap();
    assert!(
        track.contains("M-001") && track.contains("M-002"),
        "no task may leave the track before its archive copy lands: {track}"
    );

    // And a re-run completes it, without duplicating.
    run_fr_ok(root, &["clean"]);
    let track = fs::read_to_string(root.join("frame/tracks/main.md")).unwrap();
    let archive = fs::read_to_string(root.join("frame/archive/main.md")).unwrap();
    assert!(!track.contains("M-001"), "now removed from the track");
    assert_eq!(
        archive.matches("`M-001`").count(),
        1,
        "and archived exactly once: {archive}"
    );
    assert_eq!(archive.matches("`M-002`").count(), 1, "{archive}");
}

// ---------------------------------------------------------------------------
// `fr check` reports soundness in its exit status
// ---------------------------------------------------------------------------

/// A clean project exits 0 — the baseline the rest of this depends on.
#[test]
fn check_exits_zero_on_a_clean_project() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path();
    create_test_project(root);

    let (stdout, _, ok) = run_fr(root, &["check"]);
    assert!(ok, "a clean project must not report failure: {stdout}");
}

/// Errors set the status, so a pre-commit hook or a CI step can key off it
/// instead of grepping stdout. It used to print `✗ project has errors` and exit
/// 0, which meant `fr check && ...` ran the `&&` branch on a broken project.
#[test]
fn check_exits_non_zero_on_errors() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path();
    create_test_project(root);

    // A dangling dep: an error, and one with no repair.
    let track = root.join("frame/tracks/main.md");
    let body = fs::read_to_string(&track)
        .unwrap()
        .replace("- dep: M-001", "- dep: M-999");
    fs::write(&track, body).unwrap();

    let (stdout, _, ok) = run_fr(root, &["check"]);
    assert!(!ok, "errors must set the status: {stdout}");
    assert!(stdout.contains("project has errors"), "{stdout}");
}

/// Warnings do not. The status answers "is this project sound", and a warning
/// is by definition something frame is willing to live with — gating a commit
/// on one would make the whole signal useless.
#[test]
fn check_exits_zero_when_there_are_only_warnings() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path();
    create_test_project(root);

    // A task with no ID: a warning, and one `fr clean` fixes routinely.
    let track = root.join("frame/tracks/main.md");
    let body = fs::read_to_string(&track)
        .unwrap()
        .replace("## Parked", "- [ ] No ID at all\n\n## Parked");
    fs::write(&track, body).unwrap();

    let (stdout, _, ok) = run_fr(root, &["check"]);
    assert!(
        stdout.contains("Warnings:"),
        "the fixture should warn: {stdout}"
    );
    assert!(!stdout.contains("Errors:"), "and only warn: {stdout}");
    assert!(ok, "a warning is not a failure: {stdout}");
}

/// `--json` agrees with the status, so a consumer can use either.
#[test]
fn check_json_exits_non_zero_on_errors_too() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path();
    create_test_project(root);

    let track = root.join("frame/tracks/main.md");
    let body = fs::read_to_string(&track)
        .unwrap()
        .replace("- dep: M-001", "- dep: M-999");
    fs::write(&track, body).unwrap();

    let (stdout, _, ok) = run_fr(root, &["check", "--json"]);
    assert!(!ok, "{stdout}");
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("valid json");
    assert_eq!(v["valid"], serde_json::Value::Bool(false));
}

/// `--fix` follows the same rule, on the state it leaves behind: it repairs
/// what it can, and an error it could not repair is still an error.
#[test]
fn check_fix_exits_non_zero_when_errors_remain() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path();
    create_test_project(root);

    let track = root.join("frame/tracks/main.md");
    let body = fs::read_to_string(&track)
        .unwrap()
        .replace("- dep: M-001", "- dep: M-999");
    fs::write(&track, body).unwrap();

    let (stdout, _, ok) = run_fr(root, &["check", "--fix", "--yes"]);
    assert!(
        !ok,
        "a dangling dep has no repair, so the project is still unsound: {stdout}"
    );
}

// ---------------------------------------------------------------------------
// Un-archiving brings the file back
//
// `fr track archive` moves the track file to `archive/_tracks/`. `fr track
// activate` used to set `state = "active"` and stop there, leaving the config
// naming a file that is not in `tracks/` — and `load_project` skips such a
// track, so it and every task in it left the project while the command printed
// success. The TUI's unarchive has always moved the file back.
// ---------------------------------------------------------------------------

/// A track archived and then activated must be whole again: file in place,
/// tasks visible, project clean.
#[test]
fn activating_an_archived_track_brings_its_file_back() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path();
    two_track_project(root);

    run_fr_ok(root, &["track", "archive", "a"]);
    assert!(
        root.join("frame/archive/_tracks/a.md").exists(),
        "archive moved the file"
    );
    assert!(!root.join("frame/tracks/a.md").exists());

    run_fr_ok(root, &["track", "activate", "a"]);

    assert!(
        root.join("frame/tracks/a.md").exists(),
        "activate must bring it back"
    );
    assert!(
        !root.join("frame/archive/_tracks/a.md").exists(),
        "and not leave a second copy behind"
    );

    let out = run_fr_ok(root, &["list"]);
    assert!(
        out.contains("the task to move"),
        "the tasks are visible again: {out}"
    );

    let (check, _, ok) = run_fr(root, &["check"]);
    assert!(ok, "and the project is sound: {check}");
}

/// Activating a *shelved* track is a config edit and nothing more — its file
/// never moved, so there is nothing to restore and nothing to break.
#[test]
fn activating_a_shelved_track_touches_no_files() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path();
    two_track_project(root);

    run_fr_ok(root, &["track", "shelve", "a"]);
    let before = fs::read_to_string(root.join("frame/tracks/a.md")).unwrap();

    run_fr_ok(root, &["track", "activate", "a"]);

    assert_eq!(
        fs::read_to_string(root.join("frame/tracks/a.md")).unwrap(),
        before,
        "a shelved track's file stays exactly where it was"
    );
    let (check, _, ok) = run_fr(root, &["check"]);
    assert!(ok, "{check}");
}

/// Activating an already-active track is a no-op, not an error. It reaches
/// `restore_track_file` with nothing to move in the marker-recovery path, so
/// the idempotence guard there has to hold.
#[test]
fn activating_an_active_track_is_harmless() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path();
    two_track_project(root);

    run_fr_ok(root, &["track", "activate", "a"]);
    assert!(root.join("frame/tracks/a.md").exists());
    let (check, _, ok) = run_fr(root, &["check"]);
    assert!(ok, "{check}");
}

/// Cutting the restore leaves the file where it started, in
/// `archive/_tracks/` — recoverable, with nothing lost. This is the test the
/// fault hook on `restore_track_file` was added for and had no caller to reach
/// it from until `activate` gained one.
///
/// The half-applied state here is the dangerous one: the config already says
/// active while the file is not in `tracks/`, so the track is *absent* from the
/// project rather than merely misfiled. The next write command has to finish it.
#[test]
fn a_cut_unarchive_is_completed_by_the_next_write_command() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path();
    two_track_project(root);

    run_fr_ok(root, &["track", "archive", "a"]);

    let (_, stderr, ok) = run_fr_env(
        root,
        &["track", "activate", "a"],
        &[("FRAME_FAIL_WRITE", "_tracks/a.md")],
    );
    assert!(
        !ok,
        "the injected failure should fail the command: {stderr}"
    );

    // Nothing was lost — the file is still where it started.
    assert!(
        root.join("frame/archive/_tracks/a.md").exists(),
        "the file must still be somewhere — the move is what was cut"
    );
    let archived = fs::read_to_string(root.join("frame/archive/_tracks/a.md")).unwrap();
    assert!(archived.contains("the task to move"), "intact: {archived}");

    // But the track is invisible until this is finished, which is why the state
    // must not be left standing.
    assert!(
        root.join("frame/.inflight").exists(),
        "the intent is recorded"
    );
    let (check, _, _) = run_fr(root, &["check"]);
    assert!(
        check.contains("track file is missing"),
        "and check says so meanwhile: {check}"
    );

    // Any following write command completes it.
    run_fr_ok(root, &["add", "b", "unrelated"]);

    assert!(
        root.join("frame/tracks/a.md").exists(),
        "recovery should finish the move back"
    );
    assert!(!root.join("frame/archive/_tracks/a.md").exists());
    assert!(
        !root.join("frame/.inflight").exists(),
        "and clear the marker"
    );

    let out = run_fr_ok(root, &["list"]);
    assert!(out.contains("the task to move"), "tasks are back: {out}");
    let (check, _, ok) = run_fr(root, &["check"]);
    assert!(ok, "and the project is sound again: {check}");
}

// ---------------------------------------------------------------------------
// The other direction: a real `fr` writes, and a TUI session saves over it
//
// The pair above covers the CLI side, which is the side `ed273b2` fixed. These
// cover the TUI side, and they exist because `tests/concurrency.rs` — the
// property that found the defect — *models* the CLI writer rather than
// spawning it. The model mirrors `lock_and_load` so that TUI keystrokes can run
// between the CLI's load and its write, which a subprocess cannot be paused to
// allow. Its stated risk is that a real `fr` might not write what the model
// writes; these two spend a process each to close that gap on the one case that
// matters.
//
// No reload happens in between, deliberately. Relying on the watcher having
// delivered the event first is exactly what made an asynchronous notification
// load-bearing for correctness: the gap between the other process writing and
// the event loop polling is sub-millisecond, and the watcher can fail to start
// at all.
// ---------------------------------------------------------------------------

/// Load a project the way the TUI does, with no watcher attached.
fn tui_session(root: &Path) -> frame::tui::app::App {
    let project = frame::io::project_io::load_project(root).expect("project loads");
    frame::tui::app::App::new(project)
}

#[test]
fn a_tui_save_does_not_erase_a_task_a_real_fr_just_added() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path();
    create_test_project(root);
    let track = root.join("frame/tracks/main.md");

    // The session has the project open.
    let mut app = tui_session(root);

    // An agent runs `fr add` in another terminal.
    let out = Command::new(fr_bin())
        .args(["add", "main", "Added by a real fr"])
        .current_dir(root)
        .env("XDG_CONFIG_HOME", root.join(".xdg-config"))
        .output()
        .expect("failed to run fr");
    assert!(out.status.success(), "fr add failed: {out:?}");

    // The user presses a key before the watcher delivers anything.
    let tasks = app
        .find_track_mut("main")
        .unwrap()
        .section_tasks_mut(frame::model::SectionKind::Backlog)
        .unwrap();
    tasks[0].title = "Edited in the TUI".into();
    tasks[0].dirty = true;
    app.save_track_logged("main");

    let body = fs::read_to_string(&track).unwrap();
    assert!(
        body.contains("Added by a real fr"),
        "the other process's task was erased by the TUI's save:\n{body}"
    );
    assert!(
        body.contains("Edited in the TUI"),
        "the TUI's own edit did not land:\n{body}"
    );
}

#[test]
fn a_tui_save_does_not_erase_a_capture_a_real_fr_just_made() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path();
    create_test_project(root);
    let inbox = root.join("frame/inbox.md");

    let mut app = tui_session(root);

    let out = Command::new(fr_bin())
        .args(["inbox", "Captured by a real fr"])
        .current_dir(root)
        .env("XDG_CONFIG_HOME", root.join(".xdg-config"))
        .output()
        .expect("failed to run fr");
    assert!(out.status.success(), "fr inbox failed: {out:?}");

    frame::ops::inbox_ops::add_inbox_item(
        app.project.inbox.as_mut().unwrap(),
        "Captured in the TUI".into(),
        Vec::new(),
        None,
    );
    app.save_inbox_logged();

    let body = fs::read_to_string(&inbox).unwrap();
    assert!(
        body.contains("Captured by a real fr"),
        "the other process's capture was erased by the TUI's save:\n{body}"
    );
    assert!(
        body.contains("Captured in the TUI"),
        "the TUI's own capture did not land:\n{body}"
    );
}

// ---------------------------------------------------------------------------
// An archived track is frozen, including its name
//
// Rename was the one mutation that did not refuse an archived track, and it did
// not work either. `--name` rewrote `project.toml` and silently skipped the
// file's `# Title`, because `file` still reads `tracks/<id>.md` for a track
// whose file the archive moved to `archive/_tracks/`. `--new-id` moved the
// done-task archive, left the track file under the old id, and printed success
// on a project `fr check` then called an error. `--prefix` refused with `track
// not found`, which is false — the track exists; `load_project` never loaded it.
//
// The way out is `activate`, rename, `archive`, pinned below as the reason
// refusing costs no capability.
// ---------------------------------------------------------------------------

/// Set up `a` as an archived track with a done-task archive beside it, so a
/// rename has both files to get wrong.
fn archived_track_project(root: &Path) {
    two_track_project(root);
    run_fr_ok(root, &["add", "a", "a task that finished"]);
    run_fr_ok(root, &["state", "A-002", "done"]);
    // A done-task archive at `archive/a.md`, which is what `rename_track_id`
    // moves while leaving the track file behind.
    let config = root.join("frame/project.toml");
    let text = fs::read_to_string(&config).unwrap();
    fs::write(
        &config,
        text.replace("done_threshold = 100", "done_threshold = 0")
            .replace("done_retain = 10", "done_retain = 0"),
    )
    .unwrap();
    run_fr_ok(root, &["clean"]);
    assert!(
        root.join("frame/archive/a.md").exists(),
        "the done-task archive is part of the fixture"
    );
    run_fr_ok(root, &["track", "archive", "a"]);
}

/// Every flag refuses, nothing on disk moves, and the message names the way out.
#[test]
fn renaming_an_archived_track_refuses_on_every_flag() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path();
    archived_track_project(root);

    let before = fs::read_to_string(root.join("frame/project.toml")).unwrap();
    let file_before = fs::read_to_string(root.join("frame/archive/_tracks/a.md")).unwrap();
    let archive_before = fs::read_to_string(root.join("frame/archive/a.md")).unwrap();

    for flags in [
        vec!["--name", "Renamed"],
        vec!["--new-id", "b2"],
        vec!["--prefix", "QQ", "--yes"],
    ] {
        let mut args = vec!["track", "rename", "a"];
        args.extend(flags.iter().copied());
        let (_, stderr, ok) = run_fr(root, &args);
        assert!(!ok, "{flags:?} must refuse an archived track");
        assert!(
            stderr.contains("archived") && stderr.contains("fr track activate a"),
            "the message must say why and how to proceed, not `track not found`: {stderr}"
        );
    }

    assert_eq!(
        fs::read_to_string(root.join("frame/project.toml")).unwrap(),
        before,
        "a refused rename writes no config"
    );
    assert_eq!(
        fs::read_to_string(root.join("frame/archive/_tracks/a.md")).unwrap(),
        file_before,
        "nor the archived track file"
    );
    assert_eq!(
        fs::read_to_string(root.join("frame/archive/a.md")).unwrap(),
        archive_before,
        "nor the done-task archive — which `--prefix` used to rewrite before failing"
    );
    let (check, _, ok) = run_fr(root, &["check"]);
    assert!(ok, "and the project stays sound: {check}");
}

/// Deleting one refuses too, and had the same false message for the same
/// reason: `find_track` only sees tracks that loaded.
#[test]
fn deleting_an_archived_track_says_it_is_archived() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path();
    archived_track_project(root);

    let (_, stderr, ok) = run_fr(root, &["track", "delete", "a"]);
    assert!(!ok);
    assert!(
        stderr.contains("archived") && stderr.contains("fr track activate a"),
        "an archived track exists; deleting it is what is not on offer: {stderr}"
    );
}

/// A track that really is absent still gets `track not found` — from every flag,
/// including `--prefix`, which used to report `no prefix configured` instead.
#[test]
fn renaming_a_missing_track_still_says_not_found() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path();
    two_track_project(root);

    for flags in [
        vec!["--name", "X"],
        vec!["--new-id", "y"],
        vec!["--prefix", "QQ", "--yes"],
    ] {
        let mut args = vec!["track", "rename", "ghost"];
        args.extend(flags.iter().copied());
        let (_, stderr, ok) = run_fr(root, &args);
        assert!(!ok, "{flags:?}");
        assert!(
            stderr.contains("track not found: ghost"),
            "{flags:?} should say the track is missing: {stderr}"
        );
    }
}

/// Refusing costs two commands, not capability: the round trip renames both
/// files and the archived ids, and lands sound.
#[test]
fn unarchiving_to_rename_and_re_archiving_lands_sound() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path();
    archived_track_project(root);

    run_fr_ok(root, &["track", "activate", "a"]);
    run_fr_ok(root, &["track", "rename", "a", "--new-id", "a2"]);
    run_fr_ok(root, &["track", "rename", "a2", "--prefix", "AA", "--yes"]);
    run_fr_ok(root, &["track", "archive", "a2"]);

    assert!(
        root.join("frame/archive/_tracks/a2.md").exists(),
        "the track file followed the id"
    );
    assert!(!root.join("frame/archive/_tracks/a.md").exists());
    let archive = fs::read_to_string(root.join("frame/archive/a2.md")).unwrap();
    assert!(
        archive.contains("AA-002"),
        "and so did the archived task ids: {archive}"
    );
    let (check, _, ok) = run_fr(root, &["check"]);
    assert!(ok, "{check}");
}

/// The detector for the residue an old frame left behind: a file under
/// `archive/_tracks/` that no archived track claims. A warning, so the project
/// still exits 0 — it is archived content, not live work gone missing.
#[test]
fn an_unclaimed_archived_track_file_is_reported_as_a_warning() {
    let tmp = tempfile::TempDir::new().unwrap();
    let root = tmp.path();
    archived_track_project(root);

    // Exactly what `fr track rename --new-id` used to leave: the config row
    // takes the new id, the file keeps the old one.
    let config = root.join("frame/project.toml");
    let text = fs::read_to_string(&config).unwrap();
    fs::write(&config, text.replace("id = \"a\"", "id = \"a9\"")).unwrap();

    let (out, _, ok) = run_fr(root, &["check"]);
    assert!(
        !ok,
        "the missing file the row now names is still an error: {out}"
    );
    assert!(
        out.contains("archive/_tracks/a9.md"),
        "the row points nowhere: {out}"
    );
    assert!(
        out.contains("archive/_tracks/a.md"),
        "and the file it left behind is named too, which is the pair that says \
         what to move: {out}"
    );
}

// ---------------------------------------------------------------------------
// `fr clean --normalize`
// ---------------------------------------------------------------------------

/// A project written before the canonical order existed converges on request.
///
/// The serializer already orders a task the first time frame edits it, so a
/// project gets there on its own eventually; this is that convergence asked for
/// at once, for the tasks nobody is about to touch.
#[test]
fn clean_normalize_reorders_and_reports() {
    let tmp = tempfile::tempdir().unwrap();
    create_test_project(tmp.path());
    let track = tmp.path().join("frame/tracks/main.md");
    fs::write(
        &track,
        "\
# Main Track

## Backlog

- [ ] `M-001` Untouched and already in order
  - added: 2026-01-01
  - note: body

## Parked

## Done

- [x] `M-002` Date appended after the note
  - added: 2026-01-01
  - note: a note long enough to hide what follows it
  - resolved: 2026-01-02
",
    )
    .unwrap();

    let out = run_fr_ok(tmp.path(), &["clean", "--normalize"]);
    assert!(out.contains("Field order normalized:"), "{out}");
    assert!(out.contains("M-002"), "{out}");
    assert!(
        !out.contains("M-001"),
        "a task already in order should not be reported: {out}"
    );

    let text = fs::read_to_string(&track).unwrap();
    let resolved = text.find("resolved:").unwrap();
    let note = text.find("a note long enough").unwrap();
    assert!(resolved < note, "not reordered:\n{text}");

    // The task that was already in order keeps its bytes.
    assert!(
        text.contains(
            "- [ ] `M-001` Untouched and already in order\n  - added: 2026-01-01\n  - note: body\n"
        ),
        "an ordered task was rewritten anyway:\n{text}"
    );
}

/// `--dry-run` composes with it and writes nothing.
#[test]
fn clean_normalize_dry_run_writes_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    create_test_project(tmp.path());
    let track = tmp.path().join("frame/tracks/main.md");
    let original = "\
# Main Track

## Backlog

## Parked

## Done

- [x] `M-002` Out of order
  - added: 2026-01-01
  - note: body
  - resolved: 2026-01-02
";
    fs::write(&track, original).unwrap();

    let out = run_fr_ok(tmp.path(), &["clean", "--normalize", "--dry-run"]);
    assert!(out.contains("Field order normalized:"), "{out}");
    assert!(out.contains("(dry run — no changes written)"), "{out}");
    assert_eq!(fs::read_to_string(&track).unwrap(), original);
}

/// **A plain `fr clean` does not reorder.** This is the guarantee that matters:
/// clean runs unattended after every reload when the TUI's `auto_clean` is on,
/// so it may only do what is correct with nobody watching, and rewriting every
/// task in a project is not that — `fr clean` filling one `resolved:` date has
/// already hidden a one-line deletion inside a large boring diff once.
#[test]
fn clean_without_normalize_leaves_field_order_alone() {
    let tmp = tempfile::tempdir().unwrap();
    create_test_project(tmp.path());
    let track = tmp.path().join("frame/tracks/main.md");
    let original = "\
# Main Track

## Backlog

## Parked

## Done

- [x] `M-002` Out of order
  - added: 2026-01-01
  - note: body
  - resolved: 2026-01-02
";
    fs::write(&track, original).unwrap();

    let out = run_fr_ok(tmp.path(), &["clean"]);
    assert!(!out.contains("Field order normalized"), "{out}");
    assert_eq!(fs::read_to_string(&track).unwrap(), original);
}

/// Running it twice changes nothing the second time.
#[test]
fn clean_normalize_is_idempotent() {
    let tmp = tempfile::tempdir().unwrap();
    create_test_project(tmp.path());
    let track = tmp.path().join("frame/tracks/main.md");
    fs::write(
        &track,
        "\
# Main Track

## Backlog

- [ ] `M-001` Task
  - added: 2026-01-01
  - note: body
  - ref: src/a.rs

## Parked

## Done
",
    )
    .unwrap();

    run_fr_ok(tmp.path(), &["clean", "--normalize"]);
    let once = fs::read_to_string(&track).unwrap();

    let out = run_fr_ok(tmp.path(), &["clean", "--normalize"]);
    assert!(!out.contains("Field order normalized"), "{out}");
    assert_eq!(fs::read_to_string(&track).unwrap(), once);
}

/// A plain `fr clean` says a legacy project has fields out of order, and names
/// the flag that fixes them.
///
/// Field order is not damage, so it is not an `fr check` finding — without this
/// line there is nowhere to learn that `--normalize` exists, and clean would
/// call a project "clean" while holding hundreds of tasks it would rewrite the
/// moment anyone asked. A summary rather than a row per task: this is the
/// report-only path, and it must not bury clean's other findings.
#[test]
fn clean_reports_out_of_order_fields_without_changing_them() {
    let tmp = tempfile::tempdir().unwrap();
    create_test_project(tmp.path());
    let track = tmp.path().join("frame/tracks/main.md");
    let original = "\
# Main Track

## Backlog

- [ ] `M-001` One
  - added: 2026-01-01
  - note: body
  - ref: src/a.rs
- [ ] `M-003` Two
  - added: 2026-01-01
  - note: body
  - ref: src/b.rs

## Parked

## Done
";
    fs::write(&track, original).unwrap();

    let out = run_fr_ok(tmp.path(), &["clean", "--dry-run"]);
    assert!(out.contains("Field order:"), "{out}");
    assert!(
        out.contains("2 tasks have fields out of canonical order"),
        "{out}"
    );
    assert!(out.contains("fr clean --normalize"), "{out}");
    // Report only — not the per-task list, and not a rewrite.
    assert!(!out.contains("Field order normalized"), "{out}");
    assert_eq!(fs::read_to_string(&track).unwrap(), original);
}

/// One task reads as one task.
#[test]
fn clean_field_order_summary_is_singular_for_one_task() {
    let tmp = tempfile::tempdir().unwrap();
    create_test_project(tmp.path());
    fs::write(
        tmp.path().join("frame/tracks/main.md"),
        "\
# Main Track

## Backlog

- [ ] `M-001` Only one
  - added: 2026-01-01
  - note: body
  - ref: src/a.rs

## Parked

## Done
",
    )
    .unwrap();

    let out = run_fr_ok(tmp.path(), &["clean", "--dry-run"]);
    assert!(
        out.contains("1 task has fields out of canonical order"),
        "{out}"
    );
    assert!(out.contains("to rewrite it"), "{out}");
}

/// An already-ordered project still reports itself clean — the new report must
/// not fire on every project forever.
#[test]
fn clean_says_nothing_about_field_order_when_there_is_nothing_to_say() {
    let tmp = tempfile::tempdir().unwrap();
    create_test_project(tmp.path());
    fs::write(
        tmp.path().join("frame/tracks/main.md"),
        "\
# Main Track

## Backlog

- [ ] `M-001` In order
  - added: 2026-01-01
  - note: body

## Parked

## Done
",
    )
    .unwrap();

    let out = run_fr_ok(tmp.path(), &["clean"]);
    assert!(!out.contains("Field order"), "{out}");
    assert!(out.contains("✓ project is clean"), "{out}");
}

/// The per-task line says what the order was and what it became. It used to
/// read `M-001 was added, ref, note`, which parses as the verb "was added".
#[test]
fn clean_normalize_names_both_orders() {
    let tmp = tempfile::tempdir().unwrap();
    create_test_project(tmp.path());
    fs::write(
        tmp.path().join("frame/tracks/main.md"),
        "\
# Main Track

## Backlog

## Parked

## Done

- [x] `M-002` Out of order
  - added: 2026-01-01
  - note: body
  - resolved: 2026-01-02
",
    )
    .unwrap();

    let out = run_fr_ok(tmp.path(), &["clean", "--normalize"]);
    assert!(
        out.contains("[main] M-002: added, note, resolved → added, resolved, note"),
        "{out}"
    );
}

/// `fr clean --json` describes the same run the human surface does.
///
/// The two flags are what the arrays cannot say on their own: `dry_run` is
/// whether anything was written, and `normalize` is what `field_order.reordered`
/// *means* — with it those tasks were rewritten, without it they were only
/// found. A consumer ignoring the flag would read a preview as a result.
#[test]
fn clean_json_reports_the_same_run() {
    let tmp = tempfile::tempdir().unwrap();
    create_test_project(tmp.path());
    let track = tmp.path().join("frame/tracks/main.md");
    let original = "\
# Main Track

## Backlog

- [ ] `M-001` Out of order
  - added: 2026-01-01
  - note: body
  - dep: M-404
- [ ] No id at all

## Parked

## Done
";
    fs::write(&track, original).unwrap();

    // Preview: reports, changes nothing, and says so.
    let out = run_fr_ok(tmp.path(), &["--json", "clean", "--dry-run"]);
    let v: serde_json::Value = serde_json::from_str(&out).expect("valid JSON");
    assert_eq!(v["dry_run"], true);
    assert_eq!(v["normalize"], false);
    assert_eq!(v["ids_assigned"][0]["title"], "No id at all");
    assert_eq!(v["dangling_deps"][0]["dep_id"], "M-404");
    assert_eq!(v["field_order"]["reordered"][0]["task"], "M-001");
    assert_eq!(
        v["field_order"]["reordered"][0]["now"],
        serde_json::json!(["added", "dep", "note"])
    );
    assert_eq!(fs::read_to_string(&track).unwrap(), original);

    // Real run with the flag: same document shape, `normalize` now true, and
    // the file actually reordered.
    let out = run_fr_ok(tmp.path(), &["--json", "clean", "--normalize"]);
    let v: serde_json::Value = serde_json::from_str(&out).expect("valid JSON");
    assert_eq!(v["dry_run"], false);
    assert_eq!(v["normalize"], true);
    assert_eq!(v["field_order"]["reordered"][0]["task"], "M-001");

    let text = fs::read_to_string(&track).unwrap();
    assert!(
        text.find("dep:").unwrap() < text.find("note:").unwrap(),
        "{text}"
    );
}

/// A JSON run that cannot write must not print a document saying it did.
#[test]
fn clean_json_prints_nothing_when_the_project_will_not_load() {
    let tmp = tempfile::tempdir().unwrap();
    // No frame/ directory at all.
    let (stdout, _stderr, success) = run_fr(tmp.path(), &["--json", "clean"]);
    assert!(!success);
    assert!(
        stdout.trim().is_empty(),
        "a failed run emitted a result document: {stdout}"
    );
}

/// The human surface is unchanged by the JSON one existing — they are a pair,
/// and `--json` must not leak into the default output.
#[test]
fn clean_human_output_has_no_json() {
    let tmp = tempfile::tempdir().unwrap();
    create_test_project(tmp.path());
    let out = run_fr_ok(tmp.path(), &["clean"]);
    assert!(!out.trim_start().starts_with('{'), "{out}");
}

// ---------------------------------------------------------------------------
// `--json` on the write commands
// ---------------------------------------------------------------------------

/// A creator returns the task it made, in the shape `fr show --json` returns.
#[test]
fn write_json_creator_returns_the_new_task() {
    let tmp = tempfile::tempdir().unwrap();
    create_test_project(tmp.path());

    let out = run_fr_ok(tmp.path(), &["--json", "add", "main", "a new task"]);
    let v: serde_json::Value = serde_json::from_str(&out).expect("valid JSON");
    assert_eq!(v["command"], "add");
    assert_eq!(v["changed"], true);
    assert_eq!(v["track"], "main");
    assert_eq!(v["tasks"][0]["title"], "a new task");
    assert_eq!(v["tasks"][0]["state"], "todo");

    // The same task, looked up — the write surface must not invent a second
    // task shape alongside the read one.
    let id = v["tasks"][0]["id"].as_str().unwrap().to_string();
    let shown = run_fr_ok(tmp.path(), &["--json", "show", &id]);
    let shown: serde_json::Value = serde_json::from_str(&shown).unwrap();
    assert_eq!(v["tasks"][0], shown);
}

/// `changed` is not "did it succeed" — it is "does the project differ".
///
/// `fr tag T add x` on a task already tagged `x` succeeds and changes nothing,
/// and a caller deciding whether to commit needs those told apart. Computed by
/// comparing the task before and after, so it needs no per-op bookkeeping.
#[test]
fn write_json_changed_is_false_for_a_no_op() {
    let tmp = tempfile::tempdir().unwrap();
    create_test_project(tmp.path());

    let first = run_fr_ok(tmp.path(), &["--json", "tag", "M-001", "add", "fresh"]);
    let first: serde_json::Value = serde_json::from_str(&first).unwrap();
    assert_eq!(first["changed"], true);

    let again = run_fr_ok(tmp.path(), &["--json", "tag", "M-001", "add", "fresh"]);
    let again: serde_json::Value = serde_json::from_str(&again).unwrap();
    assert_eq!(again["changed"], false, "re-adding a tag changed nothing");
    // Still reports the task, so a caller sees the state either way.
    assert_eq!(again["tasks"][0]["id"], "M-001");
}

/// `delete` reports the tasks as they were — after it runs there is nothing
/// left to describe.
#[test]
fn write_json_delete_reports_the_pre_delete_snapshot() {
    let tmp = tempfile::tempdir().unwrap();
    create_test_project(tmp.path());

    let out = run_fr_ok(tmp.path(), &["--json", "delete", "M-001", "--yes"]);
    let v: serde_json::Value = serde_json::from_str(&out).expect("valid JSON");
    assert_eq!(v["command"], "delete");
    assert_eq!(v["tasks"][0]["id"], "M-001");
    assert!(
        v["tasks"][0]["title"]
            .as_str()
            .is_some_and(|t| !t.is_empty()),
        "the snapshot should carry the task's content: {v}"
    );

    let (_, _, still_there) = run_fr(tmp.path(), &["show", "M-001"]);
    assert!(!still_there, "the task should be gone");
}

/// **`--json` must never hang on a prompt, and never auto-confirm one.**
///
/// `--json` says the caller is a program: a confirmation blocks on a stdin that
/// will never answer, and confirming for it would let the flag silently escalate
/// a destructive command. All three prompting commands fail fast instead —
/// `check --fix` included, which already had a JSON surface and so already had
/// this defect.
#[test]
fn write_json_refuses_to_prompt() {
    let tmp = tempfile::tempdir().unwrap();
    create_test_project(tmp.path());

    let (stdout, stderr, ok) = run_fr(tmp.path(), &["--json", "delete", "M-001"]);
    assert!(!ok, "should have failed rather than prompted");
    assert!(stderr.contains("--yes"), "{stderr}");
    assert!(
        stdout.trim().is_empty(),
        "no document on a refusal: {stdout}"
    );

    // And the task is still there — a refusal must not half-do the job.
    let (_, _, still_there) = run_fr(tmp.path(), &["show", "M-001"]);
    assert!(still_there);
}

/// A track write reports the track in the shape `fr tracks --json` lists it.
#[test]
fn write_json_track_reports_the_track() {
    let tmp = tempfile::tempdir().unwrap();
    create_test_project(tmp.path());

    let out = run_fr_ok(tmp.path(), &["--json", "track", "shelve", "side"]);
    let v: serde_json::Value = serde_json::from_str(&out).expect("valid JSON");
    assert_eq!(v["command"], "shelve");
    assert_eq!(v["track"]["id"], "side");
    assert_eq!(v["track"]["state"], "shelved");
    assert!(v["track"]["stats"].is_object(), "{v}");
}

/// `import` creates many, and reports them all.
#[test]
fn write_json_import_reports_every_task() {
    let tmp = tempfile::tempdir().unwrap();
    create_test_project(tmp.path());
    fs::write(
        tmp.path().join("in.md"),
        "- [ ] first imported\n- [ ] second imported\n",
    )
    .unwrap();

    let out = run_fr_ok(
        tmp.path(),
        &["--json", "import", "in.md", "--track", "main"],
    );
    let v: serde_json::Value = serde_json::from_str(&out).expect("valid JSON");
    assert_eq!(v["command"], "import");
    assert_eq!(v["tasks"].as_array().unwrap().len(), 2);
}

/// The human surface is unchanged by the JSON one existing.
#[test]
fn write_human_output_is_still_human() {
    let tmp = tempfile::tempdir().unwrap();
    create_test_project(tmp.path());
    let out = run_fr_ok(tmp.path(), &["add", "main", "plain"]);
    assert!(!out.trim_start().starts_with('{'), "{out}");
    assert!(out.trim().starts_with("M-"), "{out}");
}

// ---------------------------------------------------------------------------
// `--dry-run` on `fr mv`
// ---------------------------------------------------------------------------
//
// The reason the flag exists: `fr mv` is the command whose result you cannot
// read off the invocation. A cross-track move, a promote and a reparent all
// re-mint the id — of the whole moved subtree — so "what will this task be
// called afterwards" is a question only frame can answer. A preview that named a
// different id than the real run would be worse than no preview at all.
//
// `tests/parity.rs` proves the writes are suppressed, for every command. These
// prove the *answer* is right.

/// The id a `--dry-run` names is the id the real run mints, in every form of the
/// move that re-mints one.
#[test]
fn mv_dry_run_names_the_id_the_real_run_mints() {
    for (what, argv) in [
        ("cross-track", &["mv", "M-001", "--track", "side"][..]),
        ("promote", &["mv", "M-003.1", "--promote"][..]),
        ("reparent", &["mv", "M-001", "--parent", "M-002"][..]),
    ] {
        let tmp = tempfile::tempdir().unwrap();
        create_test_project(tmp.path());

        let mut previewed: Vec<&str> = argv.to_vec();
        previewed.push("--dry-run");
        let preview = run_fr_ok(tmp.path(), &previewed);
        let real = run_fr_ok(tmp.path(), argv);

        // The first line is the handler's own report; the preview then adds its
        // trailer. Compare the reports.
        let preview_line = preview.lines().next().unwrap_or_default();
        let real_line = real.lines().next().unwrap_or_default();
        assert_eq!(
            preview_line, real_line,
            "{what}: preview said {preview_line:?}, the real run did {real_line:?}"
        );
        assert!(
            preview.contains("dry run — nothing was written"),
            "{what}: no trailer in:\n{preview}"
        );
    }
}

/// A same-track reorder previews too, and touches only its own track file.
#[test]
fn mv_dry_run_reorder_reports_only_the_track_it_would_rewrite() {
    let tmp = tempfile::tempdir().unwrap();
    create_test_project(tmp.path());

    let out = run_fr_ok(tmp.path(), &["--json", "mv", "M-003", "--top", "--dry-run"]);
    let v: serde_json::Value = serde_json::from_str(&out).expect("valid JSON");
    assert_eq!(v["command"], "mv");
    assert_eq!(v["dry_run"], true);

    let would: Vec<&str> = v["would_write"]
        .as_array()
        .expect("would_write")
        .iter()
        .map(|p| p.as_str().unwrap())
        .collect();
    assert_eq!(
        would,
        vec!["frame/tracks/main.md"],
        "a reorder rewrites one file and mints nothing"
    );
}

/// A cross-track move reports both track files and the id frontier, because it
/// writes all three — and the frontier is the one a `--dry-run` used to advance.
#[test]
fn mv_dry_run_across_tracks_reports_every_file_it_would_write() {
    let tmp = tempfile::tempdir().unwrap();
    create_test_project(tmp.path());

    let out = run_fr_ok(
        tmp.path(),
        &["--json", "mv", "M-001", "--track", "side", "--dry-run"],
    );
    let v: serde_json::Value = serde_json::from_str(&out).expect("valid JSON");
    let would: Vec<&str> = v["would_write"]
        .as_array()
        .expect("would_write")
        .iter()
        .map(|p| p.as_str().unwrap())
        .collect();

    for expected in ["frame/tracks/main.md", "frame/tracks/side.md"] {
        assert!(
            would.contains(&expected),
            "{expected} missing from {would:?}"
        );
    }
}

/// A real run says it is not a preview, so a consumer never has to infer it from
/// the field being absent.
#[test]
fn a_real_run_reports_dry_run_false() {
    let tmp = tempfile::tempdir().unwrap();
    create_test_project(tmp.path());

    let out = run_fr_ok(tmp.path(), &["--json", "add", "main", "real"]);
    let v: serde_json::Value = serde_json::from_str(&out).expect("valid JSON");
    assert_eq!(v["dry_run"], false);
    assert!(v.get("would_write").is_none(), "empty on a real run");
}

// ---------------------------------------------------------------------------
// `fr mv --track` in both index directions
// ---------------------------------------------------------------------------
//
// A cross-track move needs two mutable references into one `Vec<(String,
// Track)>`, so the handler splits it. Reading the split's two halves as
// positional — "left is the earlier track" — rather than as what they are named
// (source, target) is what broke this: a second swap reversed them whenever the
// source sat after the target, so `move_task_to_track` searched the destination
// for the task and reported `task not found` on an id `fr show` resolves.
//
// The failure was therefore a function of *config order*, not of the tracks
// involved: on a project of n tracks, every move to a later track worked and
// every move to an earlier one failed. Reordering `project.toml` moved the
// failure with it. These pin both directions, because a test that only moves
// forward passes against the bug.

/// Both directions of a cross-track move work, and land the task where asked.
#[test]
fn mv_across_tracks_works_in_both_index_directions() {
    // `main` is the first track in the fixture's config, `side` the second.
    for (what, task, from, to) in [
        ("forward: main (#0) → side (#1)", "M-001", "main", "side"),
        ("backward: side (#1) → main (#0)", "S-001", "side", "main"),
    ] {
        let tmp = tempfile::tempdir().unwrap();
        create_test_project(tmp.path());

        let (stdout, stderr, ok) = run_fr(tmp.path(), &["mv", task, "--track", to]);
        assert!(ok, "{what}: {stderr}");

        // The task left the source and arrived in the destination, under the
        // destination's prefix.
        let new_id = stdout
            .split_whitespace()
            .nth(2)
            .unwrap_or_default()
            .to_string();
        let landed = run_fr_ok(tmp.path(), &["list", to]);
        assert!(landed.contains(&new_id), "{what}: not in {to}:\n{landed}");

        let left = run_fr_ok(tmp.path(), &["list", from]);
        assert!(!left.contains(task), "{what}: still in {from}:\n{left}");
    }
}

/// The reporter's shape: on a multi-track project, every ordered pair moves.
///
/// A triangular result — forward fine, backward failing — is the signature of
/// the split being read positionally, so the matrix is the assertion.
#[test]
fn mv_across_tracks_works_for_every_ordered_pair() {
    let names = ["alpha", "beta", "gamma", "delta"];

    for (i, from) in names.iter().enumerate() {
        for (j, to) in names.iter().enumerate() {
            if i == j {
                continue;
            }
            let tmp = tempfile::tempdir().unwrap();
            let mut init = vec!["init", "--name", "matrix"];
            for n in &names {
                init.push("--track");
                init.push(n);
                init.push(n);
            }
            let (_, stderr, ok) = run_fr(tmp.path(), &init);
            assert!(ok, "init failed: {stderr}");

            let (added, stderr, ok) = run_fr(tmp.path(), &["add", from, "the task"]);
            assert!(ok, "add failed: {stderr}");
            let id = added.trim().to_string();

            let (_, stderr, ok) = run_fr(tmp.path(), &["mv", &id, "--track", to]);
            assert!(
                ok,
                "moving {id} from {from} (#{i}) to {to} (#{j}) failed: {stderr}"
            );

            let landed = run_fr_ok(tmp.path(), &["list", to]);
            assert!(
                landed.contains("the task"),
                "{id}: {from} (#{i}) → {to} (#{j}) reported success but did not land:\n{landed}"
            );
        }
    }
}

/// `--track` naming the track the task is already in is not a cross-track move.
///
/// It used to reach the two-reference split with one index for both halves,
/// indexing past the end of one of them: `index out of bounds: the len is 1 but
/// the index is 1`, a panic rather than an error.
#[test]
fn mv_to_the_same_track_is_a_no_op_not_a_panic() {
    let tmp = tempfile::tempdir().unwrap();
    create_test_project(tmp.path());

    let before = fs::read_to_string(tmp.path().join("frame/tracks/main.md")).unwrap();
    let (stdout, stderr, ok) = run_fr(tmp.path(), &["mv", "M-001", "--track", "main"]);
    assert!(ok, "{stderr}");
    assert!(stdout.contains("already in main"), "{stdout}");

    // No re-mint: the id, and the file, are untouched.
    let after = fs::read_to_string(tmp.path().join("frame/tracks/main.md")).unwrap();
    assert_eq!(before, after, "a no-op move rewrote the track");
}

/// The same, under `--json`: a request already satisfied reports `changed: false`.
#[test]
fn mv_to_the_same_track_reports_changed_false() {
    let tmp = tempfile::tempdir().unwrap();
    create_test_project(tmp.path());

    let out = run_fr_ok(tmp.path(), &["--json", "mv", "M-001", "--track", "main"]);
    let v: serde_json::Value = serde_json::from_str(&out).expect("valid JSON");
    assert_eq!(v["command"], "mv");
    assert_eq!(v["changed"], false);
    assert_eq!(v["tasks"][0]["id"], "M-001", "still under its original id");
}

/// With a placement flag, the same-track form is a reorder — and still no re-mint.
#[test]
fn mv_to_the_same_track_with_a_placement_flag_reorders() {
    let tmp = tempfile::tempdir().unwrap();
    create_test_project(tmp.path());

    let (_, stderr, ok) = run_fr(tmp.path(), &["mv", "M-003", "--track", "main", "--top"]);
    assert!(ok, "{stderr}");

    let listed = run_fr_ok(tmp.path(), &["list", "main"]);
    let first = listed
        .lines()
        .find(|l| l.contains("M-00"))
        .unwrap_or_default();
    assert!(first.contains("M-003"), "M-003 is not first:\n{listed}");
}

/// A `dep:` pointing at a task moved backward is rewritten, as it is forward.
///
/// The reversed move failed before it could renumber anything, so the deps half
/// of a cross-track move was never exercised in that direction either.
#[test]
fn mv_backward_across_tracks_rewrites_deps_pointing_at_it() {
    let tmp = tempfile::tempdir().unwrap();
    create_test_project(tmp.path());

    // M-002 already depends on M-001. Move S-001 backward and point M-001 at it,
    // so a dep crosses the move in the failing direction.
    run_fr_ok(tmp.path(), &["dep", "M-001", "add", "S-001"]);
    let (stdout, stderr, ok) = run_fr(tmp.path(), &["mv", "S-001", "--track", "main"]);
    assert!(ok, "{stderr}");
    let new_id = stdout
        .split_whitespace()
        .nth(2)
        .unwrap_or_default()
        .to_string();

    let shown = run_fr_ok(tmp.path(), &["show", "M-001"]);
    assert!(
        shown.contains(&new_id),
        "dep still points at the retired id:\n{shown}"
    );
    assert!(!shown.contains("S-001"), "stale dep survived:\n{shown}");
}

// ---------------------------------------------------------------------------
// `-C` / `--project-dir` resolution
// ---------------------------------------------------------------------------
//
// `-C` names a project. It used to name a place to *start looking* for one, so
// a path inside a project resolved upward to that project — silently, reporting
// success, with nothing separating "operated on the sandbox" from "operated on
// the live tracks".

/// A `-C` at a directory inside a project is an error, not that project.
#[test]
fn project_dir_at_a_nested_directory_is_an_error() {
    let tmp = tempfile::tempdir().unwrap();
    create_test_project(tmp.path());
    let nested = tmp.path().join("sandbox");
    fs::create_dir(&nested).unwrap();

    let (_, stderr, ok) = run_fr(tmp.path(), &["tracks", "-C", "./sandbox"]);
    assert!(!ok, "-C at a nested directory resolved upward");
    assert!(
        stderr.contains("not a frame project"),
        "unexpected error:\n{stderr}"
    );
    // The enclosing project is what the caller may have meant, so it is named.
    assert!(
        stderr.contains("an enclosing project exists at"),
        "the refusal does not name the enclosing project:\n{stderr}"
    );
}

/// And it writes nothing: the write half resolved upward too.
#[test]
fn project_dir_at_a_nested_directory_writes_nothing() {
    let tmp = tempfile::tempdir().unwrap();
    create_test_project(tmp.path());
    let nested = tmp.path().join("sandbox");
    fs::create_dir(&nested).unwrap();
    let track = tmp.path().join("frame/tracks/main.md");
    let before = fs::read_to_string(&track).unwrap();

    let (_, _, ok) = run_fr(
        tmp.path(),
        &["add", "main", "leaked task", "-C", "./sandbox"],
    );
    assert!(!ok, "a write through a nested -C succeeded");

    let after = fs::read_to_string(&track).unwrap();
    assert_eq!(before, after, "the write landed in the enclosing project");
    assert!(
        !nested.join("frame").exists(),
        "a project appeared at the -C path"
    );
}

/// A `-C` at a project root still reads and writes that project, from anywhere.
#[test]
fn project_dir_at_a_project_root_reads_and_writes_there() {
    let base = tempfile::tempdir().unwrap();
    let proj = base.path().join("proj");
    let elsewhere = base.path().join("elsewhere");
    fs::create_dir_all(&proj).unwrap();
    fs::create_dir_all(&elsewhere).unwrap();
    create_test_project(&proj);
    let proj_arg = proj.to_str().unwrap();

    let listed = run_fr_ok(&elsewhere, &["list", "-C", proj_arg]);
    assert!(listed.contains("M-001"), "did not read the named project");

    let (_, stderr, ok) = run_fr(&elsewhere, &["add", "main", "landed here", "-C", proj_arg]);
    assert!(ok, "{stderr}");
    let track = fs::read_to_string(proj.join("frame/tracks/main.md")).unwrap();
    assert!(track.contains("landed here"), "the write went elsewhere");

    // The preview names files relative to the project it is about, not to a
    // working directory that has nothing to do with it.
    let preview = run_fr_ok(
        &elsewhere,
        &["add", "main", "previewed", "-C", proj_arg, "--dry-run"],
    );
    assert!(
        preview.contains("frame/tracks/main.md"),
        "dry-run paths are not relative to the named project:\n{preview}"
    );
}

/// Outside any project, `-C` errors as it always did — and has nothing to offer.
#[test]
fn project_dir_outside_any_project_errors_without_a_hint() {
    let base = tempfile::tempdir().unwrap();
    let empty = base.path().join("empty");
    fs::create_dir_all(&empty).unwrap();

    let (_, stderr, ok) = run_fr(base.path(), &["tracks", "-C", empty.to_str().unwrap()]);
    assert!(!ok, "-C outside a project succeeded");
    assert!(
        stderr.contains("not a frame project"),
        "unexpected error:\n{stderr}"
    );
    assert!(
        !stderr.contains("an enclosing project"),
        "named an enclosing project that does not exist:\n{stderr}"
    );
}

/// A `-C` at a path that does not exist is an error, not a project created there.
#[test]
fn project_dir_at_a_missing_path_errors() {
    let tmp = tempfile::tempdir().unwrap();
    create_test_project(tmp.path());

    let (_, stderr, ok) = run_fr(tmp.path(), &["tracks", "-C", "./no-such-dir"]);
    assert!(!ok, "-C at a missing path succeeded");
    assert!(
        stderr.contains("cannot resolve -C path"),
        "unexpected error:\n{stderr}"
    );
}

/// `fr init -C <path>` initializes the named directory, not the working one.
///
/// `init` runs before project discovery and so never saw the override at all:
/// it created the project in the working directory and said nothing.
#[test]
fn init_honors_project_dir() {
    let base = tempfile::tempdir().unwrap();
    let target = base.path().join("target");
    let elsewhere = base.path().join("elsewhere");
    fs::create_dir_all(&target).unwrap();
    fs::create_dir_all(&elsewhere).unwrap();

    let (_, stderr, ok) = run_fr(
        &elsewhere,
        &[
            "init",
            "--name",
            "sandboxed",
            "-C",
            target.to_str().unwrap(),
            "--track",
            "main",
            "Main",
        ],
    );
    assert!(ok, "{stderr}");
    assert!(
        target.join("frame/project.toml").exists(),
        "no project at the -C path"
    );
    assert!(
        !elsewhere.join("frame").exists(),
        "init created the project in the working directory"
    );
}

/// `fr track rename --prefix` renumbers the named project, not the caller's.
///
/// The gate validated the `-C` project and then the prefix branch re-discovered
/// from the working directory, so the bulk rewrite — every task ID in the track,
/// plus every `dep:` pointing at one — landed in whatever project the caller
/// happened to be standing in, while the lock was held on the one named.
#[test]
fn track_rename_prefix_honors_project_dir() {
    let base = tempfile::tempdir().unwrap();
    let caller = base.path().join("caller");
    let named = base.path().join("named");
    fs::create_dir_all(&caller).unwrap();
    fs::create_dir_all(&named).unwrap();
    create_test_project(&caller);
    create_test_project(&named);

    let caller_before = fs::read_to_string(caller.join("frame/tracks/main.md")).unwrap();

    let (_, stderr, ok) = run_fr(
        &caller,
        &[
            "track",
            "rename",
            "main",
            "--prefix",
            "ZZZ",
            "-C",
            named.to_str().unwrap(),
            "-y",
        ],
    );
    assert!(ok, "{stderr}");

    let renamed = fs::read_to_string(named.join("frame/tracks/main.md")).unwrap();
    assert!(
        renamed.contains("ZZZ-001"),
        "the named project was not renamed:\n{renamed}"
    );
    let caller_after = fs::read_to_string(caller.join("frame/tracks/main.md")).unwrap();
    assert_eq!(
        caller_before, caller_after,
        "the rename rewrote the caller's project"
    );
}
