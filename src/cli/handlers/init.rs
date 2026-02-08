use std::fs;

use crate::cli::commands::InitArgs;
use crate::io::project_io;
use crate::ops::track_ops::generate_prefix;

const PROJECT_TOML_TEMPLATE: &str = r##"[project]
name = "{name}"

[agent]
cc_focus = ""
default_tags = ["cc-added"]

[clean]
auto_clean = true
done_threshold = 250
archive_per_track = true

# Directories to search for spec: and ref: path validation and autocomplete.
# Paths are relative to the project root (parent of frame/).
ref_paths = ["doc", "spec", "docs", "design", "papers"]

# --- Tracks ---
# Add tracks with [[tracks]] entries, or use: fr track new <id> "name"
#
# [[tracks]]
# id = "example"
# name = "Example Track"
# state = "active"
# file = "tracks/example.md"

# --- ID Prefixes ---
# Map track IDs to uppercase prefixes for task numbering.
# Auto-generated when tracks are created. Edit freely.
#
# [ids.prefixes]
# example = "EXA"

# --- UI Customization ---
# Uncomment and edit to override defaults.

[ui]
# # search only these directories for references and spec autocomplete
ref_paths = ["doc", "spec", "docs", "design", "papers"]

# # include only these file extensions in refs and spec autocomplete
# ref_extensions = ["md", "txt", "rst", "pdf", "toml", "yaml"]
# show_key_hints = false
# tag_style = "foreground"        # "foreground" or "pill"
#
# [ui.colors]
# background = "#0C001B"
# text = "#A09BFE"
# text_bright = "#FFFFFF"
# highlight = "#FB4196"
# dim = "#5A5580"
# red = "#FF4444"
# yellow = "#FFD700"
# green = "#44FF88"
# cyan = "#44DDFF"
#
# [ui.tag_colors]
# research = "#4488FF"
# design = "#44DDFF"
# bug = "#FF4444"
# cc = "#44FF88"
# cc-added = "#CC66FF"
# needs-input = "#FFD700"
"##;

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

/// Render project.toml with tracks and prefixes replacing the commented examples.
fn render_project_toml(
    name: &str,
    tracks: &[(&str, &str)],
    prefixes: &[(String, String)],
) -> String {
    let base = PROJECT_TOML_TEMPLATE.replace("{name}", name);

    if tracks.is_empty() {
        return base;
    }

    // Build track entries
    let mut track_section = String::new();
    for (id, tname) in tracks {
        track_section.push_str(&format!(
            "\n[[tracks]]\nid = \"{}\"\nname = \"{}\"\nstate = \"active\"\nfile = \"tracks/{}.md\"\n",
            id, tname, id
        ));
    }

    // Build prefix entries
    let mut prefix_section = String::new();
    prefix_section.push_str("\n[ids.prefixes]\n");
    for (id, pfx) in prefixes {
        prefix_section.push_str(&format!("{} = \"{}\"\n", id, pfx));
    }

    // Replace commented track section with real entries
    let result = base.replace(
        "# --- Tracks ---\n# Add tracks with [[tracks]] entries, or use: fr track new <id> \"name\"\n#\n# [[tracks]]\n# id = \"example\"\n# name = \"Example Track\"\n# state = \"active\"\n# file = \"tracks/example.md\"",
        &format!("# --- Tracks ---{}", track_section.trim_end()),
    );

    // Replace commented prefix section with real entries
    result.replace(
        "# --- ID Prefixes ---\n# Map track IDs to uppercase prefixes for task numbering.\n# Auto-generated when tracks are created. Edit freely.\n#\n# [ids.prefixes]\n# example = \"EXA\"",
        &format!("# --- ID Prefixes ---\n# Map track IDs to uppercase prefixes for task numbering.\n# Auto-generated when tracks are created. Edit freely.\n{}", prefix_section.trim_end()),
    )
}

pub fn cmd_init(args: InitArgs) -> Result<(), Box<dyn std::error::Error>> {
    let cwd = std::env::current_dir()?;
    let frame_dir = cwd.join("frame");

    // Check if already initialized
    if frame_dir.is_dir() {
        return Err("Frame project already exists in ./frame/".into());
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

    // Print summary
    println!("Initialized Frame project: {}", name);
    if !track_pairs.is_empty() {
        for (id, tname) in &track_pairs {
            let pfx = prefixes.iter().find(|(pid, _)| pid == id).unwrap();
            println!("  track: {} ({}) [{}]", tname, id, pfx.1);
        }
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
    fn test_render_project_toml_no_tracks() {
        let result = render_project_toml("Test Project", &[], &[]);
        assert!(result.contains("name = \"Test Project\""));
        assert!(result.contains("# [[tracks]]"));
        assert!(result.contains("# [ids.prefixes]"));
    }

    #[test]
    fn test_render_project_toml_with_tracks() {
        let tracks = vec![("effects", "Effect System"), ("infra", "Infrastructure")];
        let prefixes = vec![
            ("effects".to_string(), "EFF".to_string()),
            ("infra".to_string(), "INF".to_string()),
        ];
        let result = render_project_toml("Test", &tracks, &prefixes);
        assert!(result.contains("id = \"effects\""));
        assert!(result.contains("name = \"Effect System\""));
        assert!(result.contains("file = \"tracks/effects.md\""));
        assert!(result.contains("id = \"infra\""));
        assert!(result.contains("[ids.prefixes]"));
        assert!(result.contains("effects = \"EFF\""));
        assert!(result.contains("infra = \"INF\""));
        // Should NOT contain commented examples
        assert!(!result.contains("# id = \"example\""));
        assert!(!result.contains("# example = \"EXA\""));
    }
}
