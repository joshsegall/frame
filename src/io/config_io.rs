use std::fs;
use std::path::Path;

use crate::io::project_io::ProjectError;
use crate::model::config::{ProjectConfig, TrackConfig};

/// Read the project config, returning both the parsed config and the raw
/// toml_edit Document for round-trip-safe editing.
pub fn read_config(frame_dir: &Path) -> Result<(ProjectConfig, toml_edit::DocumentMut), ProjectError> {
    let config_path = frame_dir.join("project.toml");
    let config_text = fs::read_to_string(&config_path).map_err(|e| ProjectError::ReadError {
        path: config_path.clone(),
        source: e,
    })?;
    let config: ProjectConfig = toml::from_str(&config_text)?;
    let doc: toml_edit::DocumentMut = config_text
        .parse()
        .map_err(|_: toml_edit::TomlError| ProjectError::ConfigParseError(
            toml::from_str::<ProjectConfig>("").unwrap_err(),
        ))?;
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
}
