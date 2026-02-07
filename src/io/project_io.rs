use std::fs;
use std::path::{Path, PathBuf};

use crate::model::config::ProjectConfig;
use crate::model::inbox::Inbox;
use crate::model::project::Project;
use crate::model::track::Track;
use crate::parse::{parse_inbox, parse_track};

/// Error type for project I/O operations
#[derive(Debug, thiserror::Error)]
pub enum ProjectError {
    #[error("not a Frame project: no frame/ directory found")]
    NotAProject,
    #[error("could not read {path}: {source}")]
    ReadError {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("could not parse project.toml: {0}")]
    ConfigParseError(#[from] toml::de::Error),
    #[error("io error: {0}")]
    IoError(#[from] std::io::Error),
}

/// Discover the Frame project by walking up from the given directory,
/// looking for a `frame/` subdirectory.
pub fn discover_project(start: &Path) -> Result<PathBuf, ProjectError> {
    let mut current = start.to_path_buf();
    loop {
        let frame_dir = current.join("frame");
        if frame_dir.is_dir() && frame_dir.join("project.toml").exists() {
            return Ok(current);
        }
        if !current.pop() {
            return Err(ProjectError::NotAProject);
        }
    }
}

/// Load a complete Frame project from the given root directory.
pub fn load_project(root: &Path) -> Result<Project, ProjectError> {
    let frame_dir = root.join("frame");
    if !frame_dir.is_dir() {
        return Err(ProjectError::NotAProject);
    }

    // Read and parse project.toml
    let config_path = frame_dir.join("project.toml");
    let config_text = fs::read_to_string(&config_path).map_err(|e| ProjectError::ReadError {
        path: config_path.clone(),
        source: e,
    })?;
    let config: ProjectConfig = toml::from_str(&config_text)?;

    // Load tracks
    let mut tracks = Vec::new();
    for track_config in &config.tracks {
        let track_path = frame_dir.join(&track_config.file);
        if track_path.exists() {
            let track_text =
                fs::read_to_string(&track_path).map_err(|e| ProjectError::ReadError {
                    path: track_path.clone(),
                    source: e,
                })?;
            let track = parse_track(&track_text);
            tracks.push((track_config.id.clone(), track));
        }
    }

    // Load inbox
    let inbox_path = frame_dir.join("inbox.md");
    let inbox = if inbox_path.exists() {
        let inbox_text = fs::read_to_string(&inbox_path).map_err(|e| ProjectError::ReadError {
            path: inbox_path.clone(),
            source: e,
        })?;
        Some(parse_inbox(&inbox_text))
    } else {
        None
    };

    Ok(Project {
        root: root.to_path_buf(),
        frame_dir,
        config,
        tracks,
        inbox,
    })
}

/// Save a track file back to disk
pub fn save_track(frame_dir: &Path, file_path: &str, track: &Track) -> Result<(), ProjectError> {
    let full_path = frame_dir.join(file_path);
    let content = crate::parse::serialize_track(track);
    fs::write(&full_path, content).map_err(|e| ProjectError::ReadError {
        path: full_path,
        source: e,
    })?;
    Ok(())
}

/// Save the inbox file back to disk
pub fn save_inbox(frame_dir: &Path, inbox: &Inbox) -> Result<(), ProjectError> {
    let inbox_path = frame_dir.join("inbox.md");
    let content = crate::parse::serialize_inbox(inbox);
    fs::write(&inbox_path, content).map_err(|e| ProjectError::ReadError {
        path: inbox_path,
        source: e,
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn create_test_project(dir: &Path) {
        let frame_dir = dir.join("frame");
        fs::create_dir_all(frame_dir.join("tracks")).unwrap();

        fs::write(
            frame_dir.join("project.toml"),
            r#"
[project]
name = "test"

[[tracks]]
id = "main"
name = "Main Track"
state = "active"
file = "tracks/main.md"
"#,
        )
        .unwrap();

        fs::write(
            frame_dir.join("tracks/main.md"),
            "\
# Main Track

## Backlog

- [ ] `M-001` First task

## Done
",
        )
        .unwrap();

        fs::write(
            frame_dir.join("inbox.md"),
            "\
# Inbox

- A quick note #bug
",
        )
        .unwrap();
    }

    #[test]
    fn test_discover_project() {
        let tmp = TempDir::new().unwrap();
        create_test_project(tmp.path());

        // Discover from root
        let root = discover_project(tmp.path()).unwrap();
        assert_eq!(root, tmp.path());

        // Discover from subdirectory
        let sub = tmp.path().join("frame/tracks");
        let root = discover_project(&sub).unwrap();
        assert_eq!(root, tmp.path());
    }

    #[test]
    fn test_discover_project_not_found() {
        let tmp = TempDir::new().unwrap();
        assert!(discover_project(tmp.path()).is_err());
    }

    #[test]
    fn test_load_project() {
        let tmp = TempDir::new().unwrap();
        create_test_project(tmp.path());

        let project = load_project(tmp.path()).unwrap();
        assert_eq!(project.config.project.name, "test");
        assert_eq!(project.tracks.len(), 1);
        assert_eq!(project.tracks[0].0, "main");
        assert_eq!(project.tracks[0].1.backlog().len(), 1);
        assert!(project.inbox.is_some());
        assert_eq!(project.inbox.as_ref().unwrap().items.len(), 1);
    }
}
