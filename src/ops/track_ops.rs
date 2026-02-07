use std::fs;
use std::path::Path;

use crate::io::config_io;
use crate::io::project_io::ProjectError;
use crate::model::config::{ProjectConfig, TrackConfig};
use crate::model::track::{SectionKind, Track, TrackNode};

/// Error type for track operations
#[derive(Debug, thiserror::Error)]
pub enum TrackError {
    #[error("track not found: {0}")]
    NotFound(String),
    #[error("track ID already exists: {0}")]
    AlreadyExists(String),
    #[error("invalid state transition: {0}")]
    InvalidTransition(String),
    #[error("invalid position: {0}")]
    InvalidPosition(String),
    #[error("project error: {0}")]
    ProjectError(#[from] ProjectError),
}

/// Create a new track: creates the .md file and adds it to config.
pub fn new_track(
    frame_dir: &Path,
    doc: &mut toml_edit::DocumentMut,
    config: &mut ProjectConfig,
    track_id: &str,
    name: &str,
) -> Result<Track, TrackError> {
    // Check for duplicate
    if config.tracks.iter().any(|t| t.id == track_id) {
        return Err(TrackError::AlreadyExists(track_id.to_string()));
    }

    let file_path = format!("tracks/{}.md", track_id);
    let track_config = TrackConfig {
        id: track_id.to_string(),
        name: name.to_string(),
        state: "active".to_string(),
        file: file_path.clone(),
    };

    // Create the track file
    let full_path = frame_dir.join(&file_path);
    if let Some(parent) = full_path.parent() {
        fs::create_dir_all(parent).map_err(ProjectError::IoError)?;
    }
    let content = format!("# {}\n\n## Backlog\n\n## Done\n", name);
    fs::write(&full_path, &content).map_err(ProjectError::IoError)?;

    // Generate prefix and update config
    let existing_prefixes: Vec<String> = config.ids.prefixes.values().cloned().collect();
    let prefix = generate_prefix(track_id, &existing_prefixes);

    config_io::add_track_to_config(doc, &track_config);
    config_io::set_prefix(doc, track_id, &prefix);
    config.tracks.push(track_config);
    config.ids.prefixes.insert(track_id.to_string(), prefix);

    Ok(crate::parse::parse_track(&content))
}

/// Change a track's state to shelved.
pub fn shelve_track(
    doc: &mut toml_edit::DocumentMut,
    config: &mut ProjectConfig,
    track_id: &str,
) -> Result<(), TrackError> {
    let tc = config
        .tracks
        .iter_mut()
        .find(|t| t.id == track_id)
        .ok_or_else(|| TrackError::NotFound(track_id.to_string()))?;

    if tc.state == "archived" {
        return Err(TrackError::InvalidTransition(
            "cannot shelve an archived track".into(),
        ));
    }
    tc.state = "shelved".to_string();
    config_io::update_track_state(doc, track_id, "shelved");
    Ok(())
}

/// Change a track's state to active.
pub fn activate_track(
    doc: &mut toml_edit::DocumentMut,
    config: &mut ProjectConfig,
    track_id: &str,
) -> Result<(), TrackError> {
    let tc = config
        .tracks
        .iter_mut()
        .find(|t| t.id == track_id)
        .ok_or_else(|| TrackError::NotFound(track_id.to_string()))?;

    tc.state = "active".to_string();
    config_io::update_track_state(doc, track_id, "active");
    Ok(())
}

/// Change a track's state to archived.
pub fn archive_track(
    doc: &mut toml_edit::DocumentMut,
    config: &mut ProjectConfig,
    track_id: &str,
) -> Result<(), TrackError> {
    let tc = config
        .tracks
        .iter_mut()
        .find(|t| t.id == track_id)
        .ok_or_else(|| TrackError::NotFound(track_id.to_string()))?;

    tc.state = "archived".to_string();
    config_io::update_track_state(doc, track_id, "archived");
    Ok(())
}

/// Reorder active tracks: move `track_id` to `new_position` (0-indexed among
/// active tracks only).
pub fn reorder_tracks(
    config: &mut ProjectConfig,
    track_id: &str,
    new_position: usize,
) -> Result<(), TrackError> {
    // Find the track's current index in the full list
    let current_idx = config
        .tracks
        .iter()
        .position(|t| t.id == track_id)
        .ok_or_else(|| TrackError::NotFound(track_id.to_string()))?;

    if config.tracks[current_idx].state != "active" {
        return Err(TrackError::InvalidTransition(
            "can only reorder active tracks".into(),
        ));
    }

    // Collect indices of active tracks
    let active_indices: Vec<usize> = config
        .tracks
        .iter()
        .enumerate()
        .filter(|(_, t)| t.state == "active")
        .map(|(i, _)| i)
        .collect();

    if new_position >= active_indices.len() {
        return Err(TrackError::InvalidPosition(format!(
            "position {} out of range (0..{})",
            new_position,
            active_indices.len()
        )));
    }

    // Remove from current position and reinsert at new position
    let tc = config.tracks.remove(current_idx);

    // Recalculate active indices after removal
    let active_indices: Vec<usize> = config
        .tracks
        .iter()
        .enumerate()
        .filter(|(_, t)| t.state == "active")
        .map(|(i, _)| i)
        .collect();

    // Find the insertion index in the full list
    let insert_idx = if new_position >= active_indices.len() {
        // After the last active track
        active_indices
            .last()
            .map(|&i| i + 1)
            .unwrap_or(config.tracks.len())
    } else {
        active_indices[new_position]
    };

    config.tracks.insert(insert_idx, tc);
    Ok(())
}

/// Set the cc-focus track.
pub fn set_cc_focus(
    doc: &mut toml_edit::DocumentMut,
    config: &mut ProjectConfig,
    track_id: &str,
) -> Result<(), TrackError> {
    // Validate track exists and is active
    let tc = config
        .tracks
        .iter()
        .find(|t| t.id == track_id)
        .ok_or_else(|| TrackError::NotFound(track_id.to_string()))?;

    if tc.state != "active" {
        return Err(TrackError::InvalidTransition(
            "cc-focus must be an active track".into(),
        ));
    }

    config.agent.cc_focus = Some(track_id.to_string());
    config_io::set_cc_focus(doc, track_id);
    Ok(())
}

/// Generate an uppercase prefix for a track ID.
///
/// Rules:
/// 1. Take the last hyphen-separated segment of the track ID
/// 2. Uppercase the first 3 characters
/// 3. If the segment is shorter than 3 chars, use what's available
/// 4. If the result collides with an existing prefix, prepend characters
///    from earlier segments until unique
pub fn generate_prefix(track_id: &str, existing: &[String]) -> String {
    let segments: Vec<&str> = track_id.split('-').collect();
    let last = segments.last().unwrap_or(&track_id);
    let base: String = last.chars().take(3).collect::<String>().to_uppercase();

    if !existing.contains(&base) {
        return base;
    }

    // Collision — prepend chars from earlier segments to disambiguate.
    // Build a pool of chars from all segments except the last, in order.
    let earlier: String = segments[..segments.len().saturating_sub(1)]
        .iter()
        .flat_map(|s| s.chars())
        .collect();

    for i in 1..=earlier.len() {
        let prefix_chars: String = earlier[..i].chars().collect();
        let candidate: String = format!("{}{}", prefix_chars, last)
            .chars()
            .take(3)
            .collect::<String>()
            .to_uppercase();
        if !existing.contains(&candidate) {
            return candidate;
        }
    }

    // Fallback: use full track_id chars (shouldn't happen in practice)
    track_id
        .replace('-', "")
        .chars()
        .take(3)
        .collect::<String>()
        .to_uppercase()
}

/// Get all tasks from a track (backlog + parked), for counting/stats.
pub fn task_counts(track: &Track) -> TrackStats {
    let mut stats = TrackStats::default();
    for node in &track.nodes {
        if let TrackNode::Section { kind, tasks, .. } = node {
            count_tasks(tasks, &mut stats, *kind);
        }
    }
    stats
}

#[derive(Debug, Default)]
pub struct TrackStats {
    pub active: usize,
    pub blocked: usize,
    pub todo: usize,
    pub parked: usize,
    pub done: usize,
}

fn count_tasks(tasks: &[crate::model::Task], stats: &mut TrackStats, _section: SectionKind) {
    for task in tasks {
        match task.state {
            crate::model::TaskState::Active => stats.active += 1,
            crate::model::TaskState::Blocked => stats.blocked += 1,
            crate::model::TaskState::Todo => stats.todo += 1,
            crate::model::TaskState::Parked => stats.parked += 1,
            crate::model::TaskState::Done => stats.done += 1,
        }
        count_tasks(&task.subtasks, stats, _section);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn setup_test_project() -> (
        TempDir,
        std::path::PathBuf,
        ProjectConfig,
        toml_edit::DocumentMut,
    ) {
        let tmp = TempDir::new().unwrap();
        let frame_dir = tmp.path().join("frame");
        fs::create_dir_all(frame_dir.join("tracks")).unwrap();

        let config_text = r#"[project]
name = "test"

[agent]
cc_focus = "main"

[[tracks]]
id = "main"
name = "Main"
state = "active"
file = "tracks/main.md"

[[tracks]]
id = "side"
name = "Side"
state = "active"
file = "tracks/side.md"

[[tracks]]
id = "old"
name = "Old"
state = "shelved"
file = "tracks/old.md"
"#;
        fs::write(frame_dir.join("project.toml"), config_text).unwrap();

        let config: ProjectConfig = toml::from_str(config_text).unwrap();
        let doc: toml_edit::DocumentMut = config_text.parse().unwrap();

        (tmp, frame_dir, config, doc)
    }

    #[test]
    fn test_new_track() {
        let (tmp, frame_dir, mut config, mut doc) = setup_test_project();
        let track = new_track(&frame_dir, &mut doc, &mut config, "feat", "Features").unwrap();
        assert_eq!(track.title, "Features");
        assert!(frame_dir.join("tracks/feat.md").exists());
        assert_eq!(config.tracks.len(), 4);
        assert_eq!(config.tracks[3].id, "feat");
        drop(tmp);
    }

    #[test]
    fn test_new_track_duplicate() {
        let (tmp, frame_dir, mut config, mut doc) = setup_test_project();
        let result = new_track(&frame_dir, &mut doc, &mut config, "main", "Main Again");
        assert!(result.is_err());
        drop(tmp);
    }

    #[test]
    fn test_shelve_track() {
        let (_tmp, _, mut config, mut doc) = setup_test_project();
        shelve_track(&mut doc, &mut config, "main").unwrap();
        assert_eq!(config.tracks[0].state, "shelved");
    }

    #[test]
    fn test_activate_track() {
        let (_tmp, _, mut config, mut doc) = setup_test_project();
        activate_track(&mut doc, &mut config, "old").unwrap();
        assert_eq!(config.tracks[2].state, "active");
    }

    #[test]
    fn test_archive_track() {
        let (_tmp, _, mut config, mut doc) = setup_test_project();
        archive_track(&mut doc, &mut config, "main").unwrap();
        assert_eq!(config.tracks[0].state, "archived");
    }

    #[test]
    fn test_shelve_archived_fails() {
        let (_tmp, _, mut config, mut doc) = setup_test_project();
        archive_track(&mut doc, &mut config, "main").unwrap();
        let result = shelve_track(&mut doc, &mut config, "main");
        assert!(result.is_err());
    }

    #[test]
    fn test_set_cc_focus() {
        let (_tmp, _, mut config, mut doc) = setup_test_project();
        set_cc_focus(&mut doc, &mut config, "side").unwrap();
        assert_eq!(config.agent.cc_focus.as_deref(), Some("side"));
    }

    #[test]
    fn test_set_cc_focus_inactive_fails() {
        let (_tmp, _, mut config, mut doc) = setup_test_project();
        let result = set_cc_focus(&mut doc, &mut config, "old");
        assert!(result.is_err());
    }

    #[test]
    fn test_reorder_tracks() {
        let (_tmp, _, mut config, _doc) = setup_test_project();
        // main is at active position 0, side at position 1
        reorder_tracks(&mut config, "side", 0).unwrap();
        let active: Vec<&str> = config
            .tracks
            .iter()
            .filter(|t| t.state == "active")
            .map(|t| t.id.as_str())
            .collect();
        assert_eq!(active, vec!["side", "main"]);
    }

    #[test]
    fn test_generate_prefix_basic() {
        let existing = vec![];
        assert_eq!(generate_prefix("effects", &existing), "EFF");
        assert_eq!(generate_prefix("core", &existing), "COR");
        assert_eq!(generate_prefix("modules", &existing), "MOD");
    }

    #[test]
    fn test_generate_prefix_hyphenated() {
        let existing = vec![];
        assert_eq!(generate_prefix("compiler-infra", &existing), "INF");
        assert_eq!(generate_prefix("unique-types", &existing), "TYP");
        assert_eq!(generate_prefix("error-handling", &existing), "HAN");
    }

    #[test]
    fn test_generate_prefix_short_segment() {
        let existing = vec![];
        assert_eq!(generate_prefix("ui", &existing), "UI");
    }

    #[test]
    fn test_generate_prefix_collision() {
        // type-inference → TYP, then unique-types would also be TYP
        let existing = vec!["TYP".to_string()];
        let result = generate_prefix("unique-types", &existing);
        assert_eq!(result, "UTY");
        assert_ne!(result, "TYP");
    }

    #[test]
    fn test_generate_prefix_no_collision_different_tracks() {
        let existing = vec!["EFF".to_string()];
        assert_eq!(generate_prefix("core", &existing), "COR");
    }

    #[test]
    fn test_generate_prefix_all_table_cases() {
        // Verify all cases from the spec table
        let mut existing = vec![];
        let cases = vec![
            ("effects", "EFF"),
            ("compiler-infra", "INF"),
            ("unique-types", "TYP"),
            ("core", "COR"),
            ("ui", "UI"),
            ("modules", "MOD"),
            ("error-handling", "HAN"),
        ];
        for (id, expected) in cases {
            let result = generate_prefix(id, &existing);
            assert_eq!(
                result, expected,
                "prefix for '{}' should be '{}'",
                id, expected
            );
            existing.push(result);
        }
    }

    #[test]
    fn test_task_counts() {
        let track = crate::parse::parse_track(
            "\
# Test

## Backlog

- [>] `T-001` Active task
- [-] `T-002` Blocked task
- [ ] `T-003` Todo task
  - [ ] `T-003.1` Sub todo

## Parked

- [~] `T-010` Parked

## Done

- [x] `T-000` Done task
",
        );
        let stats = task_counts(&track);
        assert_eq!(stats.active, 1);
        assert_eq!(stats.blocked, 1);
        assert_eq!(stats.todo, 2);
        assert_eq!(stats.parked, 1);
        assert_eq!(stats.done, 1);
    }
}
