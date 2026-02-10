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

    let out = run_fr_ok(tmp.path(), &["ready", "--cc"]);
    // cc-focus is "main", and only cc-tagged ready tasks
    // M-001 is ready but not cc-tagged → excluded
    // M-002 is cc-tagged but active (not todo) → excluded
    // M-003 is todo, no cc tag → excluded
    // So nothing should be ready with --cc in our test data (M-002 has cc but is active)
    assert!(!out.contains("M-001"));
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

    // ACTIVE.md should be generated
    assert!(tmp.path().join("frame/ACTIVE.md").exists());
}

#[test]
fn test_clean_dry_run() {
    let tmp = tempfile::TempDir::new().unwrap();
    create_test_project(tmp.path());

    let out = run_fr_ok(tmp.path(), &["clean", "--dry-run"]);
    assert!(out.contains("dry run"));

    // ACTIVE.md should NOT be generated in dry-run
    assert!(!tmp.path().join("frame/ACTIVE.md").exists());
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
    assert!(out.contains("added frame/.state.json, frame/.lock to .gitignore"));

    let gitignore = fs::read_to_string(tmp.path().join(".gitignore")).unwrap();
    assert!(gitignore.contains("frame/.state.json"));
    assert!(gitignore.contains("frame/.lock"));
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
        "frame/.state.json\nframe/.lock\n",
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
    assert!(out.contains("added frame/.state.json, frame/.lock to .gitignore"));

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
