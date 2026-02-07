use frame::parse::{parse_inbox, parse_track, serialize_inbox, serialize_track};
use pretty_assertions::assert_eq;
use std::fs;
use std::path::Path;

/// Helper: load a fixture file, parse it, serialize it, and assert byte-for-byte equality
fn assert_track_round_trip(fixture_name: &str) {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(fixture_name);
    let source = fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("Could not read fixture {}: {}", fixture_name, e));

    let track = parse_track(&source);
    let output = serialize_track(&track);

    assert_eq!(
        output, source,
        "Round-trip failed for fixture: {}",
        fixture_name
    );
}

fn assert_inbox_round_trip(fixture_name: &str) {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(fixture_name);
    let source = fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("Could not read fixture {}: {}", fixture_name, e));

    let inbox = parse_inbox(&source);
    let output = serialize_inbox(&inbox);

    assert_eq!(
        output, source,
        "Round-trip failed for fixture: {}",
        fixture_name
    );
}

// ============================================================================
// Track round-trip tests
// ============================================================================

#[test]
fn round_trip_simple_track() {
    assert_track_round_trip("simple_track.md");
}

#[test]
fn round_trip_complex_track() {
    assert_track_round_trip("complex_track.md");
}

#[test]
fn round_trip_no_metadata() {
    assert_track_round_trip("no_metadata_track.md");
}

#[test]
fn round_trip_all_metadata() {
    assert_track_round_trip("all_metadata_track.md");
}

#[test]
fn round_trip_three_level_nesting() {
    assert_track_round_trip("three_level_nesting.md");
}

#[test]
fn round_trip_empty_sections() {
    assert_track_round_trip("empty_sections.md");
}

#[test]
fn round_trip_code_in_notes() {
    assert_track_round_trip("code_in_notes.md");
}

// ============================================================================
// Inbox round-trip tests
// ============================================================================

#[test]
fn round_trip_inbox() {
    assert_inbox_round_trip("inbox.md");
}

// ============================================================================
// Config round-trip test
// ============================================================================

#[test]
fn round_trip_config() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/project.toml");
    let source = fs::read_to_string(&path).unwrap();

    // Parse with toml crate
    let _config: frame::model::config::ProjectConfig = toml::from_str(&source).unwrap();

    // Parse with toml_edit and re-serialize
    let doc: toml_edit::DocumentMut = source.parse().unwrap();
    let output = doc.to_string();

    assert_eq!(output, source, "Config round-trip failed");
}

// ============================================================================
// Selective rewrite tests
// ============================================================================

/// The core property: modifying a subtask should ONLY change that subtask's
/// lines in the output. The parent, siblings, and all other tasks must remain
/// byte-for-byte identical to the original source.
#[test]
fn selective_rewrite_only_dirty_subtask_changes() {
    let source = "\
# Test Track

## Backlog

- [>] `T-001` Parent task
  - added: 2025-05-10
  - dep: T-000
  - [ ] `T-001.1` First subtask
  - [ ] `T-001.2` Second subtask
  - [ ] `T-001.3` Third subtask
- [ ] `T-002` Unrelated task
  - added: 2025-05-11

## Done";

    let mut track = parse_track(source);

    // Mutate only T-001.2: mark it done
    let backlog = track
        .section_tasks_mut(frame::model::track::SectionKind::Backlog)
        .unwrap();
    let subtask = &mut backlog[0].subtasks[1];
    assert_eq!(subtask.id.as_deref(), Some("T-001.2"));
    subtask.state = frame::model::TaskState::Done;
    subtask.dirty = true;
    subtask.source_text = None; // force canonical rewrite

    let output = serialize_track(&track);

    // The only difference should be `- [ ]` → `- [x]` on the T-001.2 line
    let expected = source.replace(
        "  - [ ] `T-001.2` Second subtask",
        "  - [x] `T-001.2` Second subtask",
    );
    assert_eq!(output, expected);
}

/// Verify that parent source_text does NOT include subtask lines.
/// This is the precondition for selective rewrite to work.
#[test]
fn parent_source_text_excludes_subtasks() {
    let source = "\
- [>] `T-001` Parent task
  - added: 2025-05-10
  - [ ] `T-001.1` First subtask
  - [ ] `T-001.2` Second subtask";

    let lines: Vec<String> = source.lines().map(|l| l.to_string()).collect();
    let (tasks, _) = frame::parse::task_parser::parse_tasks(&lines, 0, 0, 0);

    let parent = &tasks[0];
    let parent_source = parent.source_text.as_ref().unwrap();

    // Parent's source_text should be ONLY its own line + metadata
    assert_eq!(
        parent_source.len(),
        2,
        "Parent source_text should be 2 lines (task + added)"
    );
    assert_eq!(parent_source[0], "- [>] `T-001` Parent task");
    assert_eq!(parent_source[1], "  - added: 2025-05-10");

    // Subtasks should have their own source_text
    assert_eq!(tasks[0].subtasks.len(), 2);
    let sub1_source = tasks[0].subtasks[0].source_text.as_ref().unwrap();
    assert_eq!(sub1_source.len(), 1);
    assert_eq!(sub1_source[0], "  - [ ] `T-001.1` First subtask");
}

