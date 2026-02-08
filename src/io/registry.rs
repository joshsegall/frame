use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A single project entry in the global registry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectEntry {
    pub name: String,
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_accessed_tui: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_accessed_cli: Option<DateTime<Utc>>,
}

/// The global project registry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectRegistry {
    #[serde(default)]
    pub projects: Vec<ProjectEntry>,
}

impl Default for ProjectRegistry {
    fn default() -> Self {
        Self {
            projects: Vec::new(),
        }
    }
}

/// Get the registry file path, respecting XDG_CONFIG_HOME
pub fn registry_path() -> PathBuf {
    let config_dir = std::env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| dirs_home().join(".config"));
    config_dir.join("frame").join("projects.toml")
}

/// Get the user's home directory
fn dirs_home() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/"))
}

/// Read the project registry from a specific path.
/// If the file doesn't exist, returns an empty registry.
/// If the file is corrupted, backs it up as .bak and returns empty.
pub fn read_registry_from(path: &Path) -> ProjectRegistry {
    if !path.exists() {
        return ProjectRegistry::default();
    }

    match fs::read_to_string(path) {
        Ok(content) => match toml::from_str::<ProjectRegistry>(&content) {
            Ok(reg) => reg,
            Err(e) => {
                // Corrupted — back up and start fresh
                let bak = path.with_extension("toml.bak");
                let _ = fs::copy(path, &bak);
                eprintln!(
                    "warning: could not parse {} (backed up as {}): {}",
                    path.display(),
                    bak.display(),
                    e
                );
                ProjectRegistry::default()
            }
        },
        Err(_) => ProjectRegistry::default(),
    }
}

/// Read the project registry from the default location.
pub fn read_registry() -> ProjectRegistry {
    read_registry_from(&registry_path())
}

/// Write the project registry to a specific path.
pub fn write_registry_to(path: &Path, reg: &ProjectRegistry) -> Result<(), std::io::Error> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let content =
        toml::to_string_pretty(reg).map_err(|e| std::io::Error::other(e.to_string()))?;
    fs::write(path, content)
}

/// Write the project registry to the default location.
pub fn write_registry(reg: &ProjectRegistry) -> Result<(), std::io::Error> {
    write_registry_to(&registry_path(), reg)
}

/// Register a project in the registry. If already registered (by path), updates the name.
/// Returns true if this was a new registration.
pub fn register_project(name: &str, abs_path: &Path) -> bool {
    let reg_path = registry_path();
    register_project_in(&reg_path, name, abs_path)
}

/// Register a project in a specific registry file.
pub fn register_project_in(reg_path: &Path, name: &str, abs_path: &Path) -> bool {
    let path_str = abs_path.to_string_lossy().to_string();
    let mut reg = read_registry_from(reg_path);

    if let Some(entry) = reg.projects.iter_mut().find(|e| e.path == path_str) {
        // Already registered — update name in case it changed
        entry.name = name.to_string();
        let _ = write_registry_to(reg_path, &reg);
        return false;
    }

    reg.projects.push(ProjectEntry {
        name: name.to_string(),
        path: path_str,
        last_accessed_tui: None,
        last_accessed_cli: None,
    });
    let _ = write_registry_to(reg_path, &reg);
    true
}

/// Update the last_accessed_tui timestamp for a project path.
pub fn touch_tui(abs_path: &Path) {
    let reg_path = registry_path();
    let path_str = abs_path.to_string_lossy().to_string();
    let mut reg = read_registry_from(&reg_path);
    if let Some(entry) = reg.projects.iter_mut().find(|e| e.path == path_str) {
        entry.last_accessed_tui = Some(Utc::now());
        let _ = write_registry_to(&reg_path, &reg);
    }
}

/// Update the last_accessed_cli timestamp for a project path.
pub fn touch_cli(abs_path: &Path) {
    let reg_path = registry_path();
    let path_str = abs_path.to_string_lossy().to_string();
    let mut reg = read_registry_from(&reg_path);
    if let Some(entry) = reg.projects.iter_mut().find(|e| e.path == path_str) {
        entry.last_accessed_cli = Some(Utc::now());
        let _ = write_registry_to(&reg_path, &reg);
    }
}

/// Remove a project from the registry by name or path.
/// Returns the removed entry, or None if not found.
/// If name is ambiguous (multiple matches), returns Err with count.
pub fn remove_project(name_or_path: &str) -> Result<Option<ProjectEntry>, String> {
    let reg_path = registry_path();
    remove_project_from(&reg_path, name_or_path)
}

/// Remove a project from a specific registry file.
pub fn remove_project_from(
    reg_path: &Path,
    name_or_path: &str,
) -> Result<Option<ProjectEntry>, String> {
    let mut reg = read_registry_from(reg_path);

    // Try exact path match first
    let abs_path = fs::canonicalize(name_or_path).ok();
    if let Some(ref abs) = abs_path {
        let abs_str = abs.to_string_lossy().to_string();
        if let Some(idx) = reg.projects.iter().position(|e| e.path == abs_str) {
            let removed = reg.projects.remove(idx);
            let _ = write_registry_to(reg_path, &reg);
            return Ok(Some(removed));
        }
    }

    // Also try raw string match on path
    if let Some(idx) = reg.projects.iter().position(|e| e.path == name_or_path) {
        let removed = reg.projects.remove(idx);
        let _ = write_registry_to(reg_path, &reg);
        return Ok(Some(removed));
    }

    // Try name match
    let matches: Vec<usize> = reg
        .projects
        .iter()
        .enumerate()
        .filter(|(_, e)| e.name == name_or_path)
        .map(|(i, _)| i)
        .collect();

    match matches.len() {
        0 => Ok(None),
        1 => {
            let removed = reg.projects.remove(matches[0]);
            let _ = write_registry_to(reg_path, &reg);
            Ok(Some(removed))
        }
        n => Err(format!(
            "ambiguous: {} projects named \"{}\". Specify by path instead.",
            n, name_or_path
        )),
    }
}

