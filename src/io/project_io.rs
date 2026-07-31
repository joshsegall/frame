use std::fs;
use std::path::{Path, PathBuf};

use crate::model::config::ProjectConfig;
use crate::model::inbox::Inbox;
use crate::model::project::Project;
use crate::model::track::Track;
use crate::parse::{parse_inbox, parse_track};

/// Files inside `frame/` that belong to a **single working copy** and must never
/// be committed: per-worktree UI state, the advisory lock, the append-only
/// recovery log, this working copy's actor token, and the ID frontier store with
/// its lock (which only land here for a project outside git — inside git the
/// frontier lives under `.git/`, see [`crate::io::ids`]). Committing any of them
/// leaks machine-local state into shared history (and the append-only log
/// conflicts on every merge).
///
/// `fr init` writes these to `.gitignore` and `fr check` verifies them, both
/// from this one list — a project created before an entry was added here won't
/// have it, which is exactly what the check catches.
pub const LOCAL_ONLY_FRAME_FILES: [&str; 7] = [
    ".state.json",
    ".lock",
    ".recovery.log",
    ".actor",
    crate::io::inflight::MARKER_FILE,
    crate::io::ids::LOCAL_STORE,
    crate::io::ids::LOCAL_LOCK,
];

/// The `.gitignore` lines [`LOCAL_ONLY_FRAME_FILES`] corresponds to.
pub fn gitignore_entries() -> Vec<String> {
    LOCAL_ONLY_FRAME_FILES
        .iter()
        .map(|name| format!("frame/{name}"))
        .collect()
}

/// Which of [`gitignore_entries`] are absent from `<root>/.gitignore`.
///
/// Empty for a project outside git — there is nothing to leak into, and writing a
/// `.gitignore` where no repo exists would be surprising. Shared by `fr init`,
/// which writes them at creation, and `fr check --fix`, which adds the ones a
/// project created before an entry existed never got.
pub fn gitignore_entries_missing(root: &Path) -> Result<Vec<String>, std::io::Error> {
    if !root.join(".git").exists() {
        return Ok(Vec::new());
    }
    let existing = fs::read_to_string(root.join(".gitignore")).unwrap_or_default();
    Ok(gitignore_entries()
        .into_iter()
        .filter(|entry| !existing.lines().any(|line| line.trim() == entry))
        .collect())
}

/// Append one entry to `<root>/.gitignore`, creating the file if needed.
///
/// Idempotent: an entry already present is a no-op, so re-running `--fix` after a
/// partial run cannot duplicate lines.
pub fn append_gitignore_entry(root: &Path, entry: &str) -> Result<(), std::io::Error> {
    let path = root.join(".gitignore");
    let existing = fs::read_to_string(&path).unwrap_or_default();
    if existing.lines().any(|line| line.trim() == entry) {
        return Ok(());
    }
    let mut content = existing;
    if !content.is_empty() && !content.ends_with('\n') {
        content.push('\n');
    }
    content.push_str(entry);
    content.push('\n');
    fs::write(&path, content)
}

/// Error type for project I/O operations
#[derive(Debug, thiserror::Error)]
pub enum ProjectError {
    #[error("not a frame project: no frame/ directory found")]
    NotAProject,
    #[error("could not read {path}: {source}")]
    ReadError {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("could not parse project.toml: {0}")]
    ConfigParseError(#[from] toml::de::Error),
    #[error("could not serialize project.toml: {0}")]
    ConfigSerializeError(#[from] toml::ser::Error),
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
        let (inbox, dropped) = parse_inbox(&inbox_text);
        if !dropped.is_empty() {
            crate::io::recovery::log_recovery(
                &frame_dir,
                crate::io::recovery::RecoveryEntry {
                    timestamp: chrono::Utc::now(),
                    category: crate::io::recovery::RecoveryCategory::Parser,
                    description: "dropped lines".to_string(),
                    fields: vec![("Source".to_string(), "inbox.md".to_string())],
                    body: dropped.join("\n"),
                },
            );
        }
        Some(inbox)
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

/// Load archived tasks from `frame/archive/*.md` files.
///
/// Returns a list of `(track_id, tasks)` pairs. The track ID is derived from
/// the archive filename stem (e.g., `archive/main.md` → `"main"`).
/// Skips the `_tracks/` subdirectory (which holds archived whole-track files).
pub fn load_archives(
    frame_dir: &Path,
) -> Result<Vec<(String, Vec<crate::model::task::Task>)>, ProjectError> {
    let archive_dir = frame_dir.join("archive");
    if !archive_dir.is_dir() {
        return Ok(Vec::new());
    }

    let mut archives = Vec::new();
    let entries = fs::read_dir(&archive_dir).map_err(|e| ProjectError::ReadError {
        path: archive_dir.clone(),
        source: e,
    })?;

    for entry in entries {
        let entry = entry?;
        let path = entry.path();

        // Skip directories (e.g., _tracks/)
        if path.is_dir() {
            continue;
        }

        // Only process .md files
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }

        let track_id = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();

        let content = fs::read_to_string(&path).map_err(|e| ProjectError::ReadError {
            path: path.clone(),
            source: e,
        })?;

        // Parse task lines, skipping the "# Archive — ..." header
        let lines: Vec<String> = content.lines().map(|l| l.to_string()).collect();
        let start = lines
            .iter()
            .position(|l| l.starts_with("- ["))
            .unwrap_or(lines.len());
        let (tasks, _) = crate::parse::parse_tasks(&lines, start, 0, 0);

        if !tasks.is_empty() {
            archives.push((track_id, tasks));
        }
    }

    Ok(archives)
}

/// Save a track file back to disk
pub fn save_track(frame_dir: &Path, file_path: &str, track: &Track) -> Result<(), ProjectError> {
    let full_path = frame_dir.join(file_path);
    let content = crate::parse::serialize_track(track);
    if let Err(e) = crate::io::recovery::atomic_write(&full_path, content.as_bytes()) {
        crate::io::recovery::log_recovery(
            frame_dir,
            crate::io::recovery::RecoveryEntry {
                timestamp: chrono::Utc::now(),
                category: crate::io::recovery::RecoveryCategory::Write,
                description: "track write failed".to_string(),
                fields: vec![
                    ("Target".to_string(), file_path.to_string()),
                    ("Error".to_string(), e.to_string()),
                ],
                body: content,
            },
        );
        return Err(ProjectError::ReadError {
            path: full_path,
            source: e,
        });
    }
    Ok(())
}

/// Save the inbox file back to disk
pub fn save_inbox(frame_dir: &Path, inbox: &Inbox) -> Result<(), ProjectError> {
    let inbox_path = frame_dir.join("inbox.md");
    let content = crate::parse::serialize_inbox(inbox);
    if let Err(e) = crate::io::recovery::atomic_write(&inbox_path, content.as_bytes()) {
        crate::io::recovery::log_recovery(
            frame_dir,
            crate::io::recovery::RecoveryEntry {
                timestamp: chrono::Utc::now(),
                category: crate::io::recovery::RecoveryCategory::Write,
                description: "inbox write failed".to_string(),
                fields: vec![
                    ("Target".to_string(), "inbox.md".to_string()),
                    ("Error".to_string(), e.to_string()),
                ],
                body: content,
            },
        );
        return Err(ProjectError::ReadError {
            path: inbox_path,
            source: e,
        });
    }
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
