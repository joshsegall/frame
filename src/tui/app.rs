use std::collections::{HashMap, HashSet};
use std::io;
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime};

use crossterm::event::{self, Event, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use regex::Regex;

use crate::io::lock::FileLock;
use crate::io::project_io::{self, discover_project, load_project};
use crate::io::watcher::{FileEvent, FrameWatcher};
use crate::model::{Project, SectionKind, Task, Track};
use crate::parse::{parse_inbox, parse_track};

use super::input;
use super::render;
use super::theme::Theme;
use super::undo::UndoStack;

/// Which view is currently displayed
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum View {
    /// Track view for an active track (index into active_track_ids)
    Track(usize),
    /// All tracks overview
    Tracks,
    /// Inbox
    Inbox,
    /// Recently completed tasks
    Recent,
    /// Detail view for a single task
    Detail {
        track_id: String,
        task_id: String,
    },
}

/// Regions in the detail view that can be navigated
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DetailRegion {
    Title,
    Tags,
    Added,
    Deps,
    Spec,
    Refs,
    Note,
    Subtasks,
}

impl DetailRegion {
    /// Whether this region is editable
    pub fn is_editable(self) -> bool {
        !matches!(self, DetailRegion::Added | DetailRegion::Subtasks)
    }
}

/// Inline edit history for undo/redo within an editing session
#[derive(Debug, Clone, Default)]
pub struct EditHistory {
    /// Snapshots of (buffer, cursor_pos) — for single-line edits
    /// or (buffer, cursor_line, cursor_col) serialized as (buffer, combined) for multi-line
    entries: Vec<(String, usize, usize)>,
    /// Current position in history (points to the currently displayed state)
    position: usize,
}

impl EditHistory {
    pub fn new(initial_buffer: &str, cursor_pos: usize, cursor_line: usize) -> Self {
        EditHistory {
            entries: vec![(initial_buffer.to_string(), cursor_pos, cursor_line)],
            position: 0,
        }
    }

    /// Save a snapshot (call after each text-modifying action)
    pub fn snapshot(&mut self, buffer: &str, cursor_pos: usize, cursor_line: usize) {
        // Don't save duplicate consecutive states
        if let Some(last) = self.entries.get(self.position) {
            if last.0 == buffer {
                return;
            }
        }
        // Truncate any redo entries
        self.entries.truncate(self.position + 1);
        self.entries.push((buffer.to_string(), cursor_pos, cursor_line));
        self.position = self.entries.len() - 1;
    }

    /// Undo: move back in history. Returns (buffer, cursor_pos, cursor_line) or None.
    pub fn undo(&mut self) -> Option<(&str, usize, usize)> {
        if self.position > 0 {
            self.position -= 1;
            let (buf, pos, line) = &self.entries[self.position];
            Some((buf, *pos, *line))
        } else {
            None
        }
    }

    /// Redo: move forward in history. Returns (buffer, cursor_pos, cursor_line) or None.
    pub fn redo(&mut self) -> Option<(&str, usize, usize)> {
        if self.position + 1 < self.entries.len() {
            self.position += 1;
            let (buf, pos, line) = &self.entries[self.position];
            Some((buf, *pos, *line))
        } else {
            None
        }
    }
}

/// What kind of autocomplete is active
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutocompleteKind {
    /// Tag names (from config tag_colors + existing tags in project)
    Tag,
    /// Task IDs (all task IDs across tracks)
    TaskId,
    /// File paths (walk project directory)
    FilePath,
}

/// State for the autocomplete dropdown
#[derive(Debug, Clone)]
pub struct AutocompleteState {
    /// What kind of autocomplete entries to show
    pub kind: AutocompleteKind,
    /// All candidate entries (unfiltered)
    pub candidates: Vec<String>,
    /// Filtered entries matching current input
    pub filtered: Vec<String>,
    /// Currently selected index in filtered list
    pub selected: usize,
    /// Whether the dropdown is visible
    pub visible: bool,
}

impl AutocompleteState {
    pub fn new(kind: AutocompleteKind, candidates: Vec<String>) -> Self {
        let filtered = candidates.clone();
        AutocompleteState {
            kind,
            candidates,
            filtered,
            selected: 0,
            visible: true,
        }
    }

    /// Compute the byte offset within the edit buffer where the current completion
    /// word starts. This is the position where accepted text will be inserted,
    /// and is used to align the autocomplete popup horizontally.
    pub fn word_start_in_buffer(&self, buffer: &str) -> usize {
        match self.kind {
            AutocompleteKind::Tag => {
                // Last word starts after the last space (the word may begin with #)
                buffer.rfind(' ').map(|i| i + 1).unwrap_or(0)
            }
            AutocompleteKind::TaskId => {
                // Last entry starts after the last comma or whitespace
                buffer
                    .rfind(|c: char| c == ',' || c.is_whitespace())
                    .map(|i| {
                        // Skip any trailing whitespace after the delimiter
                        let rest = &buffer[i + 1..];
                        let trimmed = rest.len() - rest.trim_start().len();
                        i + 1 + trimmed
                    })
                    .unwrap_or(0)
            }
            AutocompleteKind::FilePath => {
                // Last entry starts after the last space
                buffer.rfind(' ').map(|i| i + 1).unwrap_or(0)
            }
        }
    }

    /// Filter candidates based on the current input fragment
    pub fn filter(&mut self, input: &str) {
        let query = input.to_lowercase();
        self.filtered = self
            .candidates
            .iter()
            .filter(|c| c.to_lowercase().contains(&query))
            .cloned()
            .collect();
        // Clamp selected
        if self.selected >= self.filtered.len() {
            self.selected = 0;
        }
    }

    /// Move selection up
    pub fn move_up(&mut self) {
        if !self.filtered.is_empty() {
            if self.selected == 0 {
                self.selected = self.filtered.len() - 1;
            } else {
                self.selected -= 1;
            }
        }
    }

    /// Move selection down
    pub fn move_down(&mut self) {
        if !self.filtered.is_empty() {
            self.selected = (self.selected + 1) % self.filtered.len();
        }
    }

    /// Get the currently selected entry
    pub fn selected_entry(&self) -> Option<&str> {
        self.filtered.get(self.selected).map(|s| s.as_str())
    }
}

/// State for the detail view
#[derive(Debug, Clone)]
pub struct DetailState {
    /// Which region the cursor is on
    pub region: DetailRegion,
    /// Scroll offset for the detail view
    pub scroll_offset: usize,
    /// The list of regions present for the current task (computed on render)
    pub regions: Vec<DetailRegion>,
    /// Track view index to return to on Esc
    pub return_view_idx: usize,
    /// Whether we're editing in the detail view
    pub editing: bool,
    /// For multi-line note editing: the buffer
    pub edit_buffer: String,
    /// For multi-line note editing: cursor position (line, col)
    pub edit_cursor_line: usize,
    pub edit_cursor_col: usize,
    /// Original value before editing (for cancel/undo)
    pub edit_original: String,
}

/// Current interaction mode
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    Navigate,
    Search,
    /// Inline title editing (for new tasks or editing existing)
    Edit,
    /// Task/track reordering mode
    Move,
}

