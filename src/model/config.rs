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
    #[serde(default)]
    pub default_tags: Vec<String>,
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
    #[serde(default = "default_true")]
    pub auto_clean: bool,
    #[serde(default = "default_done_threshold")]
    pub done_threshold: usize,
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

fn default_true() -> bool {
    true
}

fn default_done_threshold() -> usize {
    250
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct IdConfig {
    #[serde(default)]
    pub prefixes: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UiConfig {
    #[serde(default)]
    pub show_key_hints: bool,
    #[serde(default = "default_tag_style")]
    pub tag_style: String,
    #[serde(default)]
    pub colors: HashMap<String, String>,
    #[serde(default)]
    pub tag_colors: HashMap<String, String>,
}

impl Default for UiConfig {
    fn default() -> Self {
        UiConfig {
            show_key_hints: false,
            tag_style: "foreground".to_string(),
            colors: HashMap::new(),
            tag_colors: HashMap::new(),
        }
    }
}

fn default_tag_style() -> String {
    "foreground".to_string()
}
