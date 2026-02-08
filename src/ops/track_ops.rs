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
    #[error("track is not empty: {0}")]
    NotEmpty(String),
    #[error("prefix collision: {0}")]
    PrefixCollision(String),
    #[error("project error: {0}")]
    ProjectError(#[from] ProjectError),
}

/// Result summary for a prefix rename operation
#[derive(Debug, Default)]
pub struct RenameResult {
    pub tasks_renamed: usize,
    pub deps_updated: usize,
    pub tracks_affected: usize,
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

/// Check if a track has zero tasks and no archive file
pub fn is_track_empty(frame_dir: &Path, track: &Track) -> bool {
    // Check all sections for any tasks
    for node in &track.nodes {
        if let TrackNode::Section { tasks, .. } = node {
            if !tasks.is_empty() {
                return false;
            }
        }
    }
    // Check for archive file
    let tc_id = track.title.to_lowercase().replace(' ', "-");
    let archive_path = frame_dir.join("archive").join(format!("{}.md", tc_id));
    !archive_path.exists()
}

/// Check if a track has zero tasks and no archive file (by track id)
pub fn is_track_empty_by_id(frame_dir: &Path, track: &Track, track_id: &str) -> bool {
    // Check all sections for any tasks
    for node in &track.nodes {
        if let TrackNode::Section { tasks, .. } = node {
            if !tasks.is_empty() {
                return false;
            }
        }
    }
    // Check for archive file
    let archive_path = frame_dir.join("archive").join(format!("{}.md", track_id));
    !archive_path.exists()
}

/// Delete a track entirely. Only works if the track is empty (no tasks, no archive file).
pub fn delete_track(
    frame_dir: &Path,
    doc: &mut toml_edit::DocumentMut,
    config: &mut ProjectConfig,
    track_id: &str,
) -> Result<(), TrackError> {
    let tc = config
        .tracks
        .iter()
        .find(|t| t.id == track_id)
        .ok_or_else(|| TrackError::NotFound(track_id.to_string()))?;

    let track_file = frame_dir.join(&tc.file);

    // Remove the track file
    if track_file.exists() {
        fs::remove_file(&track_file).map_err(ProjectError::IoError)?;
    }

    // Remove from config
    config_io::remove_track_from_config(doc, track_id);
    config_io::remove_prefix(doc, track_id);
    config.tracks.retain(|t| t.id != track_id);
    config.ids.prefixes.remove(track_id);

    Ok(())
}

/// Move a track file to archive/_tracks/ directory
pub fn archive_track_file(
    frame_dir: &Path,
    track_id: &str,
    file_path: &str,
) -> Result<(), TrackError> {
    let source = frame_dir.join(file_path);
    let archive_dir = frame_dir.join("archive").join("_tracks");
    fs::create_dir_all(&archive_dir).map_err(ProjectError::IoError)?;
    let dest = archive_dir.join(format!("{}.md", track_id));
    fs::rename(&source, &dest).map_err(ProjectError::IoError)?;
    Ok(())
}

/// Restore a track file from archive/_tracks/ back to tracks/
pub fn restore_track_file(
    frame_dir: &Path,
    track_id: &str,
    file_path: &str,
) -> Result<(), TrackError> {
    let archive_path = frame_dir
        .join("archive")
        .join("_tracks")
        .join(format!("{}.md", track_id));
    let dest = frame_dir.join(file_path);
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).map_err(ProjectError::IoError)?;
    }
    fs::rename(&archive_path, &dest).map_err(ProjectError::IoError)?;
    Ok(())
}

