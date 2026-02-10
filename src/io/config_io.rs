use std::fs;
use std::path::Path;

use crate::io::project_io::ProjectError;
use crate::model::config::{ProjectConfig, TrackConfig};

/// Read the project config, returning both the parsed config and the raw
/// toml_edit Document for round-trip-safe editing.
pub fn read_config(
    frame_dir: &Path,
) -> Result<(ProjectConfig, toml_edit::DocumentMut), ProjectError> {
    let config_path = frame_dir.join("project.toml");
    let config_text = fs::read_to_string(&config_path).map_err(|e| ProjectError::ReadError {
        path: config_path.clone(),
        source: e,
    })?;
    let config: ProjectConfig = toml::from_str(&config_text)?;
    let doc: toml_edit::DocumentMut = config_text.parse().map_err(|_: toml_edit::TomlError| {
        ProjectError::ConfigParseError(toml::from_str::<ProjectConfig>("").unwrap_err())
    })?;
    Ok((config, doc))
}

/// Write the config document back to disk, preserving formatting.
pub fn write_config(frame_dir: &Path, doc: &toml_edit::DocumentMut) -> Result<(), ProjectError> {
    let config_path = frame_dir.join("project.toml");
    fs::write(&config_path, doc.to_string()).map_err(|e| ProjectError::ReadError {
        path: config_path,
        source: e,
    })?;
    Ok(())
}

/// Update the cc_focus field in the config document
pub fn set_cc_focus(doc: &mut toml_edit::DocumentMut, track_id: &str) {
    if !doc.contains_key("agent") {
        doc["agent"] = toml_edit::Item::Table(toml_edit::Table::new());
    }
    doc["agent"]["cc_focus"] = toml_edit::value(track_id);
}

/// Clear the cc_focus field from the config document
pub fn clear_cc_focus(doc: &mut toml_edit::DocumentMut) {
    if let Some(agent) = doc.get_mut("agent").and_then(|a| a.as_table_mut()) {
        agent.remove("cc_focus");
    }
}

/// Add a new track to the config document
pub fn add_track_to_config(doc: &mut toml_edit::DocumentMut, track: &TrackConfig) {
    if !doc.contains_key("tracks") {
        doc["tracks"] = toml_edit::Item::ArrayOfTables(toml_edit::ArrayOfTables::new());
    }

    if let Some(tracks) = doc["tracks"].as_array_of_tables_mut() {
        let mut table = toml_edit::Table::new();
        table["id"] = toml_edit::value(&track.id);
        table["name"] = toml_edit::value(&track.name);
        table["state"] = toml_edit::value(&track.state);
        table["file"] = toml_edit::value(&track.file);
        tracks.push(table);
    }
}

/// Set an ID prefix for a track in the config document
pub fn set_prefix(doc: &mut toml_edit::DocumentMut, track_id: &str, prefix: &str) {
    if !doc.contains_key("ids") {
        doc["ids"] = toml_edit::Item::Table(toml_edit::Table::new());
    }
    let ids = doc["ids"].as_table_mut().unwrap();
    if !ids.contains_key("prefixes") {
        ids["prefixes"] = toml_edit::Item::Table(toml_edit::Table::new());
    }
    ids["prefixes"][track_id] = toml_edit::value(prefix);
}

/// Update a track's state in the config document
pub fn update_track_state(doc: &mut toml_edit::DocumentMut, track_id: &str, new_state: &str) {
    if let Some(tracks) = doc["tracks"].as_array_of_tables_mut() {
        for table in tracks.iter_mut() {
            if table.get("id").and_then(|v| v.as_str()) == Some(track_id) {
                table["state"] = toml_edit::value(new_state);
                break;
            }
        }
    }
}

/// Remove a track entry from the config document by id
pub fn remove_track_from_config(doc: &mut toml_edit::DocumentMut, track_id: &str) {
    if let Some(tracks) = doc["tracks"].as_array_of_tables_mut() {
        let mut idx_to_remove = None;
        for (i, table) in tracks.iter().enumerate() {
            if table.get("id").and_then(|v| v.as_str()) == Some(track_id) {
                idx_to_remove = Some(i);
                break;
            }
        }
        if let Some(idx) = idx_to_remove {
            tracks.remove(idx);
        }
    }
}

/// Update the name field of a track in the config document
pub fn update_track_name(doc: &mut toml_edit::DocumentMut, track_id: &str, new_name: &str) {
    if let Some(tracks) = doc["tracks"].as_array_of_tables_mut() {
        for table in tracks.iter_mut() {
            if table.get("id").and_then(|v| v.as_str()) == Some(track_id) {
                table["name"] = toml_edit::value(new_name);
                break;
            }
        }
    }
}