/// What kind of edit operation is in progress
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditTarget {
    /// Creating a new task (title edit). Stores the assigned task ID and track_id.
    /// `parent_id` is Some for subtasks.
    NewTask {
        task_id: String,
        track_id: String,
        parent_id: Option<String>,
    },
    /// Editing an existing task's title
    ExistingTitle {
        task_id: String,
        track_id: String,
        original_title: String,
    },
    /// Editing an existing task's tags (inline from track view)
    ExistingTags {
        task_id: String,
        track_id: String,
        original_tags: String,
    },
}

/// State for MOVE mode
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MoveState {
    /// Moving a task within a track's backlog
    Task {
        track_id: String,
        task_id: String,
        original_index: usize,
    },
    /// Moving an active track in the tracks list
    Track {
        track_id: String,
        original_index: usize,
    },
}

/// Per-track UI state (cursor, scroll, expand/collapse)
#[derive(Debug, Clone, Default)]
pub struct TrackViewState {
    /// Cursor index into the flat visible items list
    pub cursor: usize,
    /// Scroll offset (first visible row)
    pub scroll_offset: usize,
    /// Set of expanded task IDs (or synthetic keys for tasks without IDs)
    pub expanded: HashSet<String>,
}

/// A flattened item in the track view's visible list
#[derive(Debug, Clone)]
pub enum FlatItem {
    /// A task from a specific section
    Task {
        section: SectionKind,
        /// Path through the task tree: indices at each nesting level
        path: Vec<usize>,
        depth: usize,
        has_children: bool,
        is_expanded: bool,
        is_last_sibling: bool,
        /// For building tree continuation lines: whether each ancestor is the last sibling
        ancestor_last: Vec<bool>,
    },
    /// The "── Parked ──" separator
    ParkedSeparator,
}

