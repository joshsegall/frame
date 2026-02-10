use std::collections::HashSet;
use std::path::Path;

use serde::Serialize;

use crate::model::project::Project;
use crate::model::task::{Metadata, Task, TaskState};
use crate::model::track::{Track, TrackNode};

/// Structured result from `fr check`, suitable for --json output.
#[derive(Debug, Default, Serialize)]
pub struct CheckResult {
    pub valid: bool,
    pub errors: Vec<CheckError>,
    pub warnings: Vec<CheckWarning>,
}

/// A validation error (something that should be fixed).
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type")]
pub enum CheckError {
    /// A dep references a task ID that doesn't exist anywhere
    #[serde(rename = "dangling_dep")]
    DanglingDep {
        track_id: String,
        task_id: String,
        dep_id: String,
    },
    /// A `ref:` path doesn't exist on disk
    #[serde(rename = "broken_ref")]
    BrokenRef {
        track_id: String,
        task_id: String,
        path: String,
    },
    /// A `spec:` path doesn't exist on disk
    #[serde(rename = "broken_spec")]
    BrokenSpec {
        track_id: String,
        task_id: String,
        path: String,
    },
    /// Duplicate task ID found
    #[serde(rename = "duplicate_id")]
    DuplicateId {
        task_id: String,
        track_ids: Vec<String>,
    },
}

/// A validation warning (non-critical issue).
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type")]
pub enum CheckWarning {
    /// Task has no ID assigned
    #[serde(rename = "missing_id")]
    MissingId { track_id: String, title: String },
    /// Task has no `added:` date
    #[serde(rename = "missing_added_date")]
    MissingAddedDate { track_id: String, task_id: String },
    /// Done task has no `resolved:` date
    #[serde(rename = "missing_resolved_date")]
    MissingResolvedDate { track_id: String, task_id: String },
    /// Done task is in the backlog section (should probably be in Done)
    #[serde(rename = "done_in_backlog")]
    DoneInBacklog { track_id: String, task_id: String },
}

// ---------------------------------------------------------------------------
// Main check entry point
// ---------------------------------------------------------------------------

/// Validate a project and return structured results.
///
/// This is a read-only operation — it does not modify the project.
///
/// Checks performed:
/// 1. All `dep:` references resolve to existing task IDs
/// 2. All `ref:` paths exist on disk
/// 3. All `spec:` paths exist on disk (section fragment stripped)
/// 4. No duplicate task IDs
/// 5. Warnings for missing IDs, dates, misplaced tasks
pub fn check_project(project: &Project) -> CheckResult {
    let mut result = CheckResult::default();

    // Collect all task IDs for dep validation and duplicate detection
    let all_ids = collect_all_task_ids(project);
    let duplicates = find_duplicate_ids(project);

    for (task_id, track_ids) in &duplicates {
        result.errors.push(CheckError::DuplicateId {
            task_id: task_id.clone(),
            track_ids: track_ids.clone(),
        });
    }

    for (track_id, track) in &project.tracks {
        check_track(track, track_id, &all_ids, &project.root, &mut result);
    }

    result.valid = result.errors.is_empty();
    result
}

// ---------------------------------------------------------------------------
// Per-track validation
// ---------------------------------------------------------------------------

fn check_track(
    track: &Track,
    track_id: &str,
    all_ids: &HashSet<String>,
    project_root: &Path,
    result: &mut CheckResult,
) {
    for node in &track.nodes {
        if let TrackNode::Section { kind, tasks, .. } = node {
            for task in tasks {
                check_task(task, track_id, *kind, all_ids, project_root, result);
            }
        }
    }
}