/// Remove a project from the registry by exact path string.
pub fn remove_by_path(path_str: &str) -> Option<ProjectEntry> {
    let reg_path = registry_path();
    let mut reg = read_registry_from(&reg_path);
    if let Some(idx) = reg.projects.iter().position(|e| e.path == path_str) {
        let removed = reg.projects.remove(idx);
        let _ = write_registry_to(&reg_path, &reg);
        Some(removed)
    } else {
        None
    }
}

/// Abbreviate a path by replacing $HOME with ~
pub fn abbreviate_path(path: &str) -> String {
    if let Ok(home) = std::env::var("HOME") {
        if let Some(rest) = path.strip_prefix(&home) {
            return format!("~{}", rest);
        }
    }
    path.to_string()
}

/// Format a relative time string like "2 min ago", "yesterday", "3 days ago"
pub fn relative_time(dt: &DateTime<Utc>) -> String {
    let now = Utc::now();
    let duration = now.signed_duration_since(*dt);

    let secs = duration.num_seconds();
    if secs < 60 {
        return "just now".to_string();
    }
    let mins = duration.num_minutes();
    if mins < 60 {
        return format!("{} min ago", mins);
    }
    let hours = duration.num_hours();
    if hours < 24 {
        return format!("{} hr ago", hours);
    }
    let days = duration.num_days();
    if days == 1 {
        return "yesterday".to_string();
    }
    if days < 7 {
        return format!("{} days ago", days);
    }
    let weeks = days / 7;
    if weeks < 5 {
        return format!("{} weeks ago", weeks);
    }
    format!("{} months ago", days / 30)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn temp_registry() -> (TempDir, PathBuf) {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("frame").join("projects.toml");
        (tmp, path)
    }

    #[test]
    fn test_empty_registry() {
        let (_tmp, path) = temp_registry();
        let reg = read_registry_from(&path);
        assert!(reg.projects.is_empty());
    }

    #[test]
    fn test_register_and_read() {
        let (_tmp, path) = temp_registry();
        let is_new = register_project_in(&path, "test-proj", Path::new("/tmp/test"));
        assert!(is_new);
        let reg = read_registry_from(&path);
        assert_eq!(reg.projects.len(), 1);
        assert_eq!(reg.projects[0].name, "test-proj");
        assert_eq!(reg.projects[0].path, "/tmp/test");
    }

    #[test]
    fn test_register_duplicate_path() {
        let (_tmp, path) = temp_registry();
        register_project_in(&path, "proj", Path::new("/tmp/test"));
        let is_new = register_project_in(&path, "proj-renamed", Path::new("/tmp/test"));
        assert!(!is_new);
        let reg = read_registry_from(&path);
        assert_eq!(reg.projects.len(), 1);
        assert_eq!(reg.projects[0].name, "proj-renamed");
    }

    #[test]
    fn test_remove_by_name() {
        let (_tmp, path) = temp_registry();
        register_project_in(&path, "my-proj", Path::new("/tmp/my-proj"));
        let removed = remove_project_from(&path, "my-proj").unwrap();
        assert!(removed.is_some());
        assert_eq!(removed.unwrap().name, "my-proj");
        let reg = read_registry_from(&path);
        assert!(reg.projects.is_empty());
    }

    #[test]
    fn test_remove_not_found() {
        let (_tmp, path) = temp_registry();
        let removed = remove_project_from(&path, "nonexistent").unwrap();
        assert!(removed.is_none());
    }

    #[test]
    fn test_abbreviate_path() {
        let home = std::env::var("HOME").unwrap_or_default();
        let p = format!("{}/code/frame", home);
        let abbrev = abbreviate_path(&p);
        assert!(abbrev.starts_with("~/"));
    }

    #[test]
    fn test_relative_time() {
        let now = Utc::now();
        assert_eq!(relative_time(&now), "just now");

        let five_min_ago = now - chrono::Duration::minutes(5);
        assert_eq!(relative_time(&five_min_ago), "5 min ago");

        let yesterday = now - chrono::Duration::days(1);
        assert_eq!(relative_time(&yesterday), "yesterday");

        let three_days = now - chrono::Duration::days(3);
        assert_eq!(relative_time(&three_days), "3 days ago");
    }

    #[test]
    fn test_corrupted_registry_backup() {
        let (_tmp, path) = temp_registry();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "not valid toml [[[").unwrap();
        let reg = read_registry_from(&path);
        assert!(reg.projects.is_empty());
        // Backup should exist
        let bak = path.with_extension("toml.bak");
        assert!(bak.exists());
    }

    #[test]
    fn test_round_trip_serialization() {
        let (_tmp, path) = temp_registry();
        let mut reg = ProjectRegistry::default();
        reg.projects.push(ProjectEntry {
            name: "test".to_string(),
            path: "/tmp/test".to_string(),
            last_accessed_tui: Some(Utc::now()),
            last_accessed_cli: None,
        });
        write_registry_to(&path, &reg).unwrap();
        let loaded = read_registry_from(&path);
        assert_eq!(loaded.projects.len(), 1);
        assert_eq!(loaded.projects[0].name, "test");
        assert!(loaded.projects[0].last_accessed_tui.is_some());
        assert!(loaded.projects[0].last_accessed_cli.is_none());
    }
}
