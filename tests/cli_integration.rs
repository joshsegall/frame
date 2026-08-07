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

    // Verify cc_focus is gone from config
    let config_text = fs::read_to_string(tmp.path().join("frame/project.toml")).unwrap();
    assert!(!config_text.contains("cc_focus"));

    // fr ready --cc should still work (no error)
    let _out = run_fr_ok(tmp.path(), &["ready", "--cc"]);
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
        actors.contains("[actors.a]"),
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
    assert!(
        registry.contains(&format!("[actors.{token}]")),
        "{registry}"
    );

    // A second mint does not re-announce (token already claimed).
    let (_stdout2, stderr2, success2) = run_fr(tmp.path(), &["add", "main", "Second"]);
    assert!(success2);
    assert!(
        !stderr2.contains("Claimed actor token"),
        "stderr2: {stderr2}"
    );
}

#[test]
fn test_dry_run_clean_on_unclaimed_clone_mints_nothing() {
    // Strict null policy: a passive path (`fr clean --dry-run`) on an unclaimed
    // clone must neither claim a token nor mint a null ID for an ID-less task.
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());
    fs::remove_file(tmp.path().join("frame/.actor")).unwrap();

    // Give the track an ID-less task.
    let main_path = tmp.path().join("frame/tracks/main.md");
    let main = fs::read_to_string(&main_path).unwrap();
    fs::write(
        &main_path,
        main.replace("## Backlog\n", "## Backlog\n\n- [ ] Task with no id\n"),
    )
    .unwrap();

    let (stdout, _stderr, success) = run_fr(tmp.path(), &["clean", "--dry-run"]);
    assert!(success);
    // Nothing was assigned, and no claim happened.
    assert!(
        !stdout.contains("IDs assigned"),
        "unclaimed clone must not mint on a dry run: {stdout}"
    );
    assert!(!tmp.path().join("frame/.actor").exists());
    assert!(!tmp.path().join("frame/actors.toml").exists());
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