/// Main application state
pub struct App {
    pub project: Project,
    pub view: View,
    pub mode: Mode,
    pub should_quit: bool,
    pub theme: Theme,
    /// IDs of active tracks (in display order)
    pub active_track_ids: Vec<String>,
    /// Per-track view state
    pub track_states: HashMap<String, TrackViewState>,
    /// Cursor for tracks view
    pub tracks_cursor: usize,
    /// Cursor for inbox view
    pub inbox_cursor: usize,
    /// Cursor for recent view
    pub recent_cursor: usize,
    /// Scroll offset for inbox view
    pub inbox_scroll: usize,
    /// Scroll offset for recent view
    pub recent_scroll: usize,
    /// Help overlay visible
    pub show_help: bool,
    /// Search mode: current query being typed
    pub search_input: String,
    /// Last executed search pattern
    pub last_search: Option<String>,
    /// Current search match index (for n/N cycling)
    pub search_match_idx: usize,
    /// Search history (most recent first, max 200)
    pub search_history: Vec<String>,
    /// Current position in search history (None = new/draft, Some(0) = most recent, etc.)
    pub search_history_index: Option<usize>,
    /// Draft search text (preserved while browsing history)
    pub search_draft: String,
    /// Wrap-around message shown after n/N wraps (cleared on next n/N or Esc)
    pub search_wrap_message: Option<String>,
    /// Number of matches for the current search pattern in the current view
    pub search_match_count: Option<usize>,
    /// True when user hit Enter with 0 matches (for red background highlight)
    pub search_zero_confirmed: bool,
    /// True after first Q press; second Q quits
    pub quit_pending: bool,
    /// Transient centered status message (cleared on next keypress)
    pub status_message: Option<String>,
    /// Edit mode: text buffer for inline editing
    pub edit_buffer: String,
    /// Edit mode: cursor position within the buffer
    pub edit_cursor: usize,
    /// Edit mode: what is being edited
    pub edit_target: Option<EditTarget>,
    /// Saved cursor position to restore on edit cancel (for new task inserts)
    pub pre_edit_cursor: Option<usize>,
    /// Move mode state
    pub move_state: Option<MoveState>,
    /// Undo/redo stack (session-only, not persisted)
    pub undo_stack: UndoStack,
    /// Pending external file reload paths (queued while in EDIT/MOVE mode)
    pub pending_reload_paths: Vec<PathBuf>,
    /// Conflict text shown when external change conflicts with in-progress edit
    pub conflict_text: Option<String>,
    /// Timestamp of last save we performed (used to ignore our own write notifications)
    pub last_save_at: Option<Instant>,
    /// Last-known mtime for each track file (keyed by track_id)
    pub track_mtimes: HashMap<String, SystemTime>,
    /// Detail view state
    pub detail_state: Option<DetailState>,
    /// Autocomplete state (active during EDIT mode for certain fields)
    pub autocomplete: Option<AutocompleteState>,
    /// Screen position (x, y) where the edit text area starts, used to anchor autocomplete dropdown
    pub autocomplete_anchor: Option<(u16, u16)>,
    /// Inline edit history for undo/redo within an editing session
    pub edit_history: Option<EditHistory>,
    /// Selection anchor for text selection in edit mode (None = no selection)
    /// Selection range is from min(anchor, edit_cursor) to max(anchor, edit_cursor)
    pub edit_selection_anchor: Option<usize>,
}

impl App {
    pub fn new(project: Project) -> Self {
        let active_track_ids: Vec<String> = project
            .config
            .tracks
            .iter()
            .filter(|t| t.state == "active")
            .map(|t| t.id.clone())
            .collect();

        let theme = Theme::from_config(&project.config.ui);

        let initial_view = if active_track_ids.is_empty() {
            View::Tracks
        } else {
            View::Track(0)
        };

        // Record initial mtimes for all track files
        let mut track_mtimes = HashMap::new();
        for tc in &project.config.tracks {
            let path = project.frame_dir.join(&tc.file);
            if let Ok(meta) = std::fs::metadata(&path) {
                if let Ok(mtime) = meta.modified() {
                    track_mtimes.insert(tc.id.clone(), mtime);
                }
            }
        }

        // Initialize track states with default expand for first task
        let mut track_states = HashMap::new();
        for track_id in &active_track_ids {
            let mut state = TrackViewState::default();
            // Expand first task by default
            if let Some(track) = Self::find_track_in_project(&project, track_id) {
                let backlog = track.backlog();
                if let Some(first) = backlog.first() {
                    let key = task_expand_key(first, SectionKind::Backlog, &[0]);
                    state.expanded.insert(key);
                }
            }
            track_states.insert(track_id.clone(), state);
        }

        App {
            project,
            view: initial_view,
            mode: Mode::Navigate,
            should_quit: false,
            theme,
            active_track_ids,
            track_states,
            tracks_cursor: 0,
            inbox_cursor: 0,
            recent_cursor: 0,
            inbox_scroll: 0,
            recent_scroll: 0,
            show_help: false,
            search_input: String::new(),
            last_search: None,
            search_match_idx: 0,
            search_history: Vec::new(),
            search_history_index: None,
            search_draft: String::new(),
            search_wrap_message: None,
            search_match_count: None,
            search_zero_confirmed: false,
            quit_pending: false,
            status_message: None,
            edit_buffer: String::new(),
            edit_cursor: 0,
            edit_target: None,
            pre_edit_cursor: None,
            move_state: None,
            undo_stack: UndoStack::new(),
            pending_reload_paths: Vec::new(),
            conflict_text: None,
            last_save_at: None,
            track_mtimes,
            detail_state: None,
            autocomplete: None,
            autocomplete_anchor: None,
            edit_history: None,
            edit_selection_anchor: None,
        }
    }