/// Update the id field of a track in the config document
pub fn update_track_id(doc: &mut toml_edit::DocumentMut, old_id: &str, new_id: &str) {
    if let Some(tracks) = doc["tracks"].as_array_of_tables_mut() {
        for table in tracks.iter_mut() {
            if table.get("id").and_then(|v| v.as_str()) == Some(old_id) {
                table["id"] = toml_edit::value(new_id);
                table["file"] = toml_edit::value(format!("tracks/{}.md", new_id));
                break;
            }
        }
    }
}

/// Remove an entry from [ids.prefixes]
pub fn remove_prefix(doc: &mut toml_edit::DocumentMut, track_id: &str) {
    if let Some(ids) = doc.get_mut("ids").and_then(|i| i.as_table_mut())
        && let Some(prefixes) = ids.get_mut("prefixes").and_then(|p| p.as_table_mut())
    {
        prefixes.remove(track_id);
    }
}

/// Move a prefix entry from old_key to new_key in [ids.prefixes]
pub fn rename_prefix_key(doc: &mut toml_edit::DocumentMut, old_key: &str, new_key: &str) {
    if let Some(ids) = doc.get_mut("ids").and_then(|i| i.as_table_mut())
        && let Some(prefixes) = ids.get_mut("prefixes").and_then(|p| p.as_table_mut())
        && let Some(value) = prefixes.get(old_key).cloned()
    {
        prefixes.remove(old_key);
        prefixes.insert(new_key, value);
    }
}

/// Set a tag color in [ui.tag_colors], creating the section if needed
pub fn set_tag_color(doc: &mut toml_edit::DocumentMut, tag: &str, hex_color: &str) {
    if !doc.contains_key("ui") {
        doc["ui"] = toml_edit::Item::Table(toml_edit::Table::new());
    }
    let ui = doc["ui"].as_table_mut().unwrap();
    if !ui.contains_key("tag_colors") {
        ui["tag_colors"] = toml_edit::Item::Table(toml_edit::Table::new());
    }
    ui["tag_colors"][tag] = toml_edit::value(hex_color);
}

