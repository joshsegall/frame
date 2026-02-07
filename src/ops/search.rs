use std::ops::Range;

use regex::Regex;

use crate::model::inbox::Inbox;
use crate::model::project::Project;
use crate::model::task::{Metadata, Task};
use crate::model::track::{Track, TrackNode};

/// Which field of a task or inbox item matched
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MatchField {
    Id,
    Title,
    Tag,
    Note,
    Dep,
    Ref,
    Spec,
    /// Inbox body text
    Body,
}

/// A search hit for a task field
#[derive(Debug, Clone)]
pub struct SearchHit {
    pub track_id: String,
    pub task_id: String,
    pub field: MatchField,
    pub spans: Vec<Range<usize>>,
}

/// A search hit for an inbox item
#[derive(Debug, Clone)]
pub struct InboxSearchHit {
    pub item_index: usize,
    pub field: MatchField,
    pub spans: Vec<Range<usize>>,
}

/// Collect all non-overlapping match byte-ranges for a regex in the given text.
fn find_matches(re: &Regex, text: &str) -> Vec<Range<usize>> {
    re.find_iter(text).map(|m| m.start()..m.end()).collect()
}

// ---------------------------------------------------------------------------
// Task search
// ---------------------------------------------------------------------------

/// Search tasks across the project.
///
/// If `track_filter` is `Some`, only that track is searched (regardless of its
/// state). If `None`, all **active** tracks are searched.
pub fn search_tasks(project: &Project, re: &Regex, track_filter: Option<&str>) -> Vec<SearchHit> {
    let mut hits = Vec::new();

    for (track_id, track) in &project.tracks {
        if let Some(filter) = track_filter {
            if track_id != filter {
                continue;
            }
        } else {
            // Default: only active tracks
            let is_active = project
                .config
                .tracks
                .iter()
                .any(|tc| tc.id == *track_id && tc.state == "active");
            if !is_active {
                continue;
            }
        }

        search_track(re, track, track_id, &mut hits);
    }

    hits
}

/// Search all tasks within a single track.
fn search_track(re: &Regex, track: &Track, track_id: &str, hits: &mut Vec<SearchHit>) {
    for node in &track.nodes {
        if let TrackNode::Section { tasks, .. } = node {
            for task in tasks {
                search_task(re, task, track_id, hits);
            }
        }
    }
}

/// Search a single task (and its subtasks recursively).
fn search_task(re: &Regex, task: &Task, track_id: &str, hits: &mut Vec<SearchHit>) {
    let task_id = task.id.as_deref().unwrap_or("");

    // ID
    if let Some(id) = &task.id {
        let spans = find_matches(re, id);
        if !spans.is_empty() {
            hits.push(SearchHit {
                track_id: track_id.to_string(),
                task_id: task_id.to_string(),
                field: MatchField::Id,
                spans,
            });
        }
    }

    // Title
    let spans = find_matches(re, &task.title);
    if !spans.is_empty() {
        hits.push(SearchHit {
            track_id: track_id.to_string(),
            task_id: task_id.to_string(),
            field: MatchField::Title,
            spans,
        });
    }

    // Tags
    for tag in &task.tags {
        let spans = find_matches(re, tag);
        if !spans.is_empty() {
            hits.push(SearchHit {
                track_id: track_id.to_string(),
                task_id: task_id.to_string(),
                field: MatchField::Tag,
                spans,
            });
        }
    }

    // Metadata fields
    for meta in &task.metadata {
        match meta {
            Metadata::Note(text) => {
                let spans = find_matches(re, text);
                if !spans.is_empty() {
                    hits.push(SearchHit {
                        track_id: track_id.to_string(),
                        task_id: task_id.to_string(),
                        field: MatchField::Note,
                        spans,
                    });
                }
            }
            Metadata::Dep(deps) => {
                for dep in deps {
                    let spans = find_matches(re, dep);
                    if !spans.is_empty() {
                        hits.push(SearchHit {
                            track_id: track_id.to_string(),
                            task_id: task_id.to_string(),
                            field: MatchField::Dep,
                            spans,
                        });
                    }
                }
            }
            Metadata::Ref(refs) => {
                for r in refs {
                    let spans = find_matches(re, r);
                    if !spans.is_empty() {
                        hits.push(SearchHit {
                            track_id: track_id.to_string(),
                            task_id: task_id.to_string(),
                            field: MatchField::Ref,
                            spans,
                        });
                    }
                }
            }
            Metadata::Spec(spec) => {
                let spans = find_matches(re, spec);
                if !spans.is_empty() {
                    hits.push(SearchHit {
                        track_id: track_id.to_string(),
                        task_id: task_id.to_string(),
                        field: MatchField::Spec,
                        spans,
                    });
                }
            }
            _ => {} // Added, Resolved not searched
        }
    }

    // Recurse into subtasks
    for subtask in &task.subtasks {
        search_task(re, subtask, track_id, hits);
    }
}