    pub fn find_track_in_project<'a>(project: &'a Project, track_id: &str) -> Option<&'a Track> {
        project
            .tracks
            .iter()
            .find(|(id, _)| id == track_id)
            .map(|(_, track)| track)
    }

    /// Get the display name for a track by its ID
    pub fn track_name<'a>(&'a self, track_id: &'a str) -> &'a str {
        self.project
            .config
            .tracks
            .iter()
            .find(|t| t.id == track_id)
            .map(|t| t.name.as_str())
            .unwrap_or(track_id)
    }

    /// Count inbox items
    pub fn inbox_count(&self) -> usize {
        self.project
            .inbox
            .as_ref()
            .map_or(0, |inbox| inbox.items.len())
    }

    /// Get the selection range (start, end) for the single-line edit buffer, if any.
    /// Returns (start, end) where start <= end.
    pub fn edit_selection_range(&self) -> Option<(usize, usize)> {
        let anchor = self.edit_selection_anchor?;
        let cursor = self.edit_cursor;
        Some((anchor.min(cursor), anchor.max(cursor)))
    }

    /// Delete the selected text and return the cursor to the start of selection.
    /// Returns true if there was a selection to delete.
    pub fn delete_selection(&mut self) -> bool {
        if let Some((start, end)) = self.edit_selection_range() {
            if start != end {
                self.edit_buffer.drain(start..end);
                self.edit_cursor = start;
                self.edit_selection_anchor = None;
                return true;
            }
        }
        self.edit_selection_anchor = None;
        false
    }

    /// Collect all unique tags from config tag_colors + all tasks in the project
    pub fn collect_all_tags(&self) -> Vec<String> {
        let mut tags: HashSet<String> = HashSet::new();

        // Tags from config tag_colors keys
        for key in self.project.config.ui.tag_colors.keys() {
            tags.insert(key.clone());
        }

        // Tags from theme tag_colors (includes hardcoded defaults like 'cc')
        for key in self.theme.tag_colors.keys() {
            tags.insert(key.clone());
        }

        // Tags from agent default_tags
        for tag in &self.project.config.agent.default_tags {
            tags.insert(tag.clone());
        }

        // Tags from all tasks across all tracks
        for (_, track) in &self.project.tracks {
            Self::collect_tags_from_tasks(&track.backlog(), &mut tags);
            Self::collect_tags_from_tasks(&track.parked(), &mut tags);
            Self::collect_tags_from_tasks(&track.done(), &mut tags);
        }

        // Tags from inbox items
        if let Some(inbox) = &self.project.inbox {
            for item in &inbox.items {
                for tag in &item.tags {
                    tags.insert(tag.clone());
                }
            }
        }

        let mut sorted: Vec<String> = tags.into_iter().collect();
        sorted.sort();
        sorted
    }

    fn collect_tags_from_tasks(tasks: &[Task], tags: &mut HashSet<String>) {
        for task in tasks {
            for tag in &task.tags {
                tags.insert(tag.clone());
            }
            Self::collect_tags_from_tasks(&task.subtasks, tags);
        }
    }

    /// Collect all task IDs across all tracks
    pub fn collect_all_task_ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = Vec::new();
        for (_, track) in &self.project.tracks {
            Self::collect_ids_from_tasks(&track.backlog(), &mut ids);
            Self::collect_ids_from_tasks(&track.parked(), &mut ids);
            Self::collect_ids_from_tasks(&track.done(), &mut ids);
        }
        ids.sort();
        ids
    }

    fn collect_ids_from_tasks(tasks: &[Task], ids: &mut Vec<String>) {
        for task in tasks {
            if let Some(ref id) = task.id {
                ids.push(id.clone());
            }
            Self::collect_ids_from_tasks(&task.subtasks, ids);
        }
    }

    /// Collect file paths from the project directory (for ref/spec autocomplete).
    /// Scoped to `ref_paths` dirs if configured; filters to `ref_extensions` if set;
    /// always excludes directories.
    pub fn collect_file_paths(&self) -> Vec<String> {
        let mut paths: Vec<String> = Vec::new();
        let frame_dir = &self.project.frame_dir;
        let project_root = frame_dir.parent().unwrap_or(frame_dir);
        let extensions = &self.project.config.ui.ref_extensions;
        let ref_paths = &self.project.config.ui.ref_paths;

        if ref_paths.is_empty() {
            Self::walk_dir_for_paths(project_root, project_root, &mut paths, 3, extensions);
        } else {
            for rp in ref_paths {
                let dir = project_root.join(rp);
                if dir.is_dir() {
                    Self::walk_dir_for_paths(project_root, &dir, &mut paths, 3, extensions);
                }
            }
        }
        paths.sort();
        paths
    }

    fn walk_dir_for_paths(
        base: &std::path::Path,
        dir: &std::path::Path,
        paths: &mut Vec<String>,
        max_depth: usize,
        extensions: &[String],
    ) {
        if max_depth == 0 {
            return;
        }
        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(_) => return,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");

            // Skip hidden dirs/files, node_modules, target, .git
            if name.starts_with('.') || name == "node_modules" || name == "target" {
                continue;
            }

            if path.is_dir() {
                Self::walk_dir_for_paths(base, &path, paths, max_depth - 1, extensions);
            } else if path.is_file() {
                // Filter by extension if ref_extensions is configured
                if !extensions.is_empty() {
                    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
                    if !extensions.iter().any(|e| e.eq_ignore_ascii_case(ext)) {
                        continue;
                    }
                }
                if let Ok(rel) = path.strip_prefix(base) {
                    paths.push(rel.to_string_lossy().to_string());
                }
            }
        }
    }

    /// Get the active search regex for highlighting.
    /// In Search mode: compiles from current input. In Navigate: compiles from last_search.
    pub fn active_search_re(&self) -> Option<Regex> {
        let pattern = match &self.mode {
            Mode::Search if !self.search_input.is_empty() => self.search_input.as_str(),
            Mode::Navigate => self.last_search.as_deref()?,
            _ => return None,
        };
        Regex::new(&format!("(?i){}", pattern))
            .or_else(|_| Regex::new(&format!("(?i){}", regex::escape(pattern))))
            .ok()
    }

    /// Get the currently active track ID (if in track view)
    pub fn current_track_id(&self) -> Option<&str> {
        match &self.view {
            View::Track(idx) => self.active_track_ids.get(*idx).map(|s| s.as_str()),
            _ => None,
        }
    }

    /// Get the track for the current view
    pub fn current_track(&self) -> Option<&Track> {
        let track_id = self.current_track_id()?;
        Self::find_track_in_project(&self.project, track_id)
    }

    /// Get or create the TrackViewState for a track
    pub fn get_track_state(&mut self, track_id: &str) -> &mut TrackViewState {
        if !self.track_states.contains_key(track_id) {
            self.track_states
                .insert(track_id.to_string(), TrackViewState::default());
        }
        self.track_states.get_mut(track_id).unwrap()
    }

    /// Get the ID prefix for a track (e.g., "EFF" for "effects")
    pub fn track_prefix(&self, track_id: &str) -> Option<&str> {
        self.project
            .config
            .ids
            .prefixes
            .get(track_id)
            .map(|s| s.as_str())
    }

    /// Get the file path for a track (relative to frame_dir)
    pub fn track_file(&self, track_id: &str) -> Option<&str> {
        self.project
            .config
            .tracks
            .iter()
            .find(|tc| tc.id == track_id)
            .map(|tc| tc.file.as_str())
    }

    /// Find a mutable track reference by ID
    pub fn find_track_mut(&mut self, track_id: &str) -> Option<&mut Track> {
        self.project
            .tracks
            .iter_mut()
            .find(|(id, _)| id == track_id)
            .map(|(_, track)| track)
    }

    /// Read and parse a single track file from disk, updating stored mtime.
    pub fn read_track_from_disk(&mut self, track_id: &str) -> Option<Track> {
        let file = self.track_file(track_id)?;
        let path = self.project.frame_dir.join(file);
        let meta = std::fs::metadata(&path).ok()?;
        let text = std::fs::read_to_string(&path).ok()?;
        if let Ok(mtime) = meta.modified() {
            self.track_mtimes.insert(track_id.to_string(), mtime);
        }
        Some(parse_track(&text))
    }

    /// Replace a track's in-memory data.
    pub fn replace_track(&mut self, track_id: &str, new_track: Track) {
        if let Some(entry) = self
            .project
            .tracks
            .iter_mut()
            .find(|(id, _)| id == track_id)
        {
            entry.1 = new_track;
        }
    }

    /// Check if the track file on disk has been modified since we last loaded/saved it.
    pub fn track_changed_on_disk(&self, track_id: &str) -> bool {
        let file = match self.track_file(track_id) {
            Some(f) => f,
            None => return false,
        };
        let path = self.project.frame_dir.join(file);
        let disk_mtime = match std::fs::metadata(&path).and_then(|m| m.modified()) {
            Ok(t) => t,
            Err(_) => return false,
        };
        match self.track_mtimes.get(track_id) {
            Some(known) => disk_mtime > *known,
            None => true, // no recorded mtime — treat as changed
        }
    }

    /// Save a track to disk with file locking. Records save time and mtime.
    pub fn save_track(&mut self, track_id: &str) -> Result<(), Box<dyn std::error::Error>> {
        let file = self
            .track_file(track_id)
            .ok_or("track not found")?
            .to_string();
        let track = Self::find_track_in_project(&self.project, track_id)
            .ok_or("track not found")?;
        let _lock = FileLock::acquire_default(&self.project.frame_dir)?;
        project_io::save_track(&self.project.frame_dir, &file, track)?;
        self.last_save_at = Some(Instant::now());
        // Record the new mtime so we know this is our write
        let path = self.project.frame_dir.join(&file);
        if let Ok(mtime) = std::fs::metadata(&path).and_then(|m| m.modified()) {
            self.track_mtimes.insert(track_id.to_string(), mtime);
        }
        Ok(())
    }

    /// Resolve the task ID from the current cursor position in a track view.
    /// Returns (track_id, task_id, section) if the cursor is on a task.
    pub fn cursor_task_id(&self) -> Option<(String, String, SectionKind)> {
        let track_id = self.current_track_id()?.to_string();
        let flat_items = self.build_flat_items(&track_id);
        let cursor = self.track_states.get(&track_id).map_or(0, |s| s.cursor);
        let item = flat_items.get(cursor)?;

        if let FlatItem::Task { section, path, .. } = item {
            let track = Self::find_track_in_project(&self.project, &track_id)?;
            let task = resolve_task_from_flat(track, *section, path)?;
            let task_id = task.id.clone()?;
            Some((track_id, task_id, *section))
        } else {
            None
        }
    }

    /// Reload changed files from disk. Returns the edit target's task_id if it was externally modified.
    pub fn reload_changed_files(&mut self, paths: &[std::path::PathBuf]) -> Option<String> {
        let mut edited_task_conflict = None;

        // Determine which task is being edited (if any)
        let editing_task_id = match &self.edit_target {
            Some(EditTarget::NewTask { task_id, .. })
            | Some(EditTarget::ExistingTitle { task_id, .. })
            | Some(EditTarget::ExistingTags { task_id, .. }) => Some(task_id.clone()),
            None => None,
        };
        let editing_track_id = match &self.edit_target {
            Some(EditTarget::NewTask { track_id, .. })
            | Some(EditTarget::ExistingTitle { track_id, .. })
            | Some(EditTarget::ExistingTags { track_id, .. }) => Some(track_id.clone()),
            None => None,
        };

        for path in paths {
            let file_name = match path.file_name().and_then(|n| n.to_str()) {
                Some(name) => name.to_string(),
                None => continue,
            };

            if file_name == "inbox.md" {
                // Reload inbox
                if let Ok(text) = std::fs::read_to_string(path) {
                    self.project.inbox = Some(parse_inbox(&text));
                }
                continue;
            }

            if file_name == "project.toml" {
                // Config changes — skip for now (would need full re-init)
                continue;
            }

            // It's a track file — find which track it belongs to
            if let Some((track_id, _track_file)) = self
                .project
                .config
                .tracks
                .iter()
                .find(|tc| tc.file == file_name || tc.file.ends_with(&format!("/{}", file_name)))
                .map(|tc| (tc.id.clone(), tc.file.clone()))
            {
                if let Ok(text) = std::fs::read_to_string(path) {
                    let new_track = parse_track(&text);

                    // Check if the edited task was modified externally
                    if editing_track_id.as_deref() == Some(&track_id) {
                        if let Some(ref edit_task_id) = editing_task_id {
                            // Check if the task exists in the new track and has different content
                            if let Some(old_track) =
                                Self::find_track_in_project(&self.project, &track_id)
                            {
                                let old_task =
                                    crate::ops::task_ops::find_task_in_track(old_track, edit_task_id);
                                let new_task =
                                    crate::ops::task_ops::find_task_in_track(&new_track, edit_task_id);

                                match (old_task, new_task) {
                                    (Some(old), Some(new)) if old.title != new.title => {
                                        // Task was modified externally — conflict
                                        edited_task_conflict = Some(edit_task_id.clone());
                                    }
                                    (Some(_), None) => {
                                        // Task was removed externally — conflict
                                        edited_task_conflict = Some(edit_task_id.clone());
                                    }
                                    _ => {}
                                }
                            }
                        }
                    }

                    // Replace the track data and update mtime
                    if let Some(entry) = self
                        .project
                        .tracks
                        .iter_mut()
                        .find(|(id, _)| id == &track_id)
                    {
                        entry.1 = new_track;
                    }
                    if let Ok(mtime) = std::fs::metadata(path).and_then(|m| m.modified()) {
                        self.track_mtimes.insert(track_id, mtime);
                    }
                }
            }
        }

        // Auto-assign IDs and dates to any newly-loaded tasks
        let modified_tracks = crate::ops::clean::ensure_ids_and_dates(&mut self.project);
        for track_id in &modified_tracks {
            let _ = self.save_track(track_id);
        }

        // Push sync marker to undo stack
        self.undo_stack.push_sync_marker();

        edited_task_conflict
    }

    /// Build the list of regions present for a task (for detail view navigation)
    pub fn build_detail_regions(task: &Task) -> Vec<DetailRegion> {
        use crate::model::Metadata;
        let mut regions = vec![DetailRegion::Title];

        // Tags region always present (can add tags even if none exist)
        regions.push(DetailRegion::Tags);

        // Added date
        if task.metadata.iter().any(|m| matches!(m, Metadata::Added(_))) {
            regions.push(DetailRegion::Added);
        }

        // Deps
        regions.push(DetailRegion::Deps);

        // Spec
        regions.push(DetailRegion::Spec);

        // Refs
        regions.push(DetailRegion::Refs);

        // Note
        regions.push(DetailRegion::Note);

        // Subtasks
        if !task.subtasks.is_empty() {
            regions.push(DetailRegion::Subtasks);
        }

        regions
    }

    /// Open the detail view for a task
    pub fn open_detail(&mut self, track_id: String, task_id: String) {
        let return_idx = match &self.view {
            View::Track(idx) => *idx,
            _ => 0,
        };

        // Build initial regions from the task
        let regions = if let Some(track) = Self::find_track_in_project(&self.project, &track_id) {
            if let Some(task) = crate::ops::task_ops::find_task_in_track(track, &task_id) {
                Self::build_detail_regions(task)
            } else {
                vec![DetailRegion::Title]
            }
        } else {
            vec![DetailRegion::Title]
        };

        let initial_region = regions.first().copied().unwrap_or(DetailRegion::Title);

        self.detail_state = Some(DetailState {
            region: initial_region,
            scroll_offset: 0,
            regions,
            return_view_idx: return_idx,
            editing: false,
            edit_buffer: String::new(),
            edit_cursor_line: 0,
            edit_cursor_col: 0,
            edit_original: String::new(),
        });
        self.view = View::Detail { track_id, task_id };
    }

    /// Build the flat list of visible items for a track view
    pub fn build_flat_items(&self, track_id: &str) -> Vec<FlatItem> {
        let track = match Self::find_track_in_project(&self.project, track_id) {
            Some(t) => t,
            None => return Vec::new(),
        };
        let state = self.track_states.get(track_id);
        let expanded = state.map(|s| &s.expanded);

        let mut items = Vec::new();

        // Backlog tasks
        let backlog = track.backlog();
        flatten_tasks(backlog, SectionKind::Backlog, 0, &mut items, expanded, &[]);

        // Parked section (if non-empty)
        let parked = track.parked();
        if !parked.is_empty() {
            items.push(FlatItem::ParkedSeparator);
            flatten_tasks(parked, SectionKind::Parked, 0, &mut items, expanded, &[]);
        }

        // Done tasks are NOT shown in track view (they're in Recent)

        items
    }
}