/// Remove a tag color from [ui.tag_colors]
pub fn clear_tag_color(doc: &mut toml_edit::DocumentMut, tag: &str) {
    if let Some(ui) = doc.get_mut("ui").and_then(|u| u.as_table_mut())
        && let Some(tag_colors) = ui.get_mut("tag_colors").and_then(|tc| tc.as_table_mut())
    {
        tag_colors.remove(tag);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn sample_config() -> &'static str {
        r#"[project]
name = "test"

[agent]
cc_focus = "infra"

[[tracks]]
id = "effects"
name = "Effect System"
state = "active"
file = "tracks/effects.md"

[[tracks]]
id = "infra"
name = "Infrastructure"
state = "active"
file = "tracks/infra.md"
"#
    }

    #[test]
    fn test_round_trip_config() {
        let tmp = TempDir::new().unwrap();
        let frame_dir = tmp.path().join("frame");
        fs::create_dir_all(&frame_dir).unwrap();
        let config_path = frame_dir.join("project.toml");

        let original = sample_config();
        fs::write(&config_path, original).unwrap();

        let (_config, doc) = read_config(&frame_dir).unwrap();
        write_config(&frame_dir, &doc).unwrap();

        let written = fs::read_to_string(&config_path).unwrap();
        assert_eq!(written, original);
    }

    #[test]
    fn test_update_cc_focus() {
        let config_text = sample_config();
        let mut doc: toml_edit::DocumentMut = config_text.parse().unwrap();
        set_cc_focus(&mut doc, "effects");
        let result = doc.to_string();
        assert!(result.contains("cc_focus = \"effects\""));
    }

    #[test]
    fn test_update_track_state() {
        let config_text = sample_config();
        let mut doc: toml_edit::DocumentMut = config_text.parse().unwrap();
        update_track_state(&mut doc, "effects", "shelved");
        let result = doc.to_string();
        assert!(result.contains("state = \"shelved\""));
        // The infra track should still be active
        let config: ProjectConfig = toml::from_str(&result).unwrap();
        assert_eq!(config.tracks[1].state, "active");
    }

    #[test]
    fn test_add_track() {
        let config_text = sample_config();
        let mut doc: toml_edit::DocumentMut = config_text.parse().unwrap();
        add_track_to_config(
            &mut doc,
            &TrackConfig {
                id: "modules".to_string(),
                name: "Module System".to_string(),
                state: "active".to_string(),
                file: "tracks/modules.md".to_string(),
            },
        );
        let result = doc.to_string();
        let config: ProjectConfig = toml::from_str(&result).unwrap();
        assert_eq!(config.tracks.len(), 3);
        assert_eq!(config.tracks[2].id, "modules");
    }

    #[test]
    fn test_remove_track_from_config() {
        let config_text = sample_config();
        let mut doc: toml_edit::DocumentMut = config_text.parse().unwrap();
        remove_track_from_config(&mut doc, "effects");
        let result = doc.to_string();
        let config: ProjectConfig = toml::from_str(&result).unwrap();
        assert_eq!(config.tracks.len(), 1);
        assert_eq!(config.tracks[0].id, "infra");
    }

    #[test]
    fn test_update_track_name() {
        let config_text = sample_config();
        let mut doc: toml_edit::DocumentMut = config_text.parse().unwrap();
        update_track_name(&mut doc, "effects", "New Effects");
        let result = doc.to_string();
        let config: ProjectConfig = toml::from_str(&result).unwrap();
        assert_eq!(config.tracks[0].name, "New Effects");
        assert_eq!(config.tracks[1].name, "Infrastructure");
    }

    #[test]
    fn test_update_track_id() {
        let config_text = sample_config();
        let mut doc: toml_edit::DocumentMut = config_text.parse().unwrap();
        update_track_id(&mut doc, "effects", "fx");
        let result = doc.to_string();
        let config: ProjectConfig = toml::from_str(&result).unwrap();
        assert_eq!(config.tracks[0].id, "fx");
        assert_eq!(config.tracks[0].file, "tracks/fx.md");
    }

    #[test]
    fn test_remove_prefix() {
        let config_text = r#"[project]
name = "test"

[ids.prefixes]
effects = "EFF"
infra = "INF"

[[tracks]]
id = "effects"
name = "Effects"
state = "active"
file = "tracks/effects.md"
"#;
        let mut doc: toml_edit::DocumentMut = config_text.parse().unwrap();
        remove_prefix(&mut doc, "effects");
        let result = doc.to_string();
        assert!(!result.contains("effects = \"EFF\""));
        assert!(result.contains("infra = \"INF\""));
    }

    #[test]
    fn test_rename_prefix_key() {
        let config_text = r#"[project]
name = "test"

[ids.prefixes]
effects = "EFF"

[[tracks]]
id = "effects"
name = "Effects"
state = "active"
file = "tracks/effects.md"
"#;
        let mut doc: toml_edit::DocumentMut = config_text.parse().unwrap();
        rename_prefix_key(&mut doc, "effects", "fx");
        let result = doc.to_string();
        assert!(!result.contains("effects = \"EFF\""));
        assert!(result.contains("fx = \"EFF\""));
    }

    #[test]
    fn test_set_tag_color_creates_section() {
        let config_text = r#"[project]
name = "test"

[[tracks]]
id = "effects"
name = "Effects"
state = "active"
file = "tracks/effects.md"
"#;
        let mut doc: toml_edit::DocumentMut = config_text.parse().unwrap();
        set_tag_color(&mut doc, "bug", "#FF4444");
        let result = doc.to_string();
        assert!(result.contains("[ui.tag_colors]"));
        assert!(result.contains("bug = \"#FF4444\""));
    }

    #[test]
    fn test_set_tag_color_existing_section() {
        let config_text = r##"[project]
name = "test"

[ui.tag_colors]
bug = "#FF4444"

[[tracks]]
id = "effects"
name = "Effects"
state = "active"
file = "tracks/effects.md"
"##;
        let mut doc: toml_edit::DocumentMut = config_text.parse().unwrap();
        set_tag_color(&mut doc, "design", "#44DDFF");
        set_tag_color(&mut doc, "bug", "#CC66FF");
        let result = doc.to_string();
        assert!(result.contains(r##"design = "#44DDFF""##));
        assert!(result.contains(r##"bug = "#CC66FF""##));
    }

    #[test]
    fn test_clear_tag_color() {
        let config_text = r##"[project]
name = "test"

[ui.tag_colors]
bug = "#FF4444"
design = "#44DDFF"

[[tracks]]
id = "effects"
name = "Effects"
state = "active"
file = "tracks/effects.md"
"##;
        let mut doc: toml_edit::DocumentMut = config_text.parse().unwrap();
        clear_tag_color(&mut doc, "bug");
        let result = doc.to_string();
        assert!(!result.contains("bug"));
        assert!(result.contains(r##"design = "#44DDFF""##));
    }

    #[test]
    fn test_clear_tag_color_nonexistent() {
        let config_text = r#"[project]
name = "test"
"#;
        let mut doc: toml_edit::DocumentMut = config_text.parse().unwrap();
        // Should not panic
        clear_tag_color(&mut doc, "bug");
    }
}