/// Rename a track's display name in config and in the track file's # Title header
pub fn rename_track_name(
    frame_dir: &Path,
    doc: &mut toml_edit::DocumentMut,
    config: &mut ProjectConfig,
    track_id: &str,
    new_name: &str,
) -> Result<(), TrackError> {
    let tc = config
        .tracks
        .iter_mut()
        .find(|t| t.id == track_id)
        .ok_or_else(|| TrackError::NotFound(track_id.to_string()))?;

    tc.name = new_name.to_string();
    config_io::update_track_name(doc, track_id, new_name);

    // Update the # Title line in the track file
    let track_path = frame_dir.join(&tc.file);
    if track_path.exists() {
        let content = fs::read_to_string(&track_path).map_err(ProjectError::IoError)?;
        let new_content = if let Some(first_line_end) = content.find('\n') {
            format!("# {}{}", new_name, &content[first_line_end..])
        } else {
            format!("# {}", new_name)
        };
        fs::write(&track_path, new_content).map_err(ProjectError::IoError)?;
    }

    Ok(())
}

/// Rename a track's ID: updates config, moves track file, moves archive file if exists, updates prefix key
pub fn rename_track_id(
    frame_dir: &Path,
    doc: &mut toml_edit::DocumentMut,
    config: &mut ProjectConfig,
    old_id: &str,
    new_id: &str,
) -> Result<(), TrackError> {
    // Check for collision
    if config.tracks.iter().any(|t| t.id == new_id) {
        return Err(TrackError::AlreadyExists(new_id.to_string()));
    }

    let tc = config
        .tracks
        .iter_mut()
        .find(|t| t.id == old_id)
        .ok_or_else(|| TrackError::NotFound(old_id.to_string()))?;

    let old_file = tc.file.clone();
    let new_file = format!("tracks/{}.md", new_id);

    // Move track file
    let old_path = frame_dir.join(&old_file);
    let new_path = frame_dir.join(&new_file);
    if old_path.exists() {
        fs::rename(&old_path, &new_path).map_err(ProjectError::IoError)?;
    }

    // Move archive file if exists
    let old_archive = frame_dir.join("archive").join(format!("{}.md", old_id));
    if old_archive.exists() {
        let new_archive = frame_dir.join("archive").join(format!("{}.md", new_id));
        fs::rename(&old_archive, &new_archive).map_err(ProjectError::IoError)?;
    }

    // Update config
    tc.id = new_id.to_string();
    tc.file = new_file;

    config_io::update_track_id(doc, old_id, new_id);
    config_io::rename_prefix_key(doc, old_id, new_id);

    // Update in-memory prefix map
    if let Some(prefix) = config.ids.prefixes.remove(old_id) {
        config.ids.prefixes.insert(new_id.to_string(), prefix);
    }

    // Update cc_focus if it pointed to the old id
    if config.agent.cc_focus.as_deref() == Some(old_id) {
        config.agent.cc_focus = Some(new_id.to_string());
        config_io::set_cc_focus(doc, new_id);
    }

    Ok(())
}

/// Rename a track's prefix: bulk-rewrites all task IDs in the track and dep references across all tracks.
/// Returns a summary of changes. Does NOT write to disk — caller must save all affected tracks/config.
pub fn rename_track_prefix(
    config: &mut ProjectConfig,
    project_tracks: &mut [(String, Track)],
    track_id: &str,
    old_prefix: &str,
    new_prefix: &str,
) -> Result<RenameResult, TrackError> {
    // Check for prefix collision
    for (tid, prefix) in &config.ids.prefixes {
        if tid != track_id && prefix.eq_ignore_ascii_case(new_prefix) {
            return Err(TrackError::PrefixCollision(new_prefix.to_string()));
        }
    }

    let mut result = RenameResult::default();

    // Rewrite task IDs in the target track
    let track = project_tracks
        .iter_mut()
        .find(|(id, _)| id == track_id)
        .map(|(_, t)| t)
        .ok_or_else(|| TrackError::NotFound(track_id.to_string()))?;

    for node in &mut track.nodes {
        if let TrackNode::Section { tasks, .. } = node {
            result.tasks_renamed += rename_task_ids(tasks, old_prefix, new_prefix);
        }
    }

    // Update dep references across ALL tracks (including the target track itself)
    let mut affected_tracks = std::collections::HashSet::new();
    for (tid, track) in project_tracks.iter_mut() {
        let count = rename_dep_references(track, old_prefix, new_prefix);
        if count > 0 {
            result.deps_updated += count;
            if tid != track_id {
                affected_tracks.insert(tid.clone());
            }
        }
    }
    result.tracks_affected = affected_tracks.len();

    // Update config prefix
    config
        .ids
        .prefixes
        .insert(track_id.to_string(), new_prefix.to_string());

    Ok(result)
}