/// Resolve a task reference from a track using section + index path
pub fn resolve_task_from_flat<'a>(
    track: &'a Track,
    section: SectionKind,
    path: &[usize],
) -> Option<&'a Task> {
    let tasks = track.section_tasks(section);
    if path.is_empty() {
        return None;
    }
    let mut current = tasks.get(path[0])?;
    for &idx in &path[1..] {
        current = current.subtasks.get(idx)?;
    }
    Some(current)
}

/// Generate a unique key for a task's expand/collapse state
pub fn task_expand_key(task: &Task, section: SectionKind, path: &[usize]) -> String {
    if let Some(id) = &task.id {
        id.clone()
    } else {
        let section_str = match section {
            SectionKind::Backlog => "b",
            SectionKind::Parked => "p",
            SectionKind::Done => "d",
        };
        format!(
            "_{}_{}",
            section_str,
            path.iter()
                .map(|i| i.to_string())
                .collect::<Vec<_>>()
                .join("_")
        )
    }
}

/// Recursively flatten tasks into visible items based on expand state
fn flatten_tasks(
    tasks: &[Task],
    section: SectionKind,
    depth: usize,
    items: &mut Vec<FlatItem>,
    expanded: Option<&HashSet<String>>,
    ancestor_last: &[bool],
) {
    flatten_tasks_inner(tasks, section, depth, items, expanded, ancestor_last, &[]);
}