// ---------------------------------------------------------------------------
// Inbox search
// ---------------------------------------------------------------------------

/// Search inbox items by title, tags, and body text.
pub fn search_inbox(inbox: &Inbox, re: &Regex) -> Vec<InboxSearchHit> {
    let mut hits = Vec::new();

    for (index, item) in inbox.items.iter().enumerate() {
        // Title
        let spans = find_matches(re, &item.title);
        if !spans.is_empty() {
            hits.push(InboxSearchHit {
                item_index: index,
                field: MatchField::Title,
                spans,
            });
        }

        // Tags
        for tag in &item.tags {
            let spans = find_matches(re, tag);
            if !spans.is_empty() {
                hits.push(InboxSearchHit {
                    item_index: index,
                    field: MatchField::Tag,
                    spans,
                });
            }
        }

        // Body
        if let Some(body) = &item.body {
            let spans = find_matches(re, body);
            if !spans.is_empty() {
                hits.push(InboxSearchHit {
                    item_index: index,
                    field: MatchField::Body,
                    spans,
                });
            }
        }
    }

    hits
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
    use crate::model::project::Project;
    use crate::parse::{parse_inbox, parse_track};
    use std::path::PathBuf;

    fn sample_track_a() -> Track {
        parse_track(
            "\
# Effects

> Effect system track.

## Backlog

- [ ] `EFF-001` Implement algebraic effects #core
  - added: 2025-05-01
  - note: This is the foundation for the effect system.
  - dep: INFRA-001
  - ref: src/effects/mod.rs
  - spec: docs/effects.md#overview
- [>] `EFF-002` Add handler syntax #parser
  - added: 2025-05-02
- [ ] `EFF-003` Effect type inference
  - added: 2025-05-03
  - [ ] `EFF-003.1` Unification for effects
  - [ ] `EFF-003.2` Subsumption checking #types

## Parked

- [~] `EFF-010` Research continuations

## Done

- [x] `EFF-000` Bootstrap effect module
  - added: 2025-04-20
  - resolved: 2025-04-25
",
        )
    }

    fn sample_track_b() -> Track {
        parse_track(
            "\
# Infrastructure

> Compiler infrastructure.

## Backlog

- [ ] `INFRA-001` Set up build pipeline #ci
  - added: 2025-05-01
- [ ] `INFRA-002` Add logging framework
  - added: 2025-05-02
  - note: Use tracing crate for structured logging.

## Parked

## Done

",
        )
    }

    fn sample_config() -> ProjectConfig {
        ProjectConfig {
            project: ProjectInfo {
                name: "test".to_string(),
            },
            agent: AgentConfig::default(),
            tracks: vec![
                TrackConfig {
                    id: "effects".to_string(),
                    name: "Effects".to_string(),
                    state: "active".to_string(),
                    file: "effects.md".to_string(),
                },
                TrackConfig {
                    id: "infra".to_string(),
                    name: "Infrastructure".to_string(),
                    state: "active".to_string(),
                    file: "infra.md".to_string(),
                },
            ],
            clean: CleanConfig::default(),
            ids: IdConfig::default(),
            ui: UiConfig::default(),
        }
    }

    fn sample_project() -> Project {
        Project {
            root: PathBuf::from("/tmp/test"),
            frame_dir: PathBuf::from("/tmp/test/frame"),
            config: sample_config(),
            tracks: vec![
                ("effects".to_string(), sample_track_a()),
                ("infra".to_string(), sample_track_b()),
            ],
            inbox: None,
        }
    }

    fn sample_inbox() -> Inbox {
        parse_inbox(
            "\
# Inbox

- Think about error handling strategy #design
  More thoughts on error handling approach.
- Review parser performance #perf #parser
- Quick idea
",
        )
    }

    // --- Title search ---

    #[test]
    fn test_search_title_match() {
        let project = sample_project();
        let re = Regex::new("handler").unwrap();
        let hits = search_tasks(&project, &re, None);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].task_id, "EFF-002");
        assert_eq!(hits[0].field, MatchField::Title);
        assert_eq!(hits[0].track_id, "effects");
        assert_eq!(hits[0].spans, vec![4..11]); // "Add [handler] syntax"
    }

    #[test]
    fn test_search_title_multiple_tracks() {
        let project = sample_project();
        let re = Regex::new("(?i)add").unwrap();
        let hits = search_tasks(&project, &re, None);
        let title_hits: Vec<_> = hits
            .iter()
            .filter(|h| h.field == MatchField::Title)
            .collect();
        assert_eq!(title_hits.len(), 2); // EFF-002 "Add handler syntax" + INFRA-002 "Add logging framework"
    }

    // --- ID search ---

    #[test]
    fn test_search_id() {
        let project = sample_project();
        let re = Regex::new("EFF-003").unwrap();
        let hits = search_tasks(&project, &re, None);
        let id_hits: Vec<_> = hits.iter().filter(|h| h.field == MatchField::Id).collect();
        // EFF-003, EFF-003.1, EFF-003.2
        assert_eq!(id_hits.len(), 3);
    }

    // --- Tag search ---

    #[test]
    fn test_search_tag() {
        let project = sample_project();
        let re = Regex::new("parser").unwrap();
        let hits = search_tasks(&project, &re, None);
        let tag_hits: Vec<_> = hits.iter().filter(|h| h.field == MatchField::Tag).collect();
        assert_eq!(tag_hits.len(), 1);
        assert_eq!(tag_hits[0].task_id, "EFF-002");
    }

    // --- Note search ---

    #[test]
    fn test_search_note() {
        let project = sample_project();
        let re = Regex::new("foundation").unwrap();
        let hits = search_tasks(&project, &re, None);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].task_id, "EFF-001");
        assert_eq!(hits[0].field, MatchField::Note);
    }

    // --- Dep search ---

    #[test]
    fn test_search_dep() {
        let project = sample_project();
        let re = Regex::new("INFRA-001").unwrap();
        let hits = search_tasks(&project, &re, None);
        let dep_hits: Vec<_> = hits.iter().filter(|h| h.field == MatchField::Dep).collect();
        assert_eq!(dep_hits.len(), 1);
        assert_eq!(dep_hits[0].task_id, "EFF-001");
    }

    // --- Ref search ---

    #[test]
    fn test_search_ref() {
        let project = sample_project();
        let re = Regex::new("effects/mod\\.rs").unwrap();
        let hits = search_tasks(&project, &re, None);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].task_id, "EFF-001");
        assert_eq!(hits[0].field, MatchField::Ref);
    }

    // --- Spec search ---

    #[test]
    fn test_search_spec() {
        let project = sample_project();
        let re = Regex::new("effects\\.md").unwrap();
        let hits = search_tasks(&project, &re, None);
        let spec_hits: Vec<_> = hits
            .iter()
            .filter(|h| h.field == MatchField::Spec)
            .collect();
        assert_eq!(spec_hits.len(), 1);
        assert_eq!(spec_hits[0].task_id, "EFF-001");
    }

    // --- Subtask search ---

    #[test]
    fn test_search_subtask_independent() {
        let project = sample_project();
        let re = Regex::new("Subsumption").unwrap();
        let hits = search_tasks(&project, &re, None);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].task_id, "EFF-003.2");
        assert_eq!(hits[0].field, MatchField::Title);
    }

    #[test]
    fn test_search_subtask_tag() {
        let project = sample_project();
        let re = Regex::new("types").unwrap();
        let hits = search_tasks(&project, &re, None);
        let tag_hits: Vec<_> = hits.iter().filter(|h| h.field == MatchField::Tag).collect();
        assert_eq!(tag_hits.len(), 1);
        assert_eq!(tag_hits[0].task_id, "EFF-003.2");
    }

    // --- Track filter ---

    #[test]
    fn test_search_with_track_filter() {
        let project = sample_project();
        let re = Regex::new("(?i)add").unwrap();

        // Only effects track
        let hits = search_tasks(&project, &re, Some("effects"));
        let title_hits: Vec<_> = hits
            .iter()
            .filter(|h| h.field == MatchField::Title)
            .collect();
        assert_eq!(title_hits.len(), 1);
        assert_eq!(title_hits[0].task_id, "EFF-002");

        // Only infra track
        let hits = search_tasks(&project, &re, Some("infra"));
        let title_hits: Vec<_> = hits
            .iter()
            .filter(|h| h.field == MatchField::Title)
            .collect();
        assert_eq!(title_hits.len(), 1);
        assert_eq!(title_hits[0].task_id, "INFRA-002");
    }

    #[test]
    fn test_search_track_filter_ignores_state() {
        // Shelved track should be searched when explicitly filtered
        let mut project = sample_project();
        project.config.tracks[1].state = "shelved".to_string();

        let re = Regex::new("logging").unwrap();

        // Default: shelved track excluded
        let hits = search_tasks(&project, &re, None);
        assert!(hits.is_empty());

        // Explicit filter: shelved track included
        // "logging" matches title and note of INFRA-002
        let hits = search_tasks(&project, &re, Some("infra"));
        assert_eq!(hits.len(), 2);
        assert!(hits.iter().any(|h| h.field == MatchField::Title));
        assert!(hits.iter().any(|h| h.field == MatchField::Note));
    }

    // --- Active-only default ---

    #[test]
    fn test_search_default_skips_shelved() {
        let mut project = sample_project();
        project.config.tracks[0].state = "shelved".to_string();

        let re = Regex::new("handler").unwrap();
        let hits = search_tasks(&project, &re, None);
        assert!(hits.is_empty());
    }

    #[test]
    fn test_search_default_skips_archived() {
        let mut project = sample_project();
        project.config.tracks[0].state = "archived".to_string();

        let re = Regex::new("handler").unwrap();
        let hits = search_tasks(&project, &re, None);
        assert!(hits.is_empty());
    }

    // --- Multiple matches in one field ---

    #[test]
    fn test_search_multiple_spans() {
        let project = sample_project();
        let re = Regex::new("e").unwrap();
        let hits = search_tasks(&project, &re, Some("effects"));
        // "Implement algebraic effects" has multiple 'e's
        let eff001_title: Vec<_> = hits
            .iter()
            .filter(|h| h.task_id == "EFF-001" && h.field == MatchField::Title)
            .collect();
        assert_eq!(eff001_title.len(), 1);
        assert!(eff001_title[0].spans.len() > 1);
    }

    // --- No matches ---

    #[test]
    fn test_search_no_matches() {
        let project = sample_project();
        let re = Regex::new("zzzznotfound").unwrap();
        let hits = search_tasks(&project, &re, None);
        assert!(hits.is_empty());
    }

    // --- Searches all sections (backlog, parked, done) ---

    #[test]
    fn test_search_parked_section() {
        let project = sample_project();
        let re = Regex::new("continuations").unwrap();
        let hits = search_tasks(&project, &re, None);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].task_id, "EFF-010");
    }

    #[test]
    fn test_search_done_section() {
        let project = sample_project();
        let re = Regex::new("Bootstrap").unwrap();
        let hits = search_tasks(&project, &re, None);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].task_id, "EFF-000");
    }

    // --- Inbox search ---

    #[test]
    fn test_inbox_search_title() {
        let inbox = sample_inbox();
        let re = Regex::new("error handling").unwrap();
        let hits = search_inbox(&inbox, &re);
        // Matches in both title and body of item 0
        assert_eq!(hits.len(), 2);
        let title_hits: Vec<_> = hits
            .iter()
            .filter(|h| h.field == MatchField::Title)
            .collect();
        assert_eq!(title_hits.len(), 1);
        assert_eq!(title_hits[0].item_index, 0);
    }

    #[test]
    fn test_inbox_search_tag() {
        let inbox = sample_inbox();
        let re = Regex::new("perf").unwrap();
        let hits = search_inbox(&inbox, &re);
        let tag_hits: Vec<_> = hits.iter().filter(|h| h.field == MatchField::Tag).collect();
        assert_eq!(tag_hits.len(), 1);
        assert_eq!(tag_hits[0].item_index, 1);
    }

    #[test]
    fn test_inbox_search_body() {
        let inbox = sample_inbox();
        let re = Regex::new("thoughts").unwrap();
        let hits = search_inbox(&inbox, &re);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].item_index, 0);
        assert_eq!(hits[0].field, MatchField::Body);
    }

    #[test]
    fn test_inbox_search_no_matches() {
        let inbox = sample_inbox();
        let re = Regex::new("zzzznotfound").unwrap();
        let hits = search_inbox(&inbox, &re);
        assert!(hits.is_empty());
    }

    #[test]
    fn test_inbox_search_multiple_items() {
        let inbox = sample_inbox();
        let re = Regex::new("parser").unwrap();
        let hits = search_inbox(&inbox, &re);
        // tag "parser" on item 1
        let tag_hits: Vec<_> = hits.iter().filter(|h| h.field == MatchField::Tag).collect();
        assert_eq!(tag_hits.len(), 1);
        assert_eq!(tag_hits[0].item_index, 1);
    }

    // --- Regex features ---

    #[test]
    fn test_search_case_insensitive_regex() {
        let project = sample_project();
        let re = Regex::new("(?i)bootstrap").unwrap();
        let hits = search_tasks(&project, &re, None);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].task_id, "EFF-000");
    }

    #[test]
    fn test_search_regex_alternation() {
        let project = sample_project();
        let re = Regex::new("handler|logging").unwrap();
        let hits = search_tasks(&project, &re, None);
        let title_hits: Vec<_> = hits
            .iter()
            .filter(|h| h.field == MatchField::Title)
            .collect();
        assert_eq!(title_hits.len(), 2);
    }

    // --- Task with no ID ---

    #[test]
    fn test_search_task_no_id() {
        // EFF-003's subtasks have IDs, but let's test a task without an ID
        let track = parse_track(
            "\
# Minimal

## Backlog

- [ ] A task without an ID #orphan

## Parked

## Done

",
        );
        let mut project = sample_project();
        project.tracks = vec![("minimal".to_string(), track)];
        project.config.tracks = vec![TrackConfig {
            id: "minimal".to_string(),
            name: "Minimal".to_string(),
            state: "active".to_string(),
            file: "minimal.md".to_string(),
        }];

        let re = Regex::new("orphan").unwrap();
        let hits = search_tasks(&project, &re, None);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].task_id, "");
        assert_eq!(hits[0].field, MatchField::Tag);
    }
}
