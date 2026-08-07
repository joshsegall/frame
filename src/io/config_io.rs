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
    let content = doc.to_string();
    crate::io::recovery::atomic_write(&config_path, content.as_bytes()).map_err(|e| {
        ProjectError::ReadError {
            path: config_path,
            source: e,
        }
    })?;
    Ok(())
}

/// Write config from the in-memory struct (no toml_edit document available).
/// Uses `toml::to_string_pretty` — key order is stable (IndexMap insertion order)
/// but comments and non-standard formatting are not preserved.
pub fn write_config_from_struct(
    frame_dir: &Path,
    config: &ProjectConfig,
) -> Result<(), ProjectError> {
    let config_path = frame_dir.join("project.toml");
    let text = toml::to_string_pretty(config)?;
    crate::io::recovery::atomic_write(&config_path, text.as_bytes()).map_err(|e| {
        ProjectError::ReadError {
            path: config_path,
            source: e,
        }
    })?;
    Ok(())
}

/// Set a key's value, keeping whatever was written around the old one.
///
/// `toml_edit::value` builds a fresh value with default decor, and a value's
/// decor is where its **trailing comment** lives. So the obvious
/// `table["k"] = value(v)` silently deletes the explanation next to the setting
/// it is changing — `cc_focus = ""  # track ID for 'fr ready --cc'` came back
/// as a bare `cc_focus = ""`, and every state change on a track would have
/// taken that row's comment the same way.
///
/// Every overwrite of an existing key here goes through this. Creating a key
/// that was not there gets the default decor, which is correct: there is
/// nothing to preserve.
fn set_keeping_decor(table: &mut toml_edit::Table, key: &str, value: &str) {
    let decor = table
        .get(key)
        .and_then(|item| item.as_value())
        .map(|v| v.decor().clone());
    table[key] = toml_edit::value(value);
    if let Some(decor) = decor
        && let Some(new) = table.get_mut(key).and_then(|item| item.as_value_mut())
    {
        *new.decor_mut() = decor;
    }
}

/// Update the cc_focus field in the config document
pub fn set_cc_focus(doc: &mut toml_edit::DocumentMut, track_id: &str) {
    if !doc.contains_key("agent") {
        doc["agent"] = toml_edit::Item::Table(toml_edit::Table::new());
    }
    if let Some(agent) = doc.get_mut("agent").and_then(|a| a.as_table_mut()) {
        set_keeping_decor(agent, "cc_focus", track_id);
    }
}