fn flatten_tasks_inner(
    tasks: &[Task],
    section: SectionKind,
    depth: usize,
    items: &mut Vec<FlatItem>,
    expanded: Option<&HashSet<String>>,
    ancestor_last: &[bool],
    parent_path: &[usize],
) {
    let count = tasks.len();
    for (i, task) in tasks.iter().enumerate() {
        let is_last = i == count - 1;
        let has_children = !task.subtasks.is_empty();

        let mut path = parent_path.to_vec();
        path.push(i);

        let key = task_expand_key(task, section, &path);
        let is_expanded = has_children && expanded.is_some_and(|set| set.contains(&key));

        items.push(FlatItem::Task {
            section,
            path: path.clone(),
            depth,
            has_children,
            is_expanded,
            is_last_sibling: is_last,
            ancestor_last: ancestor_last.to_vec(),
        });

        if is_expanded {
            let mut new_ancestor_last = ancestor_last.to_vec();
            new_ancestor_last.push(is_last);
            flatten_tasks_inner(
                &task.subtasks,
                section,
                depth + 1,
                items,
                expanded,
                &new_ancestor_last,
                &path,
            );
        }
    }
}

/// Restore UI state from .state.json
pub fn restore_ui_state(app: &mut App) {
    use crate::io::state::read_ui_state;

    let ui_state = match read_ui_state(&app.project.frame_dir) {
        Some(s) => s,
        None => return,
    };

    // Restore view
    match ui_state.view.as_str() {
        "tracks" => app.view = View::Tracks,
        "inbox" => app.view = View::Inbox,
        "recent" => app.view = View::Recent,
        "track" => {
            if let Some(idx) = app
                .active_track_ids
                .iter()
                .position(|id| id == &ui_state.active_track)
            {
                app.view = View::Track(idx);
            }
        }
        _ => {}
    }

    // Restore per-track state
    for (track_id, track_ui) in &ui_state.tracks {
        let state = app.get_track_state(track_id);
        state.cursor = track_ui.cursor;
        state.scroll_offset = track_ui.scroll_offset;
        state.expanded = track_ui.expanded.clone();
    }

    // Restore last search
    app.last_search = ui_state.last_search;

    // Restore search history
    app.search_history = ui_state.search_history;
}