/// Rename task IDs in an archive file on disk. Reads the archive, renames matching
/// IDs, writes it back. Returns the number of IDs renamed, or 0 if the file doesn't exist.
pub fn rename_archive_prefix(
    frame_dir: &Path,
    track_id: &str,
    old_prefix: &str,
    new_prefix: &str,
) -> Result<usize, TrackError> {
    let archive_path = frame_dir.join("archive").join(format!("{}.md", track_id));
    if !archive_path.exists() {
        return Ok(0);
    }
    let content = fs::read_to_string(&archive_path).map_err(ProjectError::IoError)?;
    let mut archive_track = crate::parse::parse_track(&content);
    let mut count = 0;
    for node in &mut archive_track.nodes {
        if let TrackNode::Section { tasks, .. } = node {
            count += rename_task_ids(tasks, old_prefix, new_prefix);
        }
    }
    if count > 0 {
        let serialized = crate::parse::serialize_track(&archive_track);
        fs::write(&archive_path, serialized).map_err(ProjectError::IoError)?;
    }
    Ok(count)
}

/// Rename task IDs in a list of tasks (recursive). Returns count of tasks renamed.
pub fn rename_task_ids(
    tasks: &mut [crate::model::Task],
    old_prefix: &str,
    new_prefix: &str,
) -> usize {
    let mut count = 0;
    for task in tasks.iter_mut() {
        if let Some(ref mut id) = task.id {
            if let Some(rest) = id.strip_prefix(old_prefix) {
                if rest.starts_with('-') {
                    *id = format!("{}{}", new_prefix, rest);
                    task.mark_dirty();
                    count += 1;
                }
            }
        }
        count += rename_task_ids(&mut task.subtasks, old_prefix, new_prefix);
    }
    count
}

/// Rename dep references in a track. Returns count of deps renamed.
fn rename_dep_references(track: &mut Track, old_prefix: &str, new_prefix: &str) -> usize {
    let mut count = 0;
    for node in &mut track.nodes {
        if let TrackNode::Section { tasks, .. } = node {
            count += rename_deps_in_tasks(tasks, old_prefix, new_prefix);
        }
    }
    count
}

/// Rename dep references in a list of tasks (recursive).
fn rename_deps_in_tasks(
    tasks: &mut [crate::model::Task],
    old_prefix: &str,
    new_prefix: &str,
) -> usize {
    let mut count = 0;
    for task in tasks.iter_mut() {
        for m in &mut task.metadata {
            if let crate::model::task::Metadata::Dep(deps) = m {
                for dep in deps.iter_mut() {
                    if let Some(rest) = dep.strip_prefix(old_prefix) {
                        if rest.starts_with('-') {
                            *dep = format!("{}{}", new_prefix, rest);
                            task.dirty = true;
                            count += 1;
                        }
                    }
                }
            }
        }
        count += rename_deps_in_tasks(&mut task.subtasks, old_prefix, new_prefix);
    }
    count
}

/// Generate a track ID from a display name by slugifying it:
/// lowercase, spaces to hyphens, strip non-alphanumeric/non-hyphen chars.
pub fn generate_track_id(name: &str) -> String {
    name.to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-")
}

/// Impact summary for a prefix rename (read-only, does not modify data)
#[derive(Debug, Default)]
pub struct PrefixRenameImpact {
    /// Number of task/subtask IDs carrying the old prefix in the target track
    pub task_id_count: usize,
    /// Number of dep references to old-prefix IDs across other tracks
    pub dep_ref_count: usize,
    /// Number of other tracks containing affected dep references
    pub affected_track_count: usize,
}