fn check_task(
    task: &Task,
    track_id: &str,
    section: crate::model::track::SectionKind,
    all_ids: &HashSet<String>,
    project_root: &Path,
    result: &mut CheckResult,
) {
    let task_id = task.id.as_deref().unwrap_or("");

    // Warning: missing ID
    if task.id.is_none() {
        result.warnings.push(CheckWarning::MissingId {
            track_id: track_id.to_string(),
            title: task.title.clone(),
        });
    }

    // Warning: missing added date
    let has_added = task
        .metadata
        .iter()
        .any(|m| matches!(m, Metadata::Added(_)));
    if !has_added && task.id.is_some() {
        result.warnings.push(CheckWarning::MissingAddedDate {
            track_id: track_id.to_string(),
            task_id: task_id.to_string(),
        });
    }

    // Warning: done task missing resolved date
    if task.state == TaskState::Done {
        let has_resolved = task
            .metadata
            .iter()
            .any(|m| matches!(m, Metadata::Resolved(_)));
        if !has_resolved {
            result.warnings.push(CheckWarning::MissingResolvedDate {
                track_id: track_id.to_string(),
                task_id: task_id.to_string(),
            });
        }
    }

    // Warning: done task sitting in backlog
    if task.state == TaskState::Done && section == crate::model::track::SectionKind::Backlog {
        result.warnings.push(CheckWarning::DoneInBacklog {
            track_id: track_id.to_string(),
            task_id: task_id.to_string(),
        });
    }

    // Check metadata
    for meta in &task.metadata {
        match meta {
            Metadata::Dep(deps) => {
                for dep_id in deps {
                    if !all_ids.contains(dep_id) {
                        result.errors.push(CheckError::DanglingDep {
                            track_id: track_id.to_string(),
                            task_id: task_id.to_string(),
                            dep_id: dep_id.clone(),
                        });
                    }
                }
            }
            Metadata::Ref(refs) => {
                for r in refs {
                    if !project_root.join(r).exists() {
                        result.errors.push(CheckError::BrokenRef {
                            track_id: track_id.to_string(),
                            task_id: task_id.to_string(),
                            path: r.clone(),
                        });
                    }
                }
            }
            Metadata::Spec(spec) => {
                let file_path = spec.split('#').next().unwrap_or(spec);
                if !project_root.join(file_path).exists() {
                    result.errors.push(CheckError::BrokenSpec {
                        track_id: track_id.to_string(),
                        task_id: task_id.to_string(),
                        path: spec.clone(),
                    });
                }
            }
            _ => {}
        }
    }

    // Recurse into subtasks
    for sub in &task.subtasks {
        check_task(sub, track_id, section, all_ids, project_root, result);
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn collect_all_task_ids(project: &Project) -> HashSet<String> {
    let mut ids = HashSet::new();
    for (_, track) in &project.tracks {
        for node in &track.nodes {
            if let TrackNode::Section { tasks, .. } = node {
                collect_ids_from_tasks(tasks, &mut ids);
            }
        }
    }
    ids
}

fn collect_ids_from_tasks(tasks: &[Task], ids: &mut HashSet<String>) {
    for task in tasks {
        if let Some(ref id) = task.id {
            ids.insert(id.clone());
        }
        collect_ids_from_tasks(&task.subtasks, ids);
    }
}

/// Find task IDs that appear more than once (within or across tracks).
fn find_duplicate_ids(project: &Project) -> Vec<(String, Vec<String>)> {
    use std::collections::HashMap;
    // id → list of track_ids where it appears (including repeats for within-track dups)
    let mut id_to_tracks: HashMap<String, Vec<String>> = HashMap::new();

    for (track_id, track) in &project.tracks {
        for node in &track.nodes {
            if let TrackNode::Section { tasks, .. } = node {
                collect_id_locations(tasks, track_id, &mut id_to_tracks);
            }
        }
    }

    id_to_tracks
        .into_iter()
        .filter(|(_, tracks)| tracks.len() > 1)
        .collect()
}

fn collect_id_locations(
    tasks: &[Task],
    track_id: &str,
    id_to_tracks: &mut std::collections::HashMap<String, Vec<String>>,
) {
    for task in tasks {
        if let Some(ref id) = task.id {
            id_to_tracks
                .entry(id.clone())
                .or_default()
                .push(track_id.to_string());
        }
        collect_id_locations(&task.subtasks, track_id, id_to_tracks);
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::config::{
        AgentConfig, CleanConfig, IdConfig, ProjectConfig, ProjectInfo, TrackConfig, UiConfig,
    };
    use crate::parse::parse_track;
    use indexmap::IndexMap;
    use tempfile::TempDir;

    fn make_config() -> ProjectConfig {
        ProjectConfig {
            project: ProjectInfo {
                name: "test".to_string(),
            },
            agent: AgentConfig::default(),
            tracks: vec![TrackConfig {
                id: "main".to_string(),
                name: "Main".to_string(),
                state: "active".to_string(),
                file: "tracks/main.md".to_string(),
            }],
            clean: CleanConfig::default(),
            ids: IdConfig {
                prefixes: IndexMap::new(),
            },
            ui: UiConfig::default(),
        }
    }

    fn make_project_at(root: &Path, track_src: &str) -> Project {
        let track = parse_track(track_src);
        Project {
            root: root.to_path_buf(),
            frame_dir: root.join("frame"),
            config: make_config(),
            tracks: vec![("main".to_string(), track)],
            inbox: None,
        }
    }

    // --- Dangling deps ---

    #[test]
    fn test_check_dangling_dep() {
        let tmp = TempDir::new().unwrap();
        let project = make_project_at(
            tmp.path(),
            "\
# Main

## Backlog

- [ ] `M-001` Task one
  - added: 2025-05-01
  - dep: NONEXIST-999

## Done
",
        );

        let result = check_project(&project);
        assert!(!result.valid);
        assert_eq!(result.errors.len(), 1);
        assert!(matches!(
            &result.errors[0],
            CheckError::DanglingDep { dep_id, .. } if dep_id == "NONEXIST-999"
        ));
    }

    #[test]
    fn test_check_valid_dep() {
        let tmp = TempDir::new().unwrap();
        let project = make_project_at(
            tmp.path(),
            "\
# Main

## Backlog

- [ ] `M-001` Task one
  - added: 2025-05-01
  - dep: M-002
- [ ] `M-002` Task two
  - added: 2025-05-01

## Done
",
        );

        let result = check_project(&project);
        assert!(result.valid);
        let dangling: Vec<_> = result
            .errors
            .iter()
            .filter(|e| matches!(e, CheckError::DanglingDep { .. }))
            .collect();
        assert!(dangling.is_empty());
    }

    #[test]
    fn test_check_cross_track_dep() {
        let tmp = TempDir::new().unwrap();
        let track_a = parse_track(
            "\
# A

## Backlog

- [ ] `A-001` Task A
  - added: 2025-05-01
  - dep: B-001

## Done
",
        );
        let track_b = parse_track(
            "\
# B

## Backlog

- [ ] `B-001` Task B
  - added: 2025-05-01

## Done
",
        );
        let mut config = make_config();
        config.tracks = vec![
            TrackConfig {
                id: "a".to_string(),
                name: "A".to_string(),
                state: "active".to_string(),
                file: "a.md".to_string(),
            },
            TrackConfig {
                id: "b".to_string(),
                name: "B".to_string(),
                state: "active".to_string(),
                file: "b.md".to_string(),
            },
        ];

        let project = Project {
            root: tmp.path().to_path_buf(),
            frame_dir: tmp.path().join("frame"),
            config,
            tracks: vec![("a".to_string(), track_a), ("b".to_string(), track_b)],
            inbox: None,
        };

        let result = check_project(&project);
        assert!(result.valid);
    }

    // --- Broken refs ---

    #[test]
    fn test_check_broken_ref() {
        let tmp = TempDir::new().unwrap();
        let project = make_project_at(
            tmp.path(),
            "\
# Main

## Backlog

- [ ] `M-001` Task one
  - added: 2025-05-01
  - ref: nonexistent/file.md

## Done
",
        );

        let result = check_project(&project);
        assert!(!result.valid);
        assert!(matches!(
            &result.errors[0],
            CheckError::BrokenRef { path, .. } if path == "nonexistent/file.md"
        ));
    }

    #[test]
    fn test_check_valid_ref() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("doc.md"), "content").unwrap();

        let project = make_project_at(
            tmp.path(),
            "\
# Main

## Backlog

- [ ] `M-001` Task one
  - added: 2025-05-01
  - ref: doc.md

## Done
",
        );

        let result = check_project(&project);
        let broken: Vec<_> = result
            .errors
            .iter()
            .filter(|e| matches!(e, CheckError::BrokenRef { .. }))
            .collect();
        assert!(broken.is_empty());
    }

    // --- Broken spec ---

    #[test]
    fn test_check_broken_spec() {
        let tmp = TempDir::new().unwrap();
        let project = make_project_at(
            tmp.path(),
            "\
# Main

## Backlog

- [ ] `M-001` Task one
  - added: 2025-05-01
  - spec: missing/spec.md#section

## Done
",
        );

        let result = check_project(&project);
        assert!(!result.valid);
        assert!(matches!(
            &result.errors[0],
            CheckError::BrokenSpec { path, .. } if path == "missing/spec.md#section"
        ));
    }

    #[test]
    fn test_check_valid_spec_with_section() {
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join("doc")).unwrap();
        std::fs::write(tmp.path().join("doc/spec.md"), "# Section\ncontent").unwrap();

        let project = make_project_at(
            tmp.path(),
            "\
# Main

## Backlog

- [ ] `M-001` Task one
  - added: 2025-05-01
  - spec: doc/spec.md#section

## Done
",
        );

        let result = check_project(&project);
        let broken: Vec<_> = result
            .errors
            .iter()
            .filter(|e| matches!(e, CheckError::BrokenSpec { .. }))
            .collect();
        assert!(broken.is_empty());
    }

    // --- Duplicate IDs ---

    #[test]
    fn test_check_duplicate_ids() {
        let tmp = TempDir::new().unwrap();
        let track_a = parse_track(
            "\
# A

## Backlog

- [ ] `DUP-001` Task in A
  - added: 2025-05-01

## Done
",
        );
        let track_b = parse_track(
            "\
# B

## Backlog

- [ ] `DUP-001` Same ID in B
  - added: 2025-05-01

## Done
",
        );
        let mut config = make_config();
        config.tracks = vec![
            TrackConfig {
                id: "a".to_string(),
                name: "A".to_string(),
                state: "active".to_string(),
                file: "a.md".to_string(),
            },
            TrackConfig {
                id: "b".to_string(),
                name: "B".to_string(),
                state: "active".to_string(),
                file: "b.md".to_string(),
            },
        ];

        let project = Project {
            root: tmp.path().to_path_buf(),
            frame_dir: tmp.path().join("frame"),
            config,
            tracks: vec![("a".to_string(), track_a), ("b".to_string(), track_b)],
            inbox: None,
        };

        let result = check_project(&project);
        assert!(!result.valid);
        assert!(
            result.errors.iter().any(
                |e| matches!(e, CheckError::DuplicateId { task_id, .. } if task_id == "DUP-001")
            )
        );
    }

    #[test]
    fn test_check_duplicate_ids_within_track() {
        let tmp = TempDir::new().unwrap();
        let project = make_project_at(
            tmp.path(),
            "\
# Main

## Backlog

- [ ] `M-001` First occurrence
  - added: 2025-05-01
- [ ] `M-001` Same ID in same track
  - added: 2025-05-02

## Done
",
        );

        let result = check_project(&project);
        assert!(!result.valid);
        assert!(
            result.errors.iter().any(
                |e| matches!(e, CheckError::DuplicateId { task_id, track_ids } if task_id == "M-001" && track_ids.len() == 2)
            )
        );
    }

    // --- Warnings ---

    #[test]
    fn test_warn_missing_id() {
        let tmp = TempDir::new().unwrap();
        let project = make_project_at(
            tmp.path(),
            "\
# Main

## Backlog

- [ ] Task without ID

## Done
",
        );

        let result = check_project(&project);
        assert!(result.warnings.iter().any(|w| matches!(
            w,
            CheckWarning::MissingId { title, .. } if title == "Task without ID"
        )));
    }

    #[test]
    fn test_warn_missing_added_date() {
        let tmp = TempDir::new().unwrap();
        let project = make_project_at(
            tmp.path(),
            "\
# Main

## Backlog

- [ ] `M-001` Task without added date

## Done
",
        );

        let result = check_project(&project);
        assert!(result.warnings.iter().any(|w| matches!(
            w,
            CheckWarning::MissingAddedDate { task_id, .. } if task_id == "M-001"
        )));
    }

    #[test]
    fn test_warn_missing_resolved_date() {
        let tmp = TempDir::new().unwrap();
        let project = make_project_at(
            tmp.path(),
            "\
# Main

## Backlog

## Done

- [x] `M-001` Done task without resolved
  - added: 2025-05-01
",
        );

        let result = check_project(&project);
        assert!(result.warnings.iter().any(|w| matches!(
            w,
            CheckWarning::MissingResolvedDate { task_id, .. } if task_id == "M-001"
        )));
    }

    #[test]
    fn test_warn_done_in_backlog() {
        let tmp = TempDir::new().unwrap();
        let project = make_project_at(
            tmp.path(),
            "\
# Main

## Backlog

- [x] `M-001` Done task in backlog
  - added: 2025-05-01
  - resolved: 2025-05-10

## Done
",
        );

        let result = check_project(&project);
        assert!(result.warnings.iter().any(|w| matches!(
            w,
            CheckWarning::DoneInBacklog { task_id, .. } if task_id == "M-001"
        )));
    }

    // --- Clean project ---

    #[test]
    fn test_check_clean_project() {
        let tmp = TempDir::new().unwrap();
        let project = make_project_at(
            tmp.path(),
            "\
# Main

## Backlog

- [ ] `M-001` Well-formed task
  - added: 2025-05-01
- [>] `M-002` Another good task
  - added: 2025-05-02

## Done

- [x] `M-000` Completed task
  - added: 2025-04-01
  - resolved: 2025-05-01
",
        );

        let result = check_project(&project);
        assert!(result.valid);
        assert!(result.errors.is_empty());
        assert!(result.warnings.is_empty());
    }

    // --- Subtask checks ---

    #[test]
    fn test_check_subtask_dangling_dep() {
        let tmp = TempDir::new().unwrap();
        let project = make_project_at(
            tmp.path(),
            "\
# Main

## Backlog

- [ ] `M-001` Parent
  - added: 2025-05-01
  - [ ] `M-001.1` Sub with bad dep
    - added: 2025-05-01
    - dep: GONE-999

## Done
",
        );

        let result = check_project(&project);
        assert!(!result.valid);
        assert!(matches!(
            &result.errors[0],
            CheckError::DanglingDep { task_id, dep_id, .. }
                if task_id == "M-001.1" && dep_id == "GONE-999"
        ));
    }

    // --- JSON serialization ---

    #[test]
    fn test_check_result_serializes_to_json() {
        let tmp = TempDir::new().unwrap();
        let project = make_project_at(
            tmp.path(),
            "\
# Main

## Backlog

- [ ] `M-001` Task
  - added: 2025-05-01
  - dep: GONE-001

## Done
",
        );

        let result = check_project(&project);
        let json = serde_json::to_string_pretty(&result).unwrap();
        assert!(json.contains("dangling_dep"));
        assert!(json.contains("GONE-001"));
    }
}
