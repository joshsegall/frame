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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SectionKind {
    Backlog,
    Parked,
    Done,
}

impl std::fmt::Display for SectionKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SectionKind::Backlog => write!(f, "Backlog"),
            SectionKind::Parked => write!(f, "Parked"),
            SectionKind::Done => write!(f, "Done"),
        }
    }
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
    /// The line ending the file used, re-applied when it is written back.
    /// See [`crate::parse::LineEnding`].
    pub eol: crate::parse::LineEnding,
}

/// A section kind appearing more than once in one track.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DuplicateSection {
    pub kind: SectionKind,
    /// How many sections of this kind the file has.
    pub count: usize,
    /// Tasks in the second and later sections — the ones
    /// [`Track::section_tasks`] cannot see.
    pub hidden_tasks: usize,
}

/// A `##` heading in a track file that frame does not recognise.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownHeading {
    /// The heading text, trimmed.
    pub heading: String,
    /// Task lines that fell in behind it and became literal text.
    pub stranded_tasks: usize,
}

impl Track {
    /// Get tasks from a specific section
    ///
    /// **The first section of that kind, and a track is only supposed to have
    /// one.** A file carrying two `## Done` headings — which a line-by-line git
    /// merge of a track file will produce — silently hides everything in the
    /// second from this and from the hundred-odd call sites built on it. See
    /// [`Track::duplicate_sections`], which reports it, and
    /// [`Track::merge_duplicate_sections`], which is applied on every write.
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

    /// Section kinds this track has more than one of.
    ///
    /// Cheap — a walk of the node list — so callers on the write path can ask
    /// before deciding whether any repair is needed at all.
    pub fn duplicate_sections(&self) -> Vec<DuplicateSection> {
        let mut seen: Vec<(SectionKind, usize, usize)> = Vec::new();
        for node in &self.nodes {
            if let TrackNode::Section { kind, tasks, .. } = node {
                match seen.iter_mut().find(|(k, _, _)| k == kind) {
                    Some(entry) => {
                        entry.1 += 1;
                        entry.2 += tasks.len();
                    }
                    None => seen.push((*kind, 1, 0)),
                }
            }
        }
        seen.into_iter()
            .filter(|(_, count, _)| *count > 1)
            .map(|(kind, count, hidden_tasks)| DuplicateSection {
                kind,
                count,
                hidden_tasks,
            })
            .collect()
    }

    pub fn has_duplicate_sections(&self) -> bool {
        !self.duplicate_sections().is_empty()
    }

    /// `##` headings frame does not recognise, with the task lines each one
    /// swallowed.
    ///
    /// The parser sends an unknown heading to a literal node — and then
    /// *everything after it* too, until the next heading it does know, because
    /// task lines are only parsed inside a section. So a stray heading does not
    /// merely sit there being ignored; the tasks behind it stop being tasks.
    /// That is why this is reported even when it stranded nothing: the next
    /// task written under it would vanish.
    pub fn unknown_headings(&self) -> Vec<UnknownHeading> {
        let mut out = Vec::new();
        for node in &self.nodes {
            let TrackNode::Literal(lines) = node else {
                continue;
            };
            let mut current: Option<UnknownHeading> = None;
            for line in lines {
                // Column zero, untrimmed — an indented `## ` is body text inside
                // someone's note or a fenced block, and the parser treats it that
                // way too. Trimming first is how this reported five headings in a
                // real project's inbox that were all prose.
                let trimmed = line.trim_end();
                if let Some(rest) = line.strip_prefix("## ") {
                    if let Some(h) = current.take() {
                        out.push(h);
                    }
                    current = Some(UnknownHeading {
                        heading: rest.trim().to_string(),
                        stranded_tasks: 0,
                    });
                } else if let Some(h) = &mut current
                    && trimmed.starts_with("- [")
                {
                    h.stranded_tasks += 1;
                }
            }
            if let Some(h) = current {
                out.push(h);
            }
        }
        out
    }

