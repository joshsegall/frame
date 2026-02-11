use std::fs;

use crate::cli::commands::InitArgs;
use crate::io::project_io;
use crate::ops::track_ops::generate_prefix;

const PROJECT_TOML_TEMPLATE: &str = include_str!("../../templates/project.toml");

const INBOX_TEMPLATE: &str = "# Inbox\n";

const TRACK_TEMPLATE: &str = "# {name}\n\n> \n\n## Backlog\n\n## Parked\n\n## Done\n";

/// Validate that a track ID is lowercase alphanumeric with hyphens only.
fn validate_track_id(id: &str) -> Result<(), String> {
    if id.is_empty() {
        return Err("track id cannot be empty".to_string());
    }
    if !id
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        return Err(format!(
            "invalid track id \"{}\" — use lowercase with hyphens (e.g. \"my-track\")",
            id
        ));
    }
    Ok(())
}

/// Infer a project name from a directory name: replace hyphens with spaces, title-case.
fn infer_name(dir_name: &str) -> String {
    dir_name
        .split('-')
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                None => String::new(),
                Some(c) => {
                    let upper: String = c.to_uppercase().collect();
                    upper + &chars.collect::<String>()
                }
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Parse --track pairs from the flat Vec<String> produced by clap.
/// Each pair is (id, name).
fn parse_track_pairs(args: &[String]) -> Vec<(&str, &str)> {
    args.chunks(2)
        .filter_map(|chunk| {
            if chunk.len() == 2 {
                Some((chunk[0].as_str(), chunk[1].as_str()))
            } else {
                None
            }
        })
        .collect()
}

/// Render project.toml from the embedded template.
///
/// Replaces `{{PROJECT_NAME}}` with the project name. If tracks are provided,
/// appends `[[tracks]]` entries and `[ids.prefixes]` table at the end.
fn render_project_toml(
    name: &str,
    tracks: &[(&str, &str)],
    prefixes: &[(String, String)],
) -> String {
    let mut output = PROJECT_TOML_TEMPLATE.replace("{{PROJECT_NAME}}", name);

    if tracks.is_empty() {
        return output;
    }

    // Append track entries
    for (id, tname) in tracks {
        output.push_str(&format!(
            "\n[[tracks]]\nid = \"{}\"\nname = \"{}\"\nstate = \"active\"\nfile = \"tracks/{}.md\"\n",
            id, tname, id
        ));
    }

    // Append prefix table
    output.push_str("\n[ids.prefixes]\n");
    for (id, pfx) in prefixes {
        output.push_str(&format!("{} = \"{}\"\n", id, pfx));
    }

    output
}

/// Add Frame-specific entries to .gitignore if a git repo exists.
/// Returns true if entries were actually added.
fn update_gitignore(cwd: &std::path::Path) -> bool {
    // Only act if this is a git repo (has .git dir or file)
    if !cwd.join(".git").exists() {
        return false;
    }

    let gitignore_path = cwd.join(".gitignore");
    let existing = fs::read_to_string(&gitignore_path).unwrap_or_default();

    let entries = ["frame/.state.json", "frame/.lock", "frame/.recovery.log"];
    let mut to_add = Vec::new();
    for entry in &entries {
        if !existing.lines().any(|line| line.trim() == *entry) {
            to_add.push(*entry);
        }
    }

    if to_add.is_empty() {
        return false;
    }

    let mut content = existing;
    // Ensure we start on a fresh line
    if !content.is_empty() && !content.ends_with('\n') {
        content.push('\n');
    }
    content.push_str("\n# frame (added by fr init)\n");
    for entry in &to_add {
        content.push_str(entry);
        content.push('\n');
    }

    fs::write(&gitignore_path, content).is_ok()
}

pub fn cmd_init(args: InitArgs) -> Result<(), Box<dyn std::error::Error>> {
    let cwd = std::env::current_dir()?;
    let frame_dir = cwd.join("frame");

    // Check if already initialized
    if frame_dir.is_dir() && !args.force {
        return Err("frame/ already exists (use --force to reinitialize)".into());
    }

    // Check for parent project and warn
    if let Some(parent) = cwd.parent()
        && let Ok(parent_root) = project_io::discover_project(parent)
    {
        let parent_frame = parent_root.join("frame");
        eprintln!("Note: parent project found at {}/", parent_frame.display());
        eprintln!("Creating new project in ./frame/");
    }

    // Parse track pairs and validate IDs
    let track_pairs = parse_track_pairs(&args.track);
    for (id, _) in &track_pairs {
        validate_track_id(id)?;
    }

    // Check for duplicate track IDs
    let mut seen_ids = std::collections::HashSet::new();
    for (id, _) in &track_pairs {
        if !seen_ids.insert(*id) {
            return Err(format!("duplicate track id \"{}\"", id).into());
        }
    }

    // Infer project name
    let name = args.name.unwrap_or_else(|| {
        cwd.file_name()
            .and_then(|n| n.to_str())
            .map(infer_name)
            .unwrap_or_else(|| "Untitled".to_string())
    });

    // Generate prefixes for tracks
    let mut prefixes = Vec::new();
    let mut existing_prefixes: Vec<String> = Vec::new();
    for (id, _) in &track_pairs {
        let pfx = generate_prefix(id, &existing_prefixes);
        existing_prefixes.push(pfx.clone());
        prefixes.push((id.to_string(), pfx));
    }

    // Create directory structure
    fs::create_dir_all(frame_dir.join("tracks"))?;
    fs::create_dir_all(frame_dir.join("archive"))?;

    // Write project.toml
    let toml_content = render_project_toml(&name, &track_pairs, &prefixes);
    fs::write(frame_dir.join("project.toml"), toml_content)?;

    // Write inbox.md
    fs::write(frame_dir.join("inbox.md"), INBOX_TEMPLATE)?;

    // Create track files
    for (id, tname) in &track_pairs {
        let content = TRACK_TEMPLATE.replace("{name}", tname);
        fs::write(frame_dir.join(format!("tracks/{}.md", id)), content)?;
    }

    // Register in global project registry
    crate::io::registry::register_project(&name, &cwd);

    // Update .gitignore
    let gitignore_updated = update_gitignore(&cwd);

    // Print summary
    println!("[>] frame initialized");
    println!();
    println!("  project.toml");
    println!("  inbox.md");
    for (id, _) in &track_pairs {
        println!("  tracks/{}.md", id);
    }

    if gitignore_updated {
        println!();
        println!("  added frame/.state.json, frame/.lock to .gitignore");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_track_id_valid() {
        assert!(validate_track_id("effects").is_ok());
        assert!(validate_track_id("compiler-infra").is_ok());
        assert!(validate_track_id("v2").is_ok());
        assert!(validate_track_id("my-cool-track").is_ok());
    }

    #[test]
    fn test_validate_track_id_invalid() {
        assert!(validate_track_id("My Track").is_err());
        assert!(validate_track_id("UPPER").is_err());
        assert!(validate_track_id("under_score").is_err());
        assert!(validate_track_id("").is_err());
    }

    #[test]
    fn test_infer_name() {
        assert_eq!(infer_name("my-cool-project"), "My Cool Project");
        assert_eq!(infer_name("frame"), "Frame");
        assert_eq!(infer_name("lace-compiler"), "Lace Compiler");
    }

    #[test]
    fn test_parse_track_pairs() {
        let args = vec![
            "effects".to_string(),
            "Effect System".to_string(),
            "infra".to_string(),
            "Infrastructure".to_string(),
        ];
        let pairs = parse_track_pairs(&args);
        assert_eq!(pairs.len(), 2);
        assert_eq!(pairs[0], ("effects", "Effect System"));
        assert_eq!(pairs[1], ("infra", "Infrastructure"));
    }

    #[test]
    fn test_template_embedding() {
        assert!(!PROJECT_TOML_TEMPLATE.is_empty());
        assert!(PROJECT_TOML_TEMPLATE.contains("{{PROJECT_NAME}}"));
    }

    #[test]
    fn test_render_project_toml_no_tracks() {
        let result = render_project_toml("My Project", &[], &[]);
        assert!(result.contains("name = \"My Project\""));
        assert!(!result.contains("{{PROJECT_NAME}}"));
        // Template has commented-out examples (# [[tracks]]), but no real entries
        assert!(!result.contains("\n[[tracks]]"));
        assert!(!result.contains("\n[ids.prefixes]"));
        assert!(result.contains("[clean]"));
        assert!(result.contains("[ui]"));
    }

    #[test]
    fn test_render_project_toml_with_tracks() {
        let tracks = vec![("api", "API Layer"), ("ui", "UI")];
        let prefixes = vec![
            ("api".to_string(), "API".to_string()),
            ("ui".to_string(), "UI".to_string()),
        ];
        let result = render_project_toml("Test", &tracks, &prefixes);
        assert!(result.contains("[[tracks]]"));
        assert!(result.contains("id = \"api\""));
        assert!(result.contains("name = \"API Layer\""));
        assert!(result.contains("file = \"tracks/api.md\""));
        assert!(result.contains("id = \"ui\""));
        assert!(result.contains("name = \"UI\""));
        assert!(result.contains("[ids.prefixes]"));
        assert!(result.contains("api = \"API\""));
        assert!(result.contains("ui = \"UI\""));
    }

    #[test]
    fn test_render_prefix_collision() {
        // "api" and "app" would both naively get "API" — generate_prefix handles this
        let tracks = vec![("api", "API Service"), ("app", "Application")];
        let mut existing_prefixes: Vec<String> = Vec::new();
        let mut prefixes = Vec::new();
        for (id, _) in &tracks {
            let pfx = generate_prefix(id, &existing_prefixes);
            existing_prefixes.push(pfx.clone());
            prefixes.push((id.to_string(), pfx));
        }
        // Prefixes must be distinct
        assert_ne!(prefixes[0].1, prefixes[1].1);

        let result = render_project_toml("Test", &tracks, &prefixes);
        assert!(result.contains(&format!("api = \"{}\"", prefixes[0].1)));
        assert!(result.contains(&format!("app = \"{}\"", prefixes[1].1)));
    }

    #[test]
    fn test_render_round_trip_no_tracks() {
        let result = render_project_toml("Round Trip", &[], &[]);
        let parsed: crate::model::config::ProjectConfig = toml::from_str(&result).unwrap();
        assert_eq!(parsed.project.name, "Round Trip");
    }

    #[test]
    fn test_render_round_trip_with_tracks() {
        let tracks = vec![("api", "API Layer"), ("ui", "UI")];
        let prefixes = vec![
            ("api".to_string(), "API".to_string()),
            ("ui".to_string(), "UI".to_string()),
        ];
        let result = render_project_toml("Round Trip", &tracks, &prefixes);
        let parsed: crate::model::config::ProjectConfig = toml::from_str(&result).unwrap();
        assert_eq!(parsed.project.name, "Round Trip");
        assert_eq!(parsed.tracks.len(), 2);
        assert_eq!(parsed.tracks[0].id, "api");
        assert_eq!(parsed.tracks[1].id, "ui");
        assert_eq!(parsed.ids.prefixes.get("api").unwrap(), "API");
        assert_eq!(parsed.ids.prefixes.get("ui").unwrap(), "UI");
    }

    #[test]
    fn test_update_gitignore_no_git() {
        let tmp = tempfile::TempDir::new().unwrap();
        // No .git dir — should return false
        assert!(!update_gitignore(tmp.path()));
        assert!(!tmp.path().join(".gitignore").exists());
    }

    #[test]
    fn test_update_gitignore_creates_entries() {
        let tmp = tempfile::TempDir::new().unwrap();
        fs::create_dir(tmp.path().join(".git")).unwrap();

        assert!(update_gitignore(tmp.path()));

        let content = fs::read_to_string(tmp.path().join(".gitignore")).unwrap();
        assert!(content.contains("frame/.state.json"));
        assert!(content.contains("frame/.lock"));
        assert!(content.contains("# frame (added by fr init)"));
    }

    #[test]
    fn test_update_gitignore_skips_existing() {
        let tmp = tempfile::TempDir::new().unwrap();
        fs::create_dir(tmp.path().join(".git")).unwrap();
        fs::write(
            tmp.path().join(".gitignore"),
            "frame/.state.json\nframe/.lock\nframe/.recovery.log\n",
        )
        .unwrap();

        assert!(!update_gitignore(tmp.path()));
    }

    #[test]
    fn test_update_gitignore_appends_to_existing() {
        let tmp = tempfile::TempDir::new().unwrap();
        fs::create_dir(tmp.path().join(".git")).unwrap();
        fs::write(tmp.path().join(".gitignore"), "*.log\n").unwrap();

        assert!(update_gitignore(tmp.path()));

        let content = fs::read_to_string(tmp.path().join(".gitignore")).unwrap();
        assert!(content.starts_with("*.log\n"));
        assert!(content.contains("frame/.state.json"));
        assert!(content.contains("frame/.lock"));
    }
}
