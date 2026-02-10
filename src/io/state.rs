use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

/// Persisted TUI state (written to .state.json)
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UiState {
    /// Which view is showing ("track", "tracks", "inbox", "recent")
    pub view: String,
    /// Which track is active (track ID)
    #[serde(default)]
    pub active_track: String,
    /// Per-track state
    #[serde(default)]
    pub tracks: HashMap<String, TrackUiState>,
    /// Last search pattern
    #[serde(default)]
    pub last_search: Option<String>,
    /// Per-session note wrap override (None = use config default)
    #[serde(default)]
    pub note_wrap_override: Option<bool>,
    /// Search history (most recent first, max 200)
    #[serde(default)]
    pub search_history: Vec<String>,
}

/// Per-track UI state
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TrackUiState {
    /// Cursor task ID (or positional key)
    #[serde(default)]
    pub cursor: usize,
    /// Set of expanded task IDs
    #[serde(default)]
    pub expanded: HashSet<String>,
    /// Scroll offset
    #[serde(default)]
    pub scroll_offset: usize,
}

/// Read .state.json from the frame directory
pub fn read_ui_state(frame_dir: &Path) -> Option<UiState> {
    let path = frame_dir.join(".state.json");
    let content = fs::read_to_string(&path).ok()?;
    serde_json::from_str(&content).ok()
}

/// Write .state.json to the frame directory
pub fn write_ui_state(frame_dir: &Path, state: &UiState) -> Result<(), std::io::Error> {
    let path = frame_dir.join(".state.json");
    let content = serde_json::to_string_pretty(state)?;
    fs::write(&path, content)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn write_and_read_round_trip() {
        let dir = TempDir::new().unwrap();
        let mut state = UiState {
            view: "track".into(),
            active_track: "effects".into(),
            last_search: Some("pattern".into()),
            note_wrap_override: Some(false),
            search_history: vec!["foo".into(), "bar".into()],
            ..Default::default()
        };

        let mut track_state = TrackUiState {
            cursor: 5,
            scroll_offset: 10,
            ..Default::default()
        };
        track_state.expanded.insert("T-001".into());
        state.tracks.insert("effects".into(), track_state);

        write_ui_state(dir.path(), &state).unwrap();
        let loaded = read_ui_state(dir.path()).unwrap();

        assert_eq!(loaded.view, "track");
        assert_eq!(loaded.active_track, "effects");
        assert_eq!(loaded.last_search, Some("pattern".into()));
        assert_eq!(loaded.note_wrap_override, Some(false));
        assert_eq!(loaded.search_history, vec!["foo", "bar"]);
        let ts = loaded.tracks.get("effects").unwrap();
        assert_eq!(ts.cursor, 5);
        assert_eq!(ts.scroll_offset, 10);
        assert!(ts.expanded.contains("T-001"));
    }

    #[test]
    fn read_missing_file_returns_none() {
        let dir = TempDir::new().unwrap();
        assert!(read_ui_state(dir.path()).is_none());
    }

    #[test]
    fn read_malformed_json_returns_none() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join(".state.json"), "not json {{{").unwrap();
        assert!(read_ui_state(dir.path()).is_none());
    }

    #[test]
    fn serde_defaults_on_minimal_object() {
        // `view` is required (no #[serde(default)]), other fields have defaults
        let state: UiState = serde_json::from_str(r#"{"view":"track"}"#).unwrap();
        assert_eq!(state.view, "track");
        assert_eq!(state.active_track, "");
        assert!(state.tracks.is_empty());
        assert!(state.last_search.is_none());
        assert!(state.note_wrap_override.is_none());
        assert!(state.search_history.is_empty());
    }

    #[test]
    fn track_ui_state_serde_defaults() {
        let ts: TrackUiState = serde_json::from_str("{}").unwrap();
        assert_eq!(ts.cursor, 0);
        assert!(ts.expanded.is_empty());
        assert_eq!(ts.scroll_offset, 0);
    }
}
