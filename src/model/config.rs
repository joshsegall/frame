use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Configuration from project.toml
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectConfig {
    pub project: ProjectInfo,
    #[serde(default)]
    pub agent: AgentConfig,
    #[serde(default)]
    pub tracks: Vec<TrackConfig>,
    #[serde(default)]
    pub clean: CleanConfig,
    #[serde(default)]
    pub ids: IdConfig,
    #[serde(default)]
    pub ui: UiConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectInfo {
    pub name: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AgentConfig {
    #[serde(default)]
    pub cc_focus: Option<String>,
    #[serde(default = "default_true")]
    pub cc_only: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrackConfig {
    pub id: String,
    pub name: String,
    pub state: String,
    pub file: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CleanConfig {
    /// Default: see src/templates/project.toml
    #[serde(default = "default_true")]
    pub auto_clean: bool,
    /// Default: see src/templates/project.toml
    #[serde(default = "default_done_threshold")]
    pub done_threshold: usize,
    /// Default: see src/templates/project.toml
    #[serde(default = "default_true")]
    pub archive_per_track: bool,
}

impl Default for CleanConfig {
    fn default() -> Self {
        CleanConfig {
            auto_clean: true,
            done_threshold: 250,
            archive_per_track: true,
        }
    }
}

/// Default: see src/templates/project.toml
fn default_true() -> bool {
    true
}

/// Default: see src/templates/project.toml
fn default_done_threshold() -> usize {
    250
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct IdConfig {
    #[serde(default)]
    pub prefixes: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UiConfig {
    #[serde(default)]
    pub show_key_hints: bool,
    #[serde(default)]
    pub colors: HashMap<String, String>,
    #[serde(default)]
    pub tag_colors: HashMap<String, String>,
    /// File extensions to show in ref/spec autocomplete (e.g. ["md", "txt", "pdf"]).
    /// If empty, all files are shown.
    #[serde(default)]
    pub ref_extensions: Vec<String>,
    /// Directories to scope ref/spec autocomplete to (e.g. ["doc", "spec"]).
    /// If empty, the whole project is searched.
    #[serde(default)]
    pub ref_paths: Vec<String>,
    /// Tags always shown in autocomplete (even if no tasks use them yet).
    #[serde(default)]
    pub default_tags: Vec<String>,
    /// Kitty keyboard protocol: true = force on, false = force off, absent = on (default).
    /// Disable if your terminal has issues with enhanced key reporting.
    #[serde(default)]
    pub kitty_keyboard: Option<bool>,
}