    /// Fold every later section of a kind into the first one of that kind.
    ///
    /// Tasks move in node order, so their relative order is exactly what the
    /// file already showed. The redundant heading and the blank lines around it
    /// are dropped — lines frame owns and writes itself — while literal nodes
    /// between the sections stay where they are, because frame does not know
    /// what they are and never gets to decide they are expendable.
    ///
    /// Returns what it merged, for the caller to report. Idempotent: running it
    /// on a healthy track changes nothing and returns empty.
    pub fn merge_duplicate_sections(&mut self) -> Vec<DuplicateSection> {
        let found = self.duplicate_sections();
        if found.is_empty() {
            return found;
        }
        for dup in &found {
            // Collect the later sections' tasks, dropping those nodes as we go.
            let mut first: Option<usize> = None;
            let mut moved: Vec<Task> = Vec::new();
            let mut drop_at: Vec<usize> = Vec::new();
            for (i, node) in self.nodes.iter_mut().enumerate() {
                if let TrackNode::Section { kind, tasks, .. } = node
                    && *kind == dup.kind
                {
                    if first.is_none() {
                        first = Some(i);
                    } else {
                        moved.append(tasks);
                        drop_at.push(i);
                    }
                }
            }
            if let Some(i) = first
                && let Some(TrackNode::Section { tasks, .. }) = self.nodes.get_mut(i)
            {
                tasks.append(&mut moved);
            }
            for i in drop_at.into_iter().rev() {
                self.nodes.remove(i);
            }
        }
        found
    }

    /// Bytes a section's tasks occupy once written out, newlines included.
    ///
    /// Measured off the model rather than the file on disk, so that the numbers
    /// `fr clean` archives by and `fr check` warns on describe the same thing —
    /// and so that a dry run can price a section it is about to change. It
    /// counts task content only: section headers and any literal text around
    /// them belong to no task and are a rounding error next to the notes.
    pub fn section_bytes(&self, kind: SectionKind) -> usize {
        tasks_bytes(self.section_tasks(kind))
    }

    /// Bytes of everything that is not done — `## Backlog` plus `## Parked`.
    ///
    /// The measure behind `limits.track_warn_bytes`. Done is left out because
    /// `[clean]` already bounds it, and bounds it by *oscillating*: including a
    /// term that swings from 64 KB to 256 KB on its own schedule would make the
    /// warning fire and clear with no human action behind either.
    pub fn live_bytes(&self) -> usize {
        self.section_bytes(SectionKind::Backlog) + self.section_bytes(SectionKind::Parked)
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

    /// Ensure a section exists, creating it if missing.
    /// New sections are inserted in canonical order: Backlog → Parked → Done.
    pub fn ensure_section(&mut self, kind: SectionKind) {
        if self.section_tasks_mut(kind).is_some() {
            return;
        }
        let header = match kind {
            SectionKind::Backlog => "## Backlog",
            SectionKind::Parked => "## Parked",
            SectionKind::Done => "## Done",
        };
        let new_node = TrackNode::Section {
            kind,
            header_lines: vec![header.to_string()],
            tasks: Vec::new(),
            trailing_lines: vec![String::new()],
        };

        // Find the right position: insert before the first section that should come after.
        let order = |k: SectionKind| -> u8 {
            match k {
                SectionKind::Backlog => 0,
                SectionKind::Parked => 1,
                SectionKind::Done => 2,
            }
        };
        let target_order = order(kind);
        let insert_pos = self
            .nodes
            .iter()
            .position(|n| {
                if let TrackNode::Section { kind: k, .. } = n {
                    order(*k) > target_order
                } else {
                    false
                }
            })
            .unwrap_or(self.nodes.len());

        self.nodes.insert(insert_pos, new_node);
    }
}

/// Bytes these tasks occupy once written out, newlines included.
///
/// Serialized rather than summed off `source_text`, because `source_text` holds
/// only a task's own lines and never its subtasks — measuring it would silently
/// under-count every task that has any.
pub fn tasks_bytes(tasks: &[Task]) -> usize {
    crate::parse::serialize_tasks(tasks, 0)
        .iter()
        .map(|l| l.len() + 1)
        .sum()
}