/// Clear the cc_focus field in the config document.
///
/// The key is emptied rather than removed, which is the file's own idiom: the
/// shipped template writes `cc_focus = ""` and documents empty as meaning none,
/// and [`crate::model::config::AgentConfig`] reads it back as `None`.
///
/// Removing it instead cost two things. The line's trailing comment went with
/// it — the same loss this whole merge exists to prevent, in miniature. And
/// setting focus again could only re-add the key at the *end* of `[agent]`,
/// because a removed key takes its position with it, so focusing a track and
/// undoing left `project.toml` reordered. P9 caught exactly that.
///
/// A key that is not there is left alone: clearing is not a reason to write a
/// setting into a file that never mentioned it.
pub fn clear_cc_focus(doc: &mut toml_edit::DocumentMut) {
    if let Some(agent) = doc.get_mut("agent").and_then(|a| a.as_table_mut())
        && agent.contains_key("cc_focus")
    {
        set_keeping_decor(agent, "cc_focus", "");
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

/// Overwrite one field of a track's entry, leaving the rest of the row alone.
///
/// The named-field helpers above each rewrite one key; this is the same move
/// with the key as data, which is what a field-by-field merge needs
/// ([`crate::ops::reconcile::reconcile_config`]) — it decides per field which
/// side won, and cannot enumerate them at the call site.
pub fn set_track_field(doc: &mut toml_edit::DocumentMut, track_id: &str, field: &str, value: &str) {
    if let Some(tracks) = doc
        .get_mut("tracks")
        .and_then(|t| t.as_array_of_tables_mut())
    {
        for table in tracks.iter_mut() {
            if table.get("id").and_then(|v| v.as_str()) == Some(track_id) {
                set_keeping_decor(table, field, value);
                break;
            }
        }
    }
}

/// Insert a track's entry at a position rather than at the end.
///
/// [`add_track_to_config`] appends, which is right for `fr track new` and wrong
/// for the TUI, where `p`/`-` place a new track among the active ones. Indices
/// past the end append.
pub fn insert_track_in_config(doc: &mut toml_edit::DocumentMut, track: &TrackConfig, index: usize) {
    add_track_to_config(doc, track);
    if let Some(tracks) = doc
        .get_mut("tracks")
        .and_then(|t| t.as_array_of_tables_mut())
    {
        let last = tracks.len().saturating_sub(1);
        if index < last {
            let mut tables: Vec<toml_edit::Table> = tracks.iter().cloned().collect();
            let table = tables.remove(last);
            tables.insert(index, table);
            rebuild_tracks(tracks, tables);
        }
    }
}

/// Reorder the `[[tracks]]` entries to match `order`, matching on id.
///
/// Entries are moved whole, so each row's own comments travel with it. An id in
/// the document that `order` does not mention keeps its position relative to the
/// other unmentioned ones, at the end — that is how a track another process
/// added survives a reorder we computed without knowing about it.
pub fn set_track_order(doc: &mut toml_edit::DocumentMut, order: &[String]) {
    let Some(tracks) = doc
        .get_mut("tracks")
        .and_then(|t| t.as_array_of_tables_mut())
    else {
        return;
    };
    let mut pool: Vec<toml_edit::Table> = tracks.iter().cloned().collect();
    let mut ordered: Vec<toml_edit::Table> = Vec::with_capacity(pool.len());
    for id in order {
        if let Some(pos) = pool
            .iter()
            .position(|t| t.get("id").and_then(|v| v.as_str()) == Some(id.as_str()))
        {
            ordered.push(pool.remove(pos));
        }
    }
    ordered.extend(pool);
    rebuild_tracks(tracks, ordered);
}

/// Replace the array's entries, each carrying its own comments with it.
///
/// **Decor travels with the row, and nothing is reassigned by position.** That
/// costs something visible: a comment block sitting above the first
/// `[[tracks]]` entry is as likely to introduce the *section* as to describe
/// that row — the shipped template's Tracks banner is exactly that, having no
/// entry of its own to attach to — so inserting a track at the top leaves the
/// banner above the second row instead.
///
/// It was written the other way first, pinning that leading block to the first
/// position and swapping the displaced one out. P9 rejected it, and was right
/// to: a swap is only its own inverse when the *same* operation runs twice.
/// Adding a track at position 0 and then undoing it is an insert followed by a
/// removal, and the blank line the swap moved off the original first row never
/// came back — `project.toml` did not survive undo byte for byte.
///
/// Attaching decor to rows is reversible under every operation here, insert,
/// remove and reorder alike, because no rule has to run backwards for it to
/// hold. A banner one row lower is cosmetic and self-correcting the moment the
/// user moves it; an undo that does not restore the file is neither.
fn rebuild_tracks(tracks: &mut toml_edit::ArrayOfTables, tables: Vec<toml_edit::Table>) {
    let mut tables = tables;

    // A table carries where in the document it is rendered, and that is what
    // decides the output — reordering the array alone moves nothing. So the
    // positions the entries currently occupy are collected and handed back out
    // in the new order, which keeps the block of `[[tracks]]` entries exactly
    // where it was in the file and only changes who sits in each slot.
    let mut slots: Vec<usize> = tracks.iter().filter_map(|t| t.position()).collect();
    slots.sort_unstable();
    for (table, slot) in tables.iter_mut().zip(slots) {
        table.set_position(slot);
    }

    let mut rebuilt = toml_edit::ArrayOfTables::new();
    for table in tables {
        rebuilt.push(table);
    }
    *tracks = rebuilt;
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
    if let Some(prefixes) = ids.get_mut("prefixes").and_then(|p| p.as_table_mut()) {
        set_keeping_decor(prefixes, track_id, prefix);
    }
}

/// Update a track's state in the config document
pub fn update_track_state(doc: &mut toml_edit::DocumentMut, track_id: &str, new_state: &str) {
    if let Some(tracks) = doc["tracks"].as_array_of_tables_mut() {
        for table in tracks.iter_mut() {
            if table.get("id").and_then(|v| v.as_str()) == Some(track_id) {
                set_keeping_decor(table, "state", new_state);
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
                set_keeping_decor(table, "name", new_name);
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
                set_keeping_decor(table, "id", new_id);
                set_keeping_decor(table, "file", &format!("tracks/{}.md", new_id));
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
    if let Some(tag_colors) = ui.get_mut("tag_colors").and_then(|t| t.as_table_mut()) {
        set_keeping_decor(tag_colors, tag, hex_color);
    }
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

    const BANNERED: &str = r#"[project]
name = "test"

# Tracks
# ------
# Each entry defines a workstream.

[[tracks]]
id = "api"
name = "API"
state = "active"
file = "tracks/api.md"

# the one nobody uses
[[tracks]]
id = "ui"
name = "UI"
state = "active"
file = "tracks/ui.md"
"#;

    fn track_ids(text: &str) -> Vec<String> {
        let config: ProjectConfig = toml::from_str(text).unwrap();
        config.tracks.into_iter().map(|t| t.id).collect()
    }

    /// Reordering moves rows, and each row's comments go with it.
    #[test]
    fn test_set_track_order_moves_rows_and_their_comments() {
        let mut doc: toml_edit::DocumentMut = BANNERED.parse().unwrap();
        set_track_order(&mut doc, &["ui".to_string(), "api".to_string()]);
        let result = doc.to_string();

        assert_eq!(track_ids(&result), vec!["ui", "api"]);
        // The comment written above `ui` travelled with `ui`, which is now
        // first — so it now sits above the whole section.
        let comment = result.find("# the one nobody uses").unwrap();
        let ui_row = result.find(r#"id = "ui""#).unwrap();
        let api_row = result.find(r#"id = "api""#).unwrap();
        assert!(comment < ui_row && ui_row < api_row);
        // Nothing outside the array moved.
        assert!(result.starts_with("[project]"));
        assert!(result.contains("# Tracks"));
    }

    /// The property that decides how decor is handled: every operation here is
    /// reversible, so an undo restores `project.toml` byte for byte. Pinning a
    /// comment block to a *position* instead of to its row fails this — P9
    /// found it, on an insert undone by a removal.
    #[test]
    fn test_track_edits_are_reversible_byte_for_byte() {
        let mut doc: toml_edit::DocumentMut = BANNERED.parse().unwrap();
        insert_track_in_config(
            &mut doc,
            &TrackConfig {
                id: "docs".to_string(),
                name: "Docs".to_string(),
                state: "active".to_string(),
                file: "tracks/docs.md".to_string(),
            },
            0,
        );
        remove_track_from_config(&mut doc, "docs");
        assert_eq!(doc.to_string(), BANNERED, "insert then remove must restore");

        let mut doc: toml_edit::DocumentMut = BANNERED.parse().unwrap();
        set_track_order(&mut doc, &["ui".to_string(), "api".to_string()]);
        set_track_order(&mut doc, &["api".to_string(), "ui".to_string()]);
        assert_eq!(doc.to_string(), BANNERED, "a reorder undone must restore");
    }

    #[test]
    fn test_insert_track_in_config_places_rather_than_appends() {
        let mut doc: toml_edit::DocumentMut = BANNERED.parse().unwrap();
        insert_track_in_config(
            &mut doc,
            &TrackConfig {
                id: "docs".to_string(),
                name: "Docs".to_string(),
                state: "active".to_string(),
                file: "tracks/docs.md".to_string(),
            },
            1,
        );
        let result = doc.to_string();
        assert_eq!(track_ids(&result), vec!["api", "docs", "ui"]);
        assert!(result.contains("# Tracks"));
    }

    /// A setting's trailing comment explains the setting. Changing the value
    /// must not take the explanation with it — which is what assigning through
    /// `toml_edit::value` does, since a value's decor is where that comment
    /// lives. Caught by smoke-testing `fr track cc-focus --clear` against the
    /// shipped template, after the code claimed to have fixed it.
    #[test]
    fn test_setting_a_value_keeps_its_trailing_comment() {
        let text = r#"[project]
name = "test"

[agent]
cc_focus = "api"             # track ID for `fr ready --cc` (empty = none)
cc_only = true

[[tracks]]
id = "api"
name = "API"                 # the display name
state = "active"
file = "tracks/api.md"

[ids.prefixes]
api = "API"                  # task ids look like API-001
"#;
        let mut doc: toml_edit::DocumentMut = text.parse().unwrap();
        clear_cc_focus(&mut doc);
        update_track_state(&mut doc, "api", "shelved");
        update_track_name(&mut doc, "api", "The API");
        set_prefix(&mut doc, "api", "AP");
        let result = doc.to_string();

        assert!(result.contains("# track ID for `fr ready --cc` (empty = none)"));
        assert!(result.contains("# the display name"));
        assert!(result.contains("# task ids look like API-001"));
        // And the values really did change.
        let config: ProjectConfig = toml::from_str(&result).unwrap();
        assert!(config.agent.cc_focus.is_none());
        assert_eq!(config.tracks[0].state, "shelved");
        assert_eq!(config.tracks[0].name, "The API");
        assert_eq!(config.ids.prefixes.get("api").unwrap(), "AP");
    }

    #[test]
    fn test_set_track_field_leaves_the_rest_of_the_row() {
        let mut doc: toml_edit::DocumentMut = BANNERED.parse().unwrap();
        set_track_field(&mut doc, "ui", "state", "shelved");
        let config: ProjectConfig = toml::from_str(&doc.to_string()).unwrap();
        assert_eq!(config.tracks[1].state, "shelved");
        assert_eq!(config.tracks[1].name, "UI");
        assert_eq!(config.tracks[0].state, "active");
    }

    /// Struct-based serialization (toml::to_string_pretty) must preserve
    /// the key order from the original file for map-like sections.
    #[test]
    fn test_struct_round_trip_preserves_prefix_order() {
        let config_text = r#"[project]
name = "test"

[ids.prefixes]
zebra = "ZEB"
alpha = "ALP"
middle = "MID"
"#;
        let config: ProjectConfig = toml::from_str(config_text).unwrap();
        let output = toml::to_string_pretty(&config).unwrap();
        let reparsed: ProjectConfig = toml::from_str(&output).unwrap();

        // Keys must appear in the same order after round-trip
        let keys: Vec<&String> = config.ids.prefixes.keys().collect();
        let keys_rt: Vec<&String> = reparsed.ids.prefixes.keys().collect();
        assert_eq!(keys, keys_rt, "prefix key order changed after round-trip");
        assert_eq!(keys, vec!["zebra", "alpha", "middle"]);
    }

    /// Verify that write_config_from_struct produces a valid config that
    /// re-parses identically.
    #[test]
    fn test_write_config_from_struct_round_trip() {
        let tmp = TempDir::new().unwrap();
        let frame_dir = tmp.path().join("frame");
        fs::create_dir_all(&frame_dir).unwrap();

        let config_text = r##"[project]
name = "test"

[ids.prefixes]
zebra = "ZEB"
alpha = "ALP"

[ui.tag_colors]
bug = "#FF0000"
design = "#00FF00"
"##;
        let original: ProjectConfig = toml::from_str(config_text).unwrap();
        write_config_from_struct(&frame_dir, &original).unwrap();

        let (reloaded, _doc) = read_config(&frame_dir).unwrap();
        let prefix_keys: Vec<&String> = reloaded.ids.prefixes.keys().collect();
        let tag_keys: Vec<&String> = reloaded.ui.tag_colors.keys().collect();
        assert_eq!(prefix_keys, vec!["zebra", "alpha"]);
        assert_eq!(tag_keys, vec!["bug", "design"]);
    }
}
