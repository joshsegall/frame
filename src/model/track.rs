use serde::{Deserialize, Serialize};

use super::task::Task;

/// The state of a track (active, shelved, or archived)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TrackState {
    Active,
    Shelved,
    Archived,
}

/// A content node in the track file — either a task section or literal text
#[derive(Debug, Clone)]
pub enum TrackNode {
    /// A literal text block (headers, descriptions, blank lines, etc.)
    Literal(Vec<String>),
    /// A section containing tasks (Backlog, Parked, Done)
    Section {
        kind: SectionKind,
        /// The section header lines (e.g., `## Backlog`)
        header_lines: Vec<String>,
        /// Tasks in this section
        tasks: Vec<Task>,
        /// Trailing blank lines after the last task
        trailing_lines: Vec<String>,
    },
}

/// The kind of task section in a track file
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SectionKind {
    Backlog,
    Parked,
    Done,
}

/// A parsed track file
#[derive(Debug, Clone)]
pub struct Track {
    /// Track title (from `# Title` line)
    pub title: String,
    /// Track description (from `> description` line)
    pub description: Option<String>,
    /// All nodes in the file, in order
    pub nodes: Vec<TrackNode>,
    /// The original source lines of the entire file
    pub source_lines: Vec<String>,
}

impl Track {
    /// Get tasks from a specific section
    pub fn section_tasks(&self, kind: SectionKind) -> &[Task] {
        for node in &self.nodes {
            if let TrackNode::Section { kind: k, tasks, .. } = node
                && *k == kind
            {
                return tasks;
            }
        }
        &[]
    }

    /// Get mutable tasks from a specific section
    pub fn section_tasks_mut(&mut self, kind: SectionKind) -> Option<&mut Vec<Task>> {
        for node in &mut self.nodes {
            if let TrackNode::Section { kind: k, tasks, .. } = node
                && *k == kind
            {
                return Some(tasks);
            }
        }
        None
    }

    /// Get all backlog tasks
    pub fn backlog(&self) -> &[Task] {
        self.section_tasks(SectionKind::Backlog)
    }

    /// Get all parked tasks
    pub fn parked(&self) -> &[Task] {
        self.section_tasks(SectionKind::Parked)
    }

    /// Get all done tasks
    pub fn done(&self) -> &[Task] {
        self.section_tasks(SectionKind::Done)
    }
}
