use std::path::PathBuf;

use super::config::ProjectConfig;
use super::inbox::Inbox;
use super::track::Track;

/// A fully loaded Frame project
#[derive(Debug)]
pub struct Project {
    /// Root directory of the project (parent of `frame/`)
    pub root: PathBuf,
    /// Path to the `frame/` directory
    pub frame_dir: PathBuf,
    /// Parsed project.toml
    pub config: ProjectConfig,
    /// Loaded tracks, indexed by track ID
    pub tracks: Vec<(String, Track)>,
    /// Loaded inbox
    pub inbox: Option<Inbox>,
}