/// Save UI state to .state.json
pub fn save_ui_state(app: &App) {
    use crate::io::state::{TrackUiState, UiState, write_ui_state};

    let (view_str, active_track) = match &app.view {
        View::Track(idx) => (
            "track".to_string(),
            app.active_track_ids.get(*idx).cloned().unwrap_or_default(),
        ),
        View::Detail { track_id, .. } => (
            "track".to_string(),
            track_id.clone(),
        ),
        View::Tracks => ("tracks".to_string(), String::new()),
        View::Inbox => ("inbox".to_string(), String::new()),
        View::Recent => ("recent".to_string(), String::new()),
    };

    let mut tracks = HashMap::new();
    for (track_id, state) in &app.track_states {
        tracks.insert(
            track_id.clone(),
            TrackUiState {
                cursor: state.cursor,
                expanded: state.expanded.clone(),
                scroll_offset: state.scroll_offset,
            },
        );
    }

    let ui_state = UiState {
        view: view_str,
        active_track,
        tracks,
        last_search: app.last_search.clone(),
        search_history: app.search_history.clone(),
    };

    let _ = write_ui_state(&app.project.frame_dir, &ui_state);
}

/// Run the TUI application
pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    // Discover and load project
    let cwd = std::env::current_dir()?;
    let root = discover_project(&cwd)?;
    let mut project = load_project(&root)?;

    // Auto-assign IDs and dates so all tasks are interactive from the start
    let modified_tracks = crate::ops::clean::ensure_ids_and_dates(&mut project);
    if !modified_tracks.is_empty() {
        let _lock = FileLock::acquire_default(&project.frame_dir)?;
        for track_id in &modified_tracks {
            if let Some(tc) = project.config.tracks.iter().find(|tc| tc.id == *track_id) {
                let file = &tc.file;
                if let Some(track) = project
                    .tracks
                    .iter()
                    .find(|(id, _)| id == track_id)
                    .map(|(_, t)| t)
                {
                    let _ = project_io::save_track(&project.frame_dir, file, track);
                }
            }
        }
    }

    let mut app = App::new(project);

    // Restore saved UI state
    restore_ui_state(&mut app);

    // Start file watcher (non-fatal if it fails)
    let watcher = FrameWatcher::start(&app.project.frame_dir).ok();

    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;

    // Install panic hook to restore terminal on panic
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
        original_hook(panic_info);
    }));

    // Run event loop
    let result = run_event_loop(&mut terminal, &mut app, watcher.as_ref());

    // Save UI state before exit
    save_ui_state(&app);

    // Restore terminal
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    result
}