/// Compute the blast radius of renaming a track's prefix without modifying any data.
/// Scans the target track for task IDs with `old_prefix`, and all other tracks for
/// dep references pointing to old-prefix IDs. Also counts archived tasks if an
/// archive file exists.
pub fn prefix_rename_impact(
    project_tracks: &[(String, Track)],
    track_id: &str,
    old_prefix: &str,
    archive_dir: Option<&Path>,
) -> PrefixRenameImpact {
    let mut impact = PrefixRenameImpact::default();

    // Count task IDs in target track
    if let Some((_, track)) = project_tracks.iter().find(|(id, _)| id == track_id) {
        for node in &track.nodes {
            if let TrackNode::Section { tasks, .. } = node {
                impact.task_id_count += count_prefix_ids(tasks, old_prefix);
            }
        }
    }

    // Count archived tasks if archive file exists
    if let Some(archive_dir) = archive_dir {
        let archive_path = archive_dir.join(format!("{}.md", track_id));
        if archive_path.exists() {
            if let Ok(content) = fs::read_to_string(&archive_path) {
                let archive_track = crate::parse::parse_track(&content);
                for node in &archive_track.nodes {
                    if let TrackNode::Section { tasks, .. } = node {
                        impact.task_id_count += count_prefix_ids(tasks, old_prefix);
                    }
                }
            }
        }
    }

    // Count dep references across ALL tracks (including the target track itself)
    let mut affected_tracks = std::collections::HashSet::new();
    for (tid, track) in project_tracks {
        let count = count_dep_references(track, old_prefix);
        if count > 0 {
            impact.dep_ref_count += count;
            if tid != track_id {
                affected_tracks.insert(tid.clone());
            }
        }
    }
    impact.affected_track_count = affected_tracks.len();

    impact
}

/// Count task IDs matching a prefix in a task list (recursive)
fn count_prefix_ids(tasks: &[crate::model::Task], prefix: &str) -> usize {
    let mut count = 0;
    for task in tasks {
        if let Some(ref id) = task.id {
            if let Some(rest) = id.strip_prefix(prefix) {
                if rest.starts_with('-') {
                    count += 1;
                }
            }
        }
        count += count_prefix_ids(&task.subtasks, prefix);
    }
    count
}

/// Count dep references matching a prefix in a track (recursive)
fn count_dep_references(track: &Track, prefix: &str) -> usize {
    let mut count = 0;
    for node in &track.nodes {
        if let TrackNode::Section { tasks, .. } = node {
            count += count_deps_in_tasks(tasks, prefix);
        }
    }
    count
}

/// Count dep references matching a prefix in a task list (recursive)
fn count_deps_in_tasks(tasks: &[crate::model::Task], prefix: &str) -> usize {
    let mut count = 0;
    for task in tasks {
        for m in &task.metadata {
            if let crate::model::task::Metadata::Dep(deps) = m {
                for dep in deps {
                    if let Some(rest) = dep.strip_prefix(prefix) {
                        if rest.starts_with('-') {
                            count += 1;
                        }
                    }
                }
            }
        }
        count += count_deps_in_tasks(&task.subtasks, prefix);
    }
    count
}