// ============================================================================
// Parse correctness tests
// ============================================================================

#[test]
fn complex_track_parse_correctness() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/complex_track.md");
    let source = fs::read_to_string(&path).unwrap();
    let track = parse_track(&source);

    assert_eq!(track.title, "Effect System");
    assert_eq!(
        track.description.as_deref(),
        Some("Design and implement the algebraic effect system for Lace.")
    );

    // Backlog
    let backlog = track.backlog();
    assert_eq!(backlog.len(), 6);

    // First task: EFF-014 with subtasks and complex note
    let eff014 = &backlog[0];
    assert_eq!(eff014.id.as_deref(), Some("EFF-014"));
    assert_eq!(eff014.state, frame::model::TaskState::Active);
    assert_eq!(eff014.tags, vec!["ready"]);
    assert_eq!(eff014.subtasks.len(), 3);

    // Check 3-level nesting
    let eff014_2 = &eff014.subtasks[1];
    assert_eq!(eff014_2.id.as_deref(), Some("EFF-014.2"));
    assert_eq!(eff014_2.subtasks.len(), 2);
    assert_eq!(eff014_2.subtasks[0].id.as_deref(), Some("EFF-014.2.1"));

    // Check multiple deps
    let eff012 = &backlog[2];
    assert_eq!(eff012.state, frame::model::TaskState::Blocked);
    if let frame::model::Metadata::Dep(deps) = &eff012.metadata[0] {
        assert_eq!(deps, &["EFF-014", "INFRA-003"]);
    } else {
        panic!("Expected Dep metadata on EFF-012");
    }

    // Parked
    let parked = track.parked();
    assert_eq!(parked.len(), 1);
    assert_eq!(parked[0].state, frame::model::TaskState::Parked);

    // Done
    let done = track.done();
    assert_eq!(done.len(), 3);
}

#[test]
fn code_in_notes_parse_correctness() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/code_in_notes.md");
    let source = fs::read_to_string(&path).unwrap();
    let track = parse_track(&source);

    let backlog = track.backlog();
    assert_eq!(
        backlog.len(),
        3,
        "Code blocks should not be parsed as tasks"
    );

    // CN-001: check that the code block content is preserved in the note
    let cn001 = &backlog[0];
    if let frame::model::Metadata::Note(note) = &cn001.metadata[0] {
        assert!(
            note.contains("- [ ] Not a task"),
            "Code block content should be preserved verbatim in note"
        );
        assert!(
            note.contains("- [x] Also not a task"),
            "Code block content should be preserved verbatim in note"
        );
    } else {
        panic!("Expected Note metadata on CN-001");
    }

    // CN-002: check multiple code blocks
    let cn002 = &backlog[1];
    if let frame::model::Metadata::Note(note) = &cn002.metadata[0] {
        assert!(note.contains("def foo()"));
        assert!(note.contains("plain code"));
        assert!(note.contains("End of note."));
    } else {
        panic!("Expected Note metadata on CN-002");
    }
}

#[test]
fn inbox_parse_correctness() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/inbox.md");
    let source = fs::read_to_string(&path).unwrap();
    let inbox = parse_inbox(&source);

    assert_eq!(inbox.items.len(), 5);

    // First item: bug with body
    assert_eq!(inbox.items[0].title, "Parser crashes on empty effect block");
    assert_eq!(inbox.items[0].tags, vec!["bug"]);
    assert!(inbox.items[0].body.is_some());

    // Second item: design with code in body
    assert_eq!(inbox.items[1].tags, vec!["design"]);
    let body = inbox.items[1].body.as_ref().unwrap();
    assert!(body.contains("```lace"));

    // Third item: multiple tags on continuation line
    assert_eq!(inbox.items[2].tags, vec!["cc-added", "bug"]);

    // Fourth: no body
    assert!(inbox.items[3].body.is_none());

    // Fifth: multiline body
    assert!(inbox.items[4].body.is_some());
}
