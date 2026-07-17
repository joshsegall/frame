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

#[test]
fn test_ref() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());

    run_fr_ok(tmp.path(), &["ref", "M-001", "doc/design.md"]);
    let track = fs::read_to_string(tmp.path().join("frame/tracks/main.md")).unwrap();
    assert!(track.contains("ref: doc/design.md"));
}

#[test]
fn test_spec() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());

    run_fr_ok(tmp.path(), &["spec", "M-001", "doc/spec.md#section"]);
    let track = fs::read_to_string(tmp.path().join("frame/tracks/main.md")).unwrap();
    assert!(track.contains("spec: doc/spec.md#section"));
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

#[test]
fn test_init_gitignore_added() {
    let tmp = tempfile::TempDir::new().unwrap();

    // Create a git repo so .gitignore logic triggers
    fs::create_dir(tmp.path().join(".git")).unwrap();

    let out = run_fr_ok(tmp.path(), &["init", "--name", "Git Project"]);
    assert!(out.contains("added frame/.state.json, frame/.lock, frame/.actor to .gitignore"));

    let gitignore = fs::read_to_string(tmp.path().join(".gitignore")).unwrap();
    assert!(gitignore.contains("frame/.state.json"));
    assert!(gitignore.contains("frame/.lock"));
    assert!(gitignore.contains("frame/.actor"));
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

#[test]
fn test_init_gitignore_partial() {
    let tmp = tempfile::TempDir::new().unwrap();
    fs::create_dir(tmp.path().join(".git")).unwrap();
    fs::write(tmp.path().join(".gitignore"), "frame/.lock\n").unwrap();

    let out = run_fr_ok(tmp.path(), &["init", "--name", "Partial"]);
    // Should still add the missing entry
    assert!(out.contains("added frame/.state.json, frame/.lock, frame/.actor to .gitignore"));

    let gitignore = fs::read_to_string(tmp.path().join(".gitignore")).unwrap();
    assert!(gitignore.contains("frame/.state.json"));
    // Original entry should still be there
    assert!(gitignore.contains("frame/.lock"));
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
