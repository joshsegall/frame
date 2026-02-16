use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

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
    #[serde(default = "default_done_retain")]
    pub done_retain: usize,
    /// Default: see src/templates/project.toml
    #[serde(default = "default_true")]
    pub archive_per_track: bool,
}

impl Default for CleanConfig {
    fn default() -> Self {
        CleanConfig {
            auto_clean: true,
            done_threshold: 100,
            done_retain: 10,
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
    100
}

/// Default: see src/templates/project.toml
fn default_done_retain() -> usize {
    10
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct IdConfig {
    #[serde(default)]
    pub prefixes: IndexMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UiConfig {
    #[serde(default)]
    pub show_key_hints: bool,
    #[serde(default)]
    pub colors: IndexMap<String, String>,
    #[serde(default)]
    pub tag_colors: IndexMap<String, String>,
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
    /// Whether note editing uses soft word wrap (default: true).
    #[serde(default = "default_true")]
    pub note_wrap: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_config_default() {
        let c = CleanConfig::default();
        assert!(c.auto_clean);
        assert_eq!(c.done_threshold, 100);
        assert_eq!(c.done_retain, 10);
        assert!(c.archive_per_track);
    }

    #[test]
    fn agent_config_default() {
        let a = AgentConfig::default();
        assert!(a.cc_focus.is_none());
        // cc_only default when using Default trait is false (bool default),
        // but serde default_true applies during deserialization
        assert!(!a.cc_only);
    }

    #[test]
    fn agent_config_serde_default_true() {
        // When deserialized from an empty object, cc_only should be true via serde
        let a: AgentConfig = serde_json::from_str("{}").unwrap();
        assert!(a.cc_only);
        assert!(a.cc_focus.is_none());
    }

    #[test]
    fn ui_config_default() {
        let u = UiConfig::default();
        assert!(!u.show_key_hints);
        assert!(u.colors.is_empty());
        assert!(u.tag_colors.is_empty());
        assert!(u.ref_extensions.is_empty());
        assert!(u.ref_paths.is_empty());
        assert!(u.default_tags.is_empty());
        assert!(u.kitty_keyboard.is_none());
        // note_wrap default via Default trait is false (bool default)
        assert!(!u.note_wrap);
    }

    #[test]
    fn ui_config_serde_note_wrap_default_true() {
        // When deserialized from empty object, note_wrap should be true via serde
        let u: UiConfig = serde_json::from_str("{}").unwrap();
        assert!(u.note_wrap);
    }
}