fn run_event_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
    watcher: Option<&FrameWatcher>,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut save_counter = 0u32;
    loop {
        terminal.draw(|frame| render::render(frame, app))?;

        // Poll for file watcher events
        if let Some(w) = watcher {
            let events = w.poll();
            if !events.is_empty() {
                // Collect all changed paths, dedup
                let mut all_paths = Vec::new();
                for evt in events {
                    match evt {
                        FileEvent::Changed(paths) => all_paths.extend(paths),
                    }
                }
                all_paths.sort();
                all_paths.dedup();

                // If we saved recently (within 1s), assume this is our own write notification
                let is_self_write = app
                    .last_save_at
                    .is_some_and(|t| t.elapsed() < Duration::from_secs(1));
                if is_self_write {
                    app.last_save_at = None; // consume the suppression
                } else if !all_paths.is_empty() {
                    // External change detected
                    if app.mode == Mode::Edit || app.mode == Mode::Move {
                        // Queue reload for when we leave EDIT/MOVE mode
                        app.pending_reload_paths.extend(all_paths);
                    } else {
                        handle_external_reload(app, &all_paths);
                    }
                }
            }
        }

        if event::poll(Duration::from_millis(250))?
            && let Event::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
        {
            input::handle_key(app, key);

            // Process pending reload when returning to Navigate mode
            if !app.pending_reload_paths.is_empty() && app.mode == Mode::Navigate {
                let paths = std::mem::take(&mut app.pending_reload_paths);
                handle_pending_reload(app, &paths);
            }

            // Debounced state save: every ~5 key presses
            save_counter += 1;
            if save_counter >= 5 {
                save_ui_state(app);
                save_counter = 0;
            }
        }

        if app.should_quit {
            break;
        }
    }
    Ok(())
}

/// Handle an external file reload (when specific changed paths are known)
fn handle_external_reload(app: &mut App, paths: &[std::path::PathBuf]) {
    let conflict_task = app.reload_changed_files(paths);
    if conflict_task.is_some() {
        // Save the orphaned edit text in conflict_text
        if !app.edit_buffer.is_empty() {
            app.conflict_text = Some(app.edit_buffer.clone());
        }
        // Cancel the edit mode
        app.mode = Mode::Navigate;
        app.edit_target = None;
        app.edit_buffer.clear();
        app.edit_cursor = 0;
    }
}

/// Handle a pending reload using the stored changed paths
fn handle_pending_reload(app: &mut App, paths: &[PathBuf]) {
    // Dedup paths (may have accumulated duplicates)
    let mut deduped: Vec<PathBuf> = Vec::new();
    for p in paths {
        if !deduped.contains(p) {
            deduped.push(p.clone());
        }
    }
    // This is after EDIT/MOVE completed, so no conflict possible — just reload
    app.reload_changed_files(&deduped);
}
