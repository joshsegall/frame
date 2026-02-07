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