/// Count the total number of tasks in a track (across all sections, including subtasks)
pub fn total_task_count(track: &Track) -> usize {
    let stats = task_counts(track);
    stats.active + stats.blocked + stats.todo + stats.parked + stats.done
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

    #[test]
    fn test_generate_track_id() {
        assert_eq!(generate_track_id("Effect System"), "effect-system");
        assert_eq!(generate_track_id("My Cool Track!"), "my-cool-track");
        assert_eq!(generate_track_id("UI"), "ui");
        assert_eq!(generate_track_id("  spaces  "), "spaces");
    }

    #[test]
    fn test_is_track_empty_by_id() {
        let track = crate::parse::parse_track("# Empty\n\n## Backlog\n\n## Done\n");
        let tmp = TempDir::new().unwrap();
        let frame_dir = tmp.path().join("frame");
        fs::create_dir_all(&frame_dir).unwrap();
        assert!(is_track_empty_by_id(&frame_dir, &track, "empty"));
    }

    #[test]
    fn test_is_track_not_empty_with_tasks() {
        let track =
            crate::parse::parse_track("# Test\n\n## Backlog\n\n- [ ] `T-001` Task\n\n## Done\n");
        let tmp = TempDir::new().unwrap();
        let frame_dir = tmp.path().join("frame");
        fs::create_dir_all(&frame_dir).unwrap();
        assert!(!is_track_empty_by_id(&frame_dir, &track, "test"));
    }

    #[test]
    fn test_is_track_not_empty_with_archive() {
        let track = crate::parse::parse_track("# Test\n\n## Backlog\n\n## Done\n");
        let tmp = TempDir::new().unwrap();
        let frame_dir = tmp.path().join("frame");
        let archive_dir = frame_dir.join("archive");
        fs::create_dir_all(&archive_dir).unwrap();
        fs::write(archive_dir.join("test.md"), "archive content").unwrap();
        assert!(!is_track_empty_by_id(&frame_dir, &track, "test"));
    }

    #[test]
    fn test_delete_track() {
        let (tmp, frame_dir, mut config, mut doc) = setup_test_project();
        // Create the track file
        fs::write(
            frame_dir.join("tracks/main.md"),
            "# Main\n\n## Backlog\n\n## Done\n",
        )
        .unwrap();
        delete_track(&frame_dir, &mut doc, &mut config, "main").unwrap();
        assert!(!frame_dir.join("tracks/main.md").exists());
        assert_eq!(config.tracks.len(), 2);
        assert!(config.tracks.iter().all(|t| t.id != "main"));
        drop(tmp);
    }

    #[test]
    fn test_archive_track_file() {
        let tmp = TempDir::new().unwrap();
        let frame_dir = tmp.path().join("frame");
        fs::create_dir_all(frame_dir.join("tracks")).unwrap();
        fs::write(frame_dir.join("tracks/test.md"), "# Test\n").unwrap();

        archive_track_file(&frame_dir, "test", "tracks/test.md").unwrap();

        assert!(!frame_dir.join("tracks/test.md").exists());
        assert!(frame_dir.join("archive/_tracks/test.md").exists());
        drop(tmp);
    }

    #[test]
    fn test_restore_track_file() {
        let tmp = TempDir::new().unwrap();
        let frame_dir = tmp.path().join("frame");
        fs::create_dir_all(frame_dir.join("tracks")).unwrap();
        let archive_dir = frame_dir.join("archive/_tracks");
        fs::create_dir_all(&archive_dir).unwrap();
        fs::write(archive_dir.join("test.md"), "# Test\n").unwrap();

        restore_track_file(&frame_dir, "test", "tracks/test.md").unwrap();

        assert!(frame_dir.join("tracks/test.md").exists());
        assert!(!archive_dir.join("test.md").exists());
        drop(tmp);
    }

    #[test]
    fn test_rename_track_name() {
        let (tmp, frame_dir, mut config, mut doc) = setup_test_project();
        fs::write(
            frame_dir.join("tracks/main.md"),
            "# Main\n\n## Backlog\n\n## Done\n",
        )
        .unwrap();
        rename_track_name(&frame_dir, &mut doc, &mut config, "main", "Main Track").unwrap();
        assert_eq!(config.tracks[0].name, "Main Track");
        let content = fs::read_to_string(frame_dir.join("tracks/main.md")).unwrap();
        assert!(content.starts_with("# Main Track"));
        drop(tmp);
    }

    #[test]
    fn test_rename_track_id() {
        let (tmp, frame_dir, mut config, mut doc) = setup_test_project();
        fs::write(frame_dir.join("tracks/main.md"), "# Main\n").unwrap();
        rename_track_id(&frame_dir, &mut doc, &mut config, "main", "primary").unwrap();
        assert_eq!(config.tracks[0].id, "primary");
        assert_eq!(config.tracks[0].file, "tracks/primary.md");
        assert!(frame_dir.join("tracks/primary.md").exists());
        assert!(!frame_dir.join("tracks/main.md").exists());
        // cc_focus should have been updated
        assert_eq!(config.agent.cc_focus.as_deref(), Some("primary"));
        drop(tmp);
    }

    #[test]
    fn test_rename_track_id_collision() {
        let (_tmp, frame_dir, mut config, mut doc) = setup_test_project();
        let result = rename_track_id(&frame_dir, &mut doc, &mut config, "main", "side");
        assert!(matches!(result, Err(TrackError::AlreadyExists(_))));
    }

    #[test]
    fn test_rename_track_prefix() {
        let track_content = "\
# Effects

## Backlog

- [>] `EFF-001` First task
  - [ ] `EFF-001.1` Subtask
- [ ] `EFF-002` Second task
  - dep: EFF-001

## Done
";
        let other_track_content = "\
# Other

## Backlog

- [ ] `OTH-001` Other task
  - dep: EFF-001, EFF-002

## Done
";
        let mut config = ProjectConfig {
            project: crate::model::config::ProjectInfo {
                name: "test".into(),
            },
            agent: Default::default(),
            tracks: vec![],
            clean: Default::default(),
            ids: crate::model::config::IdConfig {
                prefixes: [
                    ("effects".into(), "EFF".into()),
                    ("other".into(), "OTH".into()),
                ]
                .into(),
            },
            ui: Default::default(),
        };

        let mut tracks = vec![
            (
                "effects".to_string(),
                crate::parse::parse_track(track_content),
            ),
            (
                "other".to_string(),
                crate::parse::parse_track(other_track_content),
            ),
        ];

        let result = rename_track_prefix(&mut config, &mut tracks, "effects", "EFF", "FX").unwrap();
        assert_eq!(result.tasks_renamed, 3); // EFF-001, EFF-001.1, EFF-002
        assert_eq!(result.deps_updated, 3); // EFF-001 in same track + EFF-001 and EFF-002 in other track
        assert_eq!(result.tracks_affected, 1); // other track (same track not counted)
        assert_eq!(config.ids.prefixes.get("effects").unwrap(), "FX");

        // Verify task IDs were renamed
        let effects = &tracks[0].1;
        let backlog = effects.backlog();
        assert_eq!(backlog[0].id.as_deref(), Some("FX-001"));
        assert_eq!(backlog[0].subtasks[0].id.as_deref(), Some("FX-001.1"));
        assert_eq!(backlog[1].id.as_deref(), Some("FX-002"));

        // Verify same-track dep was also renamed
        let has_renamed_dep = backlog[1].metadata.iter().any(|m| {
            matches!(m, crate::model::task::Metadata::Dep(deps) if deps.contains(&"FX-001".to_string()))
        });
        assert!(has_renamed_dep, "same-track dep EFF-001 should be renamed to FX-001");
    }

    #[test]
    fn test_rename_track_prefix_collision() {
        let mut config = ProjectConfig {
            project: crate::model::config::ProjectInfo {
                name: "test".into(),
            },
            agent: Default::default(),
            tracks: vec![],
            clean: Default::default(),
            ids: crate::model::config::IdConfig {
                prefixes: [("a".into(), "AAA".into()), ("b".into(), "BBB".into())].into(),
            },
            ui: Default::default(),
        };

        let track_content = "# A\n\n## Backlog\n\n## Done\n";
        let mut tracks = vec![
            ("a".to_string(), crate::parse::parse_track(track_content)),
            ("b".to_string(), crate::parse::parse_track(track_content)),
        ];

        let result = rename_track_prefix(&mut config, &mut tracks, "a", "AAA", "BBB");
        assert!(matches!(result, Err(TrackError::PrefixCollision(_))));
    }

    #[test]
    fn test_total_task_count() {
        let track = crate::parse::parse_track(
            "# Test\n\n## Backlog\n\n- [ ] `T-001` A\n- [ ] `T-002` B\n\n## Done\n\n- [x] `T-003` C\n",
        );
        assert_eq!(total_task_count(&track), 3);
    }

    #[test]
    fn test_prefix_rename_impact_basic() {
        let track_content = "\
# Effects

## Backlog

- [>] `EFF-001` First task
  - [ ] `EFF-001.1` Subtask
- [ ] `EFF-002` Second task
  - dep: EFF-001

## Done
";
        let other_content = "\
# Other

## Backlog

- [ ] `OTH-001` Other task
  - dep: EFF-001, EFF-002

## Done
";
        let tracks = vec![
            (
                "effects".to_string(),
                crate::parse::parse_track(track_content),
            ),
            (
                "other".to_string(),
                crate::parse::parse_track(other_content),
            ),
        ];

        let impact = prefix_rename_impact(&tracks, "effects", "EFF", None);
        assert_eq!(impact.task_id_count, 3); // EFF-001, EFF-001.1, EFF-002
        assert_eq!(impact.dep_ref_count, 3); // EFF-001 in same track + EFF-001 and EFF-002 in other
        assert_eq!(impact.affected_track_count, 1); // other track (same track not counted)
    }

    #[test]
    fn test_prefix_rename_impact_no_tasks() {
        let empty_content = "# Empty\n\n## Backlog\n\n## Done\n";
        let tracks = vec![(
            "empty".to_string(),
            crate::parse::parse_track(empty_content),
        )];

        let impact = prefix_rename_impact(&tracks, "empty", "EMP", None);
        assert_eq!(impact.task_id_count, 0);
        assert_eq!(impact.dep_ref_count, 0);
        assert_eq!(impact.affected_track_count, 0);
    }

    #[test]
    fn test_prefix_rename_impact_cross_track_deps() {
        let track_a = "\
# A

## Backlog

- [ ] `AAA-001` Task A

## Done
";
        let track_b = "\
# B

## Backlog

- [ ] `BBB-001` Task B
  - dep: AAA-001

## Done
";
        let track_c = "\
# C

## Backlog

- [ ] `CCC-001` Task C
  - dep: AAA-001

## Done
";
        let tracks = vec![
            ("a".to_string(), crate::parse::parse_track(track_a)),
            ("b".to_string(), crate::parse::parse_track(track_b)),
            ("c".to_string(), crate::parse::parse_track(track_c)),
        ];

        let impact = prefix_rename_impact(&tracks, "a", "AAA", None);
        assert_eq!(impact.task_id_count, 1);
        assert_eq!(impact.dep_ref_count, 2);
        assert_eq!(impact.affected_track_count, 2);
    }

    #[test]
    fn test_prefix_rename_impact_with_archive() {
        let track_content = "\
# Effects

## Backlog

- [ ] `EFF-010` Task

## Done
";
        let tracks = vec![(
            "effects".to_string(),
            crate::parse::parse_track(track_content),
        )];

        let tmp = TempDir::new().unwrap();
        let archive_dir = tmp.path().join("archive");
        fs::create_dir_all(&archive_dir).unwrap();
        let archive_content = "\
# Effects

## Done

- [x] `EFF-001` Archived task 1
- [x] `EFF-002` Archived task 2
  - [x] `EFF-002.1` Archived subtask
";
        fs::write(archive_dir.join("effects.md"), archive_content).unwrap();

        let impact = prefix_rename_impact(&tracks, "effects", "EFF", Some(&archive_dir));
        assert_eq!(impact.task_id_count, 4); // 1 in track + 3 in archive
        assert_eq!(impact.dep_ref_count, 0);
        assert_eq!(impact.affected_track_count, 0);
    }
}
