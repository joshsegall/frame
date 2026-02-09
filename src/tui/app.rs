use std::collections::{HashMap, HashSet};
use std::io::{self, Write};
use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime};

use crossterm::event::{
    self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyEventKind,
    KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
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
use crate::model::{Metadata, Project, SectionKind, Task, TaskState, Track};
use crate::parse::{parse_inbox, parse_track};

use super::input;
use super::render;
use super::theme::Theme;
use super::undo::{Operation, UndoStack};

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
    Detail { track_id: String, task_id: String },
}

/// Regions in the detail view that can be navigated
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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
        // If buffer hasn't changed, just update the cursor position in place
        // so that undo restores the most recent cursor location
        if let Some(last) = self.entries.get_mut(self.position)
            && last.0 == buffer
        {
            last.1 = cursor_pos;
            last.2 = cursor_line;
            return;
        }
        // Truncate any redo entries
        self.entries.truncate(self.position + 1);
        self.entries
            .push((buffer.to_string(), cursor_pos, cursor_line));
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
    /// Task IDs for jump-to-task (entries are "ID  title", whole buffer is filter)
    JumpTaskId,
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
            AutocompleteKind::JumpTaskId => {
                // Whole buffer is the filter text
                0
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

/// Which view to return to when leaving the detail view
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReturnView {
    Track(usize),
    Recent,
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
    /// View to return to on Esc
    pub return_view: ReturnView,
    /// Whether we're editing in the detail view
    pub editing: bool,
    /// For multi-line note editing: the buffer
    pub edit_buffer: String,
    /// For multi-line note editing: cursor position (line, col)
    pub edit_cursor_line: usize,
    pub edit_cursor_col: usize,
    /// Original value before editing (for cancel/undo)
    pub edit_original: String,
    /// Cursor index in flattened subtask list (when region is Subtasks)
    pub subtask_cursor: usize,
    /// Flattened subtask IDs (rebuilt on each render)
    pub flat_subtask_ids: Vec<String>,
    /// Selection anchor for multi-line editing (line, col). None = no selection.
    pub multiline_selection_anchor: Option<(usize, usize)>,
    /// Horizontal scroll offset for multi-line note editing
    pub note_h_scroll: usize,
    /// Total rendered lines (set during render, used for scroll clamping)
    pub total_lines: usize,
    /// Virtual cursor line for note view-mode scrolling (None = not scrolling)
    pub note_view_line: Option<usize>,
    /// Line index of the note header in rendered content (set during render)
    pub note_header_line: Option<usize>,
    /// Last line index belonging to note content, before subtasks (set during render)
    pub note_content_end: usize,
}

/// State for the triage flow (inbox item → track task)
#[derive(Debug, Clone)]
pub enum TriageStep {
    /// Step 1: selecting which track to send the item to
    SelectTrack,
    /// Step 2: selecting position within the track (t=top, b=bottom, a=after)
    SelectPosition { track_id: String },
}

/// Source of a triage/move operation
#[derive(Debug, Clone)]
pub enum TriageSource {
    /// Triaging an inbox item
    Inbox { index: usize },
    /// Cross-track move of an existing task
    CrossTrackMove {
        source_track_id: String,
        task_id: String,
    },
    /// Bulk cross-track move of selected tasks
    BulkCrossTrackMove { source_track_id: String },
}

/// State for the triage flow
#[derive(Debug, Clone)]
pub struct TriageState {
    /// Source of this triage operation
    pub source: TriageSource,
    /// Current step
    pub step: TriageStep,
    /// Screen position for the position-selection popup (set when transitioning from track selection)
    pub popup_anchor: Option<(u16, u16)>,
    /// Cursor for position selection (0=Top, 1=Bottom, 2=Cancel)
    pub position_cursor: u8,
}

/// Confirmation prompt state
#[derive(Debug, Clone)]
pub struct ConfirmState {
    pub message: String,
    pub action: ConfirmAction,
}

/// What to do when confirmation is accepted
#[derive(Debug, Clone)]
pub enum ConfirmAction {
    DeleteInboxItem { index: usize },
    ArchiveTrack { track_id: String },
    DeleteTrack { track_id: String },
}

/// The kind of pending section move (grace period)
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PendingMoveKind {
    /// Task marked done in Backlog → will move to Done section
    ToDone,
    /// Task reopened from Done → will move to Backlog
    ToBacklog,
}

/// A pending section move with a grace period
#[derive(Debug, Clone)]
pub struct PendingMove {
    pub kind: PendingMoveKind,
    pub track_id: String,
    pub task_id: String,
    pub deadline: Instant,
}

/// State filter for track view filtering
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StateFilter {
    Active,
    Todo,
    Blocked,
    Parked,
    /// Ready: todo or active with all deps resolved
    Ready,
}

impl StateFilter {
    /// Display name for the filter indicator
    pub fn label(self) -> &'static str {
        match self {
            StateFilter::Active => "active",
            StateFilter::Todo => "todo",
            StateFilter::Blocked => "blocked",
            StateFilter::Parked => "parked",
            StateFilter::Ready => "ready",
        }
    }
}

/// Filter state for track view (global across all tracks)
#[derive(Debug, Clone, Default)]
pub struct FilterState {
    /// State filter (at most one active at a time)
    pub state_filter: Option<StateFilter>,
    /// Tag filter (at most one tag at a time)
    pub tag_filter: Option<String>,
}

impl FilterState {
    pub fn is_active(&self) -> bool {
        self.state_filter.is_some() || self.tag_filter.is_some()
    }

    pub fn clear_all(&mut self) {
        self.state_filter = None;
        self.tag_filter = None;
    }

    pub fn clear_state(&mut self) {
        self.state_filter = None;
    }
}

/// An action that can be repeated with the `.` key
#[derive(Debug, Clone)]
pub enum RepeatableAction {
    /// Cycle state (Space)
    CycleState,
    /// Set absolute state (x=Done, b=Blocked, o=Todo, ~=Parked)
    SetState(TaskState),
    /// Tag edit: adds and removes (e.g., +cc +ready -design)
    TagEdit {
        adds: Vec<String>,
        removes: Vec<String>,
    },
    /// Dep edit: adds and removes (e.g., +EFF-014 -EFF-003)
    DepEdit {
        adds: Vec<String>,
        removes: Vec<String>,
    },
    /// Toggle cc tag
    ToggleCcTag,
    /// Enter edit mode on a region (e=Title, t=Tags, @=Refs, d=Deps, n=Note)
    EnterEdit(RepeatEditRegion),
}

/// Which region to re-enter edit mode for (used by RepeatableAction::EnterEdit)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepeatEditRegion {
    Title,
    Tags,
    Deps,
    Refs,
    Note,
}

/// A single entry in the dep popup's flattened display list
#[derive(Debug, Clone)]
pub enum DepPopupEntry {
    /// Section header ("Blocked by" or "Blocking")
    SectionHeader { label: &'static str },
    /// A dependency task entry
    Task {
        task_id: String,
        title: String,
        state: Option<TaskState>,
        track_id: Option<String>,
        /// Depth in the expand tree (0 = direct dep, 1 = dep's dep, etc.)
        depth: usize,
        /// Whether this entry has children that can be expanded
        has_children: bool,
        is_expanded: bool,
        /// True if this is a circular reference
        is_circular: bool,
        /// True if the task ID was not found in any track
        is_dangling: bool,
        /// True if this is in the "Blocked by" section (vs "Blocking")
        is_upstream: bool,
    },
    /// "(nothing)" placeholder for empty sections
    Nothing,
}

/// State for the dep popup overlay
#[derive(Debug, Clone)]
pub struct DepPopupState {
    /// The root task ID whose deps we're showing
    pub root_task_id: String,
    /// Track ID of the root task
    pub root_track_id: String,
    /// Flattened entries for display
    pub entries: Vec<DepPopupEntry>,
    /// Cursor index into entries (skips section headers)
    pub cursor: usize,
    /// Scroll offset
    pub scroll_offset: usize,
    /// Set of expanded entry keys (task_id + upstream/downstream)
    pub expanded: HashSet<String>,
    /// Set of task IDs visited during expansion (for cycle detection)
    pub visited: HashSet<String>,
    /// Inverse dep index: task_id -> list of task_ids that depend on it
    pub inverse_deps: HashMap<String, Vec<String>>,
}

/// Fixed color palette for tag color assignment
pub const TAG_COLOR_PALETTE: &[(&str, &str)] = &[
    ("red", "#FF4444"),
    ("yellow", "#FFD700"),
    ("green", "#44FF88"),
    ("cyan", "#44DDFF"),
    ("blue", "#4488FF"),
    ("purple", "#CC66FF"),
    ("pink", "#FB4196"),
    ("white", "#FFFFFF"),
    ("dim", "#5A5580"),
    ("text", "#A09BFE"),
];

/// State for the tag color editor popup
#[derive(Debug, Clone)]
pub struct TagColorPopupState {
    /// Sorted list of (tag_name, current_hex_color_or_none)
    pub tags: Vec<(String, Option<String>)>,
    /// Cursor index into the tag list
    pub cursor: usize,
    /// Scroll offset for long lists
    pub scroll_offset: usize,
    /// Whether the palette picker is open on the current tag
    pub picker_open: bool,
    /// Selected swatch index in the palette (0..PALETTE.len())
    pub picker_cursor: usize,
}

/// State for the prefix rename flow (edit → confirm → execute)
#[derive(Debug, Clone)]
pub struct PrefixRenameState {
    /// Track being renamed
    pub track_id: String,
    /// Track display name (for the confirmation popup)
    pub track_name: String,
    /// Current (old) prefix
    pub old_prefix: String,
    /// New prefix being entered
    pub new_prefix: String,
    /// Whether we're in the confirmation step (true) or still editing (false)
    pub confirming: bool,
    /// Blast radius counts (populated when entering confirmation)
    pub task_id_count: usize,
    pub dep_ref_count: usize,
    pub affected_track_count: usize,
    /// Validation error message (empty when valid)
    pub validation_error: String,
}

/// State for the project picker popup
#[derive(Debug, Clone)]
pub struct ProjectPickerState {
    /// List of project entries
    pub entries: Vec<crate::io::registry::ProjectEntry>,
    /// Cursor index
    pub cursor: usize,
    /// Scroll offset
    pub scroll_offset: usize,
    /// Sort mode: true = alphabetical, false = recent (default)
    pub sort_alpha: bool,
    /// Path of the currently open project (if any)
    pub current_project_path: Option<String>,
    /// Entry pending removal confirmation
    pub confirm_remove: Option<usize>,
}

impl ProjectPickerState {
    pub fn new(
        mut entries: Vec<crate::io::registry::ProjectEntry>,
        current_path: Option<String>,
    ) -> Self {
        // Default: sort by last_accessed_tui, most recent first
        entries.sort_by(|a, b| {
            let ta = a.last_accessed_tui.unwrap_or_default();
            let tb = b.last_accessed_tui.unwrap_or_default();
            tb.cmp(&ta)
        });
        Self {
            entries,
            cursor: 0,
            scroll_offset: 0,
            sort_alpha: false,
            current_project_path: current_path,
            confirm_remove: None,
        }
    }

    pub fn move_up(&mut self) {
        if !self.entries.is_empty() {
            if self.cursor == 0 {
                self.cursor = self.entries.len() - 1;
            } else {
                self.cursor -= 1;
            }
        }
        self.confirm_remove = None;
    }

    pub fn move_down(&mut self) {
        if !self.entries.is_empty() {
            self.cursor = (self.cursor + 1) % self.entries.len();
        }
        self.confirm_remove = None;
    }

    pub fn selected_entry(&self) -> Option<&crate::io::registry::ProjectEntry> {
        self.entries.get(self.cursor)
    }

    pub fn toggle_sort(&mut self) {
        self.sort_alpha = !self.sort_alpha;
        if self.sort_alpha {
            self.entries
                .sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        } else {
            self.entries.sort_by(|a, b| {
                let ta = a.last_accessed_tui.unwrap_or_default();
                let tb = b.last_accessed_tui.unwrap_or_default();
                tb.cmp(&ta)
            });
        }
        self.cursor = 0;
        self.scroll_offset = 0;
        self.confirm_remove = None;
    }

    pub fn remove_selected(&mut self) {
        if self.entries.is_empty() {
            return;
        }
        // If already confirming this index, do the removal
        if self.confirm_remove == Some(self.cursor) {
            let entry = &self.entries[self.cursor];
            crate::io::registry::remove_by_path(&entry.path);
            self.entries.remove(self.cursor);
            if self.cursor >= self.entries.len() && self.cursor > 0 {
                self.cursor -= 1;
            }
            self.confirm_remove = None;
        } else {
            self.confirm_remove = Some(self.cursor);
        }
    }
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
    /// Triage mode (inbox → track)
    Triage,
    /// Confirmation prompt (e.g., delete inbox item)
    Confirm,
    /// Multi-select mode for bulk operations (track view only)
    Select,
    /// Command palette mode (fuzzy action launcher)
    Command,
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
    /// Creating a new inbox item (title edit)
    NewInboxItem {
        /// Index where the placeholder was inserted
        index: usize,
    },
    /// Editing an existing inbox item's title
    ExistingInboxTitle {
        index: usize,
        original_title: String,
    },
    /// Editing an existing inbox item's tags
    ExistingInboxTags { index: usize, original_tags: String },
    /// Creating a new track (name edit in Tracks view)
    NewTrackName,
    /// Editing an existing track's name (in Tracks view)
    ExistingTrackName {
        track_id: String,
        original_name: String,
    },
    /// Selecting a tag for filter (using autocomplete)
    FilterTag,
    /// Bulk tag edit in SELECT mode (+tag -tag syntax)
    BulkTags,
    /// Bulk dep edit in SELECT mode (+ID -ID syntax)
    BulkDeps,
    /// Jump-to-task prompt (J key)
    JumpTo,
    /// Editing a track's prefix (P key in Tracks view)
    ExistingPrefix {
        track_id: String,
        original_prefix: String,
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
    /// Moving an inbox item
    InboxItem { original_index: usize },
    /// Bulk move of selected tasks within a track
    BulkTask {
        track_id: String,
        /// The removed tasks with their original backlog indices, in original order
        removed_tasks: Vec<(usize, Task)>,
        /// Current insertion point index in the (reduced) backlog
        insert_pos: usize,
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
        /// True if this task is shown only as ancestor context for a matching descendant
        /// (dimmed, non-selectable, cursor skips over it)
        is_context: bool,
    },
    /// The "── Parked ──" separator
    ParkedSeparator,
    /// Stand-in row during bulk move showing "━━━ N tasks ━━━"
    BulkMoveStandin { count: usize },
}

/// Main application state
pub struct App {
    pub project: Project,
    pub view: View,
    pub mode: Mode,
    pub should_quit: bool,
    /// Set to true after a project switch so the event loop can reinitialize the file watcher
    pub watcher_needs_restart: bool,
    pub theme: Theme,
    /// IDs of active tracks (in display order)
    pub active_track_ids: Vec<String>,
    /// Per-track view state
    pub track_states: HashMap<String, TrackViewState>,
    /// Cursor for tracks view
    pub tracks_cursor: usize,
    /// Minimum name column width for tracks view (prevents columns shifting left mid-session)
    pub tracks_name_col_min: usize,
    /// Cursor for inbox view
    pub inbox_cursor: usize,
    /// Cursor for recent view
    pub recent_cursor: usize,
    /// Scroll offset for inbox view
    pub inbox_scroll: usize,
    /// Index of inbox item whose note is being edited (None when not editing)
    pub inbox_note_index: Option<usize>,
    /// Scroll offset for the inline note editor in inbox view
    pub inbox_note_editor_scroll: usize,
    /// Scroll offset for recent view
    pub recent_scroll: usize,
    /// Help overlay visible
    pub show_help: bool,
    /// Scroll offset for help overlay
    pub help_scroll: usize,
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
    /// If true, status_message renders with error style (bright text on red bg)
    pub status_is_error: bool,
    /// Consecutive Esc presses in Navigate mode (shows quit hint at 5+)
    pub esc_streak: u8,
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
    /// Stack of (track_id, task_id) for parent breadcrumbs when drilling into subtasks
    pub detail_stack: Vec<(String, String)>,
    /// Autocomplete state (active during EDIT mode for certain fields)
    pub autocomplete: Option<AutocompleteState>,
    /// Screen position (x, y) where the edit text area starts, used to anchor autocomplete dropdown
    pub autocomplete_anchor: Option<(u16, u16)>,
    /// Inline edit history for undo/redo within an editing session
    pub edit_history: Option<EditHistory>,
    /// Selection anchor for text selection in edit mode (None = no selection)
    /// Selection range is from min(anchor, edit_cursor) to max(anchor, edit_cursor)
    pub edit_selection_anchor: Option<usize>,
    /// True when in edit mode for a new subtask and no character has been typed yet.
    /// Used to detect `-` as first keystroke for outdent behavior.
    pub edit_is_fresh: bool,
    /// Desired position (among active tracks) for new track insertion.
    /// Set by tracks_add_track / tracks_prepend / tracks_insert_after;
    /// consumed by the NewTrackName confirm handler.
    pub new_track_insert_pos: Option<usize>,
    /// Triage flow state (active during Mode::Triage)
    pub triage_state: Option<TriageState>,
    /// Confirmation prompt state (active during Mode::Confirm)
    pub confirm_state: Option<ConfirmState>,
    /// State color for current flash (None = undo yellow-orange default)
    pub flash_state: Option<TaskState>,
    /// Task ID to flash-highlight after undo/redo navigation
    pub flash_task_id: Option<String>,
    /// Multiple task IDs to flash (for bulk undo)
    pub flash_task_ids: HashSet<String>,
    /// Track ID to flash-highlight in tracks view after undo/redo
    pub flash_track_id: Option<String>,
    /// Detail region to flash (for field edit undo — flashes the specific region, not header)
    pub flash_detail_region: Option<DetailRegion>,
    /// When the flash started (for auto-clearing after timeout)
    pub flash_started: Option<Instant>,
    /// Pending section moves (grace period before moving tasks between sections)
    pub pending_moves: Vec<PendingMove>,
    /// Expanded task IDs in the Recent view (for tree structure)
    pub recent_expanded: HashSet<String>,
    /// Global filter state for track views (not persisted)
    pub filter_state: FilterState,
    /// True when 'f' prefix key has been pressed, waiting for second key
    pub filter_pending: bool,
    /// Selected task IDs in SELECT mode (empty = not in select mode)
    pub selection: HashSet<String>,
    /// Anchor flat-item index for V range select preview (None = not in range select mode)
    pub range_anchor: Option<usize>,
    /// Last repeatable action for `.` key (persists across tab switches)
    pub last_action: Option<RepeatableAction>,
    /// Command palette state (active during Mode::Command)
    pub command_palette: Option<super::command_actions::CommandPaletteState>,
    /// Dep popup state (overlay showing dependency relationships)
    pub dep_popup: Option<DepPopupState>,
    /// Tag color editor popup state
    pub tag_color_popup: Option<TagColorPopupState>,
    /// Prefix rename state (active during prefix rename flow)
    pub prefix_rename: Option<PrefixRenameState>,
    /// Project picker popup state
    pub project_picker: Option<ProjectPickerState>,
    /// Debug mode: show raw KeyEvent info in status row
    pub key_debug: bool,
    /// Last raw KeyEvent description (for debug display)
    pub last_key_event: Option<String>,
    /// Whether Kitty keyboard protocol is active
    pub kitty_enabled: bool,
    /// Horizontal scroll offset for single-line edit (character-based)
    pub edit_h_scroll: usize,
    /// Available width for edit field (set during render, read during input)
    pub last_edit_available_width: u16,
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
            if let Ok(meta) = std::fs::metadata(&path)
                && let Ok(mtime) = meta.modified()
            {
                track_mtimes.insert(tc.id.clone(), mtime);
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
            watcher_needs_restart: false,
            theme,
            active_track_ids,
            track_states,
            tracks_cursor: 0,
            tracks_name_col_min: 0,
            inbox_cursor: 0,
            recent_cursor: 0,
            inbox_scroll: 0,
            inbox_note_index: None,
            inbox_note_editor_scroll: 0,
            recent_scroll: 0,
            show_help: false,
            help_scroll: 0,
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
            status_is_error: false,
            esc_streak: 0,
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
            detail_stack: Vec::new(),
            autocomplete: None,
            autocomplete_anchor: None,
            edit_history: None,
            edit_selection_anchor: None,
            edit_is_fresh: false,
            new_track_insert_pos: None,
            triage_state: None,
            confirm_state: None,
            flash_state: None,
            flash_task_id: None,
            flash_task_ids: HashSet::new(),
            flash_track_id: None,
            flash_detail_region: None,
            flash_started: None,
            pending_moves: Vec::new(),
            recent_expanded: HashSet::new(),
            filter_state: FilterState::default(),
            filter_pending: false,
            selection: HashSet::new(),
            range_anchor: None,
            last_action: None,
            command_palette: None,
            dep_popup: None,
            tag_color_popup: None,
            prefix_rename: None,
            project_picker: None,
            key_debug: false,
            last_key_event: None,
            kitty_enabled: false,
            edit_h_scroll: 0,
            last_edit_available_width: 0,
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
        if let Some((start, end)) = self.edit_selection_range()
            && start != end
        {
            self.edit_buffer.drain(start..end);
            self.edit_cursor = start;
            self.edit_selection_anchor = None;
            return true;
        }
        self.edit_selection_anchor = None;
        false
    }

    /// Get the selected text in single-line edit mode (if any).
    pub fn get_selection_text(&self) -> Option<String> {
        let (start, end) = self.edit_selection_range()?;
        if start == end {
            return None;
        }
        Some(self.edit_buffer[start..end].to_string())
    }

    /// Start flashing a task (highlight after undo/redo navigation)
    pub fn flash_task(&mut self, task_id: &str) {
        self.flash_task_id = Some(task_id.to_string());
        self.flash_task_ids.clear();
        self.flash_track_id = None;
        self.flash_detail_region = None;
        self.flash_started = Some(Instant::now());
    }

    /// Start flashing multiple tasks (for bulk undo)
    pub fn flash_tasks(&mut self, task_ids: HashSet<String>) {
        self.flash_task_id = None;
        self.flash_task_ids = task_ids;
        self.flash_track_id = None;
        self.flash_started = Some(Instant::now());
    }

    /// Start flashing a track row in tracks view
    pub fn flash_track(&mut self, track_id: &str) {
        self.flash_track_id = Some(track_id.to_string());
        self.flash_task_id = None;
        self.flash_task_ids.clear();
        self.flash_started = Some(Instant::now());
    }

    /// Check if a specific task is currently flashing
    pub fn is_flashing(&self, task_id: &str) -> bool {
        if let Some(started) = self.flash_started {
            if started.elapsed() >= Duration::from_millis(300) {
                return false;
            }
            if self.flash_task_id.as_deref() == Some(task_id) {
                return true;
            }
            if self.flash_task_ids.contains(task_id) {
                return true;
            }
        }
        false
    }

    /// Check if a specific track is currently flashing (tracks view)
    pub fn is_track_flashing(&self, track_id: &str) -> bool {
        if let (Some(flash_id), Some(started)) = (&self.flash_track_id, self.flash_started) {
            flash_id == track_id && started.elapsed() < Duration::from_millis(300)
        } else {
            false
        }
    }

    /// Clear flash if the timeout has expired
    pub fn clear_expired_flash(&mut self) {
        if let Some(started) = self.flash_started
            && started.elapsed() >= Duration::from_millis(300)
        {
            self.flash_state = None;
            self.flash_task_id = None;
            self.flash_task_ids.clear();
            self.flash_track_id = None;
            self.flash_detail_region = None;
            self.flash_started = None;
        }
    }

    /// Check if a task has a pending move
    pub fn has_pending_move(&self, track_id: &str, task_id: &str) -> bool {
        self.pending_moves
            .iter()
            .any(|pm| pm.track_id == track_id && pm.task_id == task_id)
    }

    /// Cancel a pending move for a task. Returns the cancelled move if found.
    pub fn cancel_pending_move(&mut self, track_id: &str, task_id: &str) -> Option<PendingMove> {
        let idx = self
            .pending_moves
            .iter()
            .position(|pm| pm.track_id == track_id && pm.task_id == task_id)?;
        Some(self.pending_moves.remove(idx))
    }

    /// Execute a single pending move. Returns the track_id that was modified.
    fn execute_pending_move(&mut self, pm: &PendingMove) -> Option<String> {
        use crate::ops::task_ops::move_task_between_sections;
        let track = self.find_track_mut(&pm.track_id)?;
        match pm.kind {
            PendingMoveKind::ToDone => {
                let source_index = move_task_between_sections(
                    track,
                    &pm.task_id,
                    SectionKind::Backlog,
                    SectionKind::Done,
                )?;
                // Push SectionMove undo entry
                self.undo_stack.push(Operation::SectionMove {
                    track_id: pm.track_id.clone(),
                    task_id: pm.task_id.clone(),
                    from_section: SectionKind::Backlog,
                    to_section: SectionKind::Done,
                    from_index: source_index,
                });
                Some(pm.track_id.clone())
            }
            PendingMoveKind::ToBacklog => {
                // For reopen flush: move from Done to Backlog top
                // No extra undo entry — the existing Reopen operation handles full reversal
                move_task_between_sections(
                    track,
                    &pm.task_id,
                    SectionKind::Done,
                    SectionKind::Backlog,
                )?;
                // Now remove the resolved date (kept during grace period for sort stability)
                let track = self.find_track_mut(&pm.track_id)?;
                let task = crate::ops::task_ops::find_task_mut_in_track(track, &pm.task_id)?;
                task.metadata.retain(|m| m.key() != "resolved");
                task.mark_dirty();
                Some(pm.track_id.clone())
            }
        }
    }

    /// Flush all pending moves whose deadline has expired. Returns modified track IDs.
    pub fn flush_expired_pending_moves(&mut self) -> Vec<String> {
        let now = Instant::now();
        let expired: Vec<PendingMove> = self
            .pending_moves
            .iter()
            .filter(|pm| now >= pm.deadline)
            .cloned()
            .collect();
        self.pending_moves.retain(|pm| now < pm.deadline);

        let mut modified = Vec::new();
        for pm in &expired {
            if let Some(tid) = self.execute_pending_move(pm)
                && !modified.contains(&tid)
            {
                modified.push(tid);
            }
        }
        modified
    }

    /// Flush all pending moves immediately (used on view change, quit). Returns modified track IDs.
    pub fn flush_all_pending_moves(&mut self) -> Vec<String> {
        let all: Vec<PendingMove> = std::mem::take(&mut self.pending_moves);
        let mut modified = Vec::new();
        for pm in &all {
            if let Some(tid) = self.execute_pending_move(pm)
                && !modified.contains(&tid)
            {
                modified.push(tid);
            }
        }
        modified
    }

    /// Open the tag color editor popup
    pub fn open_tag_color_popup(&mut self) {
        let tag_names = self.collect_all_tags();
        let tags: Vec<(String, Option<String>)> = tag_names
            .into_iter()
            .map(|tag| {
                // Check config first (explicit user setting), then theme defaults
                let hex = self
                    .project
                    .config
                    .ui
                    .tag_colors
                    .get(&tag)
                    .cloned()
                    .or_else(|| {
                        self.theme.tag_colors.get(&tag).and_then(|color| {
                            if let ratatui::style::Color::Rgb(r, g, b) = color {
                                Some(format!("#{:02X}{:02X}{:02X}", r, g, b))
                            } else {
                                None
                            }
                        })
                    });
                (tag, hex)
            })
            .collect();
        self.tag_color_popup = Some(TagColorPopupState {
            tags,
            cursor: 0,
            scroll_offset: 0,
            picker_open: false,
            picker_cursor: 0,
        });
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

        // Tags from UI default_tags
        for tag in &self.project.config.ui.default_tags {
            tags.insert(tag.clone());
        }

        // Tags from all tasks across all tracks
        for (_, track) in &self.project.tracks {
            Self::collect_tags_from_tasks(track.backlog(), &mut tags);
            Self::collect_tags_from_tasks(track.parked(), &mut tags);
            Self::collect_tags_from_tasks(track.done(), &mut tags);
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
            Self::collect_ids_from_tasks(track.backlog(), &mut ids);
            Self::collect_ids_from_tasks(track.parked(), &mut ids);
            Self::collect_ids_from_tasks(track.done(), &mut ids);
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

    /// Collect all task IDs across active tracks only (for jump-to-task).
    /// Each entry is "ID  title" for display in autocomplete.
    pub fn collect_active_track_task_ids(&self) -> Vec<String> {
        let mut entries: Vec<String> = Vec::new();
        for track_id in &self.active_track_ids {
            if let Some(track) = Self::find_track_in_project(&self.project, track_id) {
                Self::collect_id_title_from_tasks(track.backlog(), &mut entries);
                Self::collect_id_title_from_tasks(track.parked(), &mut entries);
                Self::collect_id_title_from_tasks(track.done(), &mut entries);
            }
        }
        entries.sort();
        entries
    }

    fn collect_id_title_from_tasks(tasks: &[Task], entries: &mut Vec<String>) {
        for task in tasks {
            if let Some(ref id) = task.id {
                entries.push(format!("{}  {}", id, task.title));
            }
            Self::collect_id_title_from_tasks(&task.subtasks, entries);
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

    /// Find which active track contains a given task ID.
    /// Returns the track_id if found.
    pub fn find_task_track_id(&self, task_id: &str) -> Option<String> {
        for track_id in &self.active_track_ids {
            if let Some(track) = Self::find_track_in_project(&self.project, track_id)
                && crate::ops::task_ops::find_task_in_track(track, task_id).is_some()
            {
                return Some(track_id.clone());
            }
        }
        None
    }

    /// Jump to a task by ID: switch track if needed, expand parent chain, move cursor.
    /// Returns true if the jump succeeded.
    pub fn jump_to_task(&mut self, task_id: &str) -> bool {
        let target_track_id = match self.find_task_track_id(task_id) {
            Some(id) => id,
            None => return false,
        };

        // Switch to the target track's tab
        let track_idx = match self
            .active_track_ids
            .iter()
            .position(|id| id == &target_track_id)
        {
            Some(idx) => idx,
            None => return false,
        };
        self.close_detail_fully();
        self.view = View::Track(track_idx);

        // Expand parent chain: for "EFF-014.2.1", expand "EFF-014" and "EFF-014.2"
        self.expand_parent_chain(&target_track_id, task_id);

        // Build flat items and find the target task
        let flat_items = self.build_flat_items(&target_track_id);
        let track = match Self::find_track_in_project(&self.project, &target_track_id) {
            Some(t) => t,
            None => return false,
        };
        for (i, item) in flat_items.iter().enumerate() {
            if let FlatItem::Task { section, path, .. } = item
                && let Some(task) = resolve_task_from_flat(track, *section, path)
                && task.id.as_deref() == Some(task_id)
            {
                let state = self.get_track_state(&target_track_id);
                state.cursor = i;
                return true;
            }
        }
        false
    }

    /// Expand the parent chain for a task ID so it becomes visible in the flat list.
    /// For "EFF-014.2.1", expands "EFF-014" and "EFF-014.2".
    fn expand_parent_chain(&mut self, track_id: &str, task_id: &str) {
        // Walk up the ID hierarchy: "A.B.C" → expand "A" then "A.B"
        let parts: Vec<&str> = task_id.split('.').collect();
        if parts.len() <= 1 {
            return; // top-level task, nothing to expand
        }

        // Collect ancestor IDs that exist in the track
        let mut ancestors_to_expand = Vec::new();
        if let Some(track) = Self::find_track_in_project(&self.project, track_id) {
            for i in 1..parts.len() {
                let ancestor_id = parts[..i].join(".");
                if crate::ops::task_ops::find_task_in_track(track, &ancestor_id).is_some() {
                    ancestors_to_expand.push(ancestor_id);
                }
            }
        }

        // Now expand them (separate borrow)
        let state = self.get_track_state(track_id);
        for ancestor_id in ancestors_to_expand {
            state.expanded.insert(ancestor_id);
        }
    }

    /// Build the inverse dependency index: for each task ID, which tasks depend on it.
    pub fn build_dep_index(project: &Project) -> HashMap<String, Vec<String>> {
        let mut index: HashMap<String, Vec<String>> = HashMap::new();
        for (_, track) in &project.tracks {
            for node in &track.nodes {
                if let crate::model::TrackNode::Section { tasks, .. } = node {
                    Self::collect_deps_recursive(tasks, &mut index);
                }
            }
        }
        index
    }

    fn collect_deps_recursive(tasks: &[Task], index: &mut HashMap<String, Vec<String>>) {
        for task in tasks {
            if let Some(task_id) = &task.id {
                for m in &task.metadata {
                    if let Metadata::Dep(deps) = m {
                        for dep_id in deps {
                            index
                                .entry(dep_id.clone())
                                .or_default()
                                .push(task_id.clone());
                        }
                    }
                }
            }
            Self::collect_deps_recursive(&task.subtasks, index);
        }
    }

    /// Open the dep popup for a given task
    pub fn open_dep_popup(&mut self, track_id: &str, task_id: &str) {
        let inverse_deps = Self::build_dep_index(&self.project);
        let mut state = DepPopupState {
            root_task_id: task_id.to_string(),
            root_track_id: track_id.to_string(),
            entries: Vec::new(),
            cursor: 0,
            scroll_offset: 0,
            expanded: HashSet::new(),
            visited: HashSet::new(),
            inverse_deps,
        };
        // Build the entry list
        self.rebuild_dep_popup_entries(&mut state);
        // Set initial cursor to first selectable entry
        state.cursor = state
            .entries
            .iter()
            .position(|e| matches!(e, DepPopupEntry::Task { .. }))
            .unwrap_or(0);
        self.dep_popup = Some(state);
    }

    /// Rebuild the flattened entry list for the dep popup.
    /// Called on open and after expand/collapse.
    pub fn rebuild_dep_popup_entries(&self, state: &mut DepPopupState) {
        let task_id = state.root_task_id.clone();
        state.entries.clear();

        // Gather direct upstream deps (what this task depends on)
        let mut upstream_ids: Vec<String> = Vec::new();
        for (_, track) in &self.project.tracks {
            if let Some(task) = crate::ops::task_ops::find_task_in_track(track, &task_id) {
                for m in &task.metadata {
                    if let Metadata::Dep(deps) = m {
                        upstream_ids.extend(deps.iter().cloned());
                    }
                }
                break;
            }
        }

        // Gather direct downstream deps (what this task blocks)
        let downstream_ids: Vec<String> = state
            .inverse_deps
            .get(&task_id)
            .cloned()
            .unwrap_or_default();

        // Auto-expand logic: 1-2 entries → expand one level, 3+ → collapsed
        let auto_expand_upstream = upstream_ids.len() <= 2;
        let auto_expand_downstream = downstream_ids.len() <= 2;
        if state.expanded.is_empty() {
            // Only auto-expand on initial open
            if auto_expand_upstream {
                for id in &upstream_ids {
                    state.expanded.insert(format!("up:{}", id));
                }
            }
            if auto_expand_downstream {
                for id in &downstream_ids {
                    state.expanded.insert(format!("down:{}", id));
                }
            }
        }

        // "Blocked by" section
        state.entries.push(DepPopupEntry::SectionHeader {
            label: "Blocked by",
        });
        if upstream_ids.is_empty() {
            state.entries.push(DepPopupEntry::Nothing);
        } else {
            for dep_id in &upstream_ids {
                let mut visited = HashSet::new();
                visited.insert(task_id.to_string());
                self.add_dep_entry(state, dep_id, 0, true, &mut visited);
            }
        }

        // "Blocking" section
        state
            .entries
            .push(DepPopupEntry::SectionHeader { label: "Blocking" });
        if downstream_ids.is_empty() {
            state.entries.push(DepPopupEntry::Nothing);
        } else {
            for dep_id in &downstream_ids {
                let mut visited = HashSet::new();
                visited.insert(task_id.to_string());
                self.add_dep_entry(state, dep_id, 0, false, &mut visited);
            }
        }
    }

    /// Add a single dep entry and its expanded children recursively
    fn add_dep_entry(
        &self,
        state: &mut DepPopupState,
        dep_id: &str,
        depth: usize,
        is_upstream: bool,
        visited: &mut HashSet<String>,
    ) {
        // Cycle detection
        if visited.contains(dep_id) {
            state.entries.push(DepPopupEntry::Task {
                task_id: dep_id.to_string(),
                title: String::new(),
                state: None,
                track_id: None,
                depth,
                has_children: false,
                is_expanded: false,
                is_circular: true,
                is_dangling: false,
                is_upstream,
            });
            return;
        }

        // Find the task across all tracks
        let mut found_task: Option<(&str, &Task)> = None;
        for (tid, track) in &self.project.tracks {
            if let Some(task) = crate::ops::task_ops::find_task_in_track(track, dep_id) {
                found_task = Some((tid.as_str(), task));
                break;
            }
        }

        if let Some((found_track_id, task)) = found_task {
            // Determine if this entry has children (further deps to explore)
            let children_ids = if is_upstream {
                // In "Blocked by": children are what this dep itself depends on
                let mut ids = Vec::new();
                for m in &task.metadata {
                    if let Metadata::Dep(deps) = m {
                        ids.extend(deps.iter().cloned());
                    }
                }
                ids
            } else {
                // In "Blocking": children are what this dep is also blocking
                state.inverse_deps.get(dep_id).cloned().unwrap_or_default()
            };
            let has_children = !children_ids.is_empty();

            let expand_key = format!("{}:{}", if is_upstream { "up" } else { "down" }, dep_id);
            let is_expanded = state.expanded.contains(&expand_key);

            state.entries.push(DepPopupEntry::Task {
                task_id: dep_id.to_string(),
                title: task.title.clone(),
                state: Some(task.state),
                track_id: Some(found_track_id.to_string()),
                depth,
                has_children,
                is_expanded,
                is_circular: false,
                is_dangling: false,
                is_upstream,
            });

            // Recurse into expanded children
            if is_expanded && has_children {
                visited.insert(dep_id.to_string());
                for child_id in &children_ids {
                    self.add_dep_entry(state, child_id, depth + 1, is_upstream, visited);
                }
                visited.remove(dep_id);
            }
        } else {
            // Dangling reference
            state.entries.push(DepPopupEntry::Task {
                task_id: dep_id.to_string(),
                title: String::new(),
                state: None,
                track_id: None,
                depth,
                has_children: false,
                is_expanded: false,
                is_circular: false,
                is_dangling: true,
                is_upstream,
            });
        }
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

    /// Show a save error as a status message, if any.
    pub fn show_save_error(&mut self, result: Result<(), Box<dyn std::error::Error>>) {
        if let Err(e) = result {
            self.status_message = Some(format!("Save error: {}", e));
        }
    }

    /// Save the inbox to disk with file locking. Records save time.
    pub fn save_inbox(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let inbox = self.project.inbox.as_ref().ok_or("no inbox loaded")?;
        let _lock = FileLock::acquire_default(&self.project.frame_dir)?;
        project_io::save_inbox(&self.project.frame_dir, inbox)?;
        self.last_save_at = Some(Instant::now());
        Ok(())
    }

    /// Save a track to disk with file locking. Records save time and mtime.
    pub fn save_track(&mut self, track_id: &str) -> Result<(), Box<dyn std::error::Error>> {
        let file = self
            .track_file(track_id)
            .ok_or("track not found")?
            .to_string();
        let track =
            Self::find_track_in_project(&self.project, track_id).ok_or("track not found")?;
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
            _ => None,
        };
        let editing_track_id = match &self.edit_target {
            Some(EditTarget::NewTask { track_id, .. })
            | Some(EditTarget::ExistingTitle { track_id, .. })
            | Some(EditTarget::ExistingTags { track_id, .. }) => Some(track_id.clone()),
            _ => None,
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
                && let Ok(text) = std::fs::read_to_string(path)
            {
                let new_track = parse_track(&text);

                // Check if the edited task was modified externally
                if editing_track_id.as_deref() == Some(&track_id)
                    && let Some(ref edit_task_id) = editing_task_id
                {
                    // Check if the task exists in the new track and has different content
                    if let Some(old_track) = Self::find_track_in_project(&self.project, &track_id) {
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
        if task
            .metadata
            .iter()
            .any(|m| matches!(m, Metadata::Added(_)))
        {
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

    /// Close detail view fully: clear state and stack
    pub fn close_detail_fully(&mut self) {
        self.detail_state = None;
        self.detail_stack.clear();
    }

    /// Open the detail view for a task
    pub fn open_detail(&mut self, track_id: String, task_id: String) {
        // If already in detail view, push current onto stack for back-navigation
        let return_view = if let View::Detail {
            track_id: ref cur_track,
            task_id: ref cur_task,
        } = self.view
        {
            self.detail_stack
                .push((cur_track.clone(), cur_task.clone()));
            // Preserve the return_view from current detail state
            self.detail_state
                .as_ref()
                .map(|ds| ds.return_view.clone())
                .unwrap_or(ReturnView::Track(0))
        } else {
            match &self.view {
                View::Track(idx) => ReturnView::Track(*idx),
                View::Recent => ReturnView::Recent,
                _ => ReturnView::Track(0),
            }
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
            return_view,
            editing: false,
            edit_buffer: String::new(),
            edit_cursor_line: 0,
            edit_cursor_col: 0,
            edit_original: String::new(),
            subtask_cursor: 0,
            flat_subtask_ids: Vec::new(),
            multiline_selection_anchor: None,
            note_h_scroll: 0,
            total_lines: 0,
            note_view_line: None,
            note_header_line: None,
            note_content_end: 0,
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

        // Apply filter if active
        if self.filter_state.is_active() {
            apply_filter(&mut items, track, &self.filter_state, &self.project);
        }

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

/// Recursively flatten subtask IDs in depth-first order
pub fn flatten_subtask_ids(task: &Task) -> Vec<String> {
    let mut ids = Vec::new();
    flatten_subtask_ids_inner(&task.subtasks, &mut ids);
    ids
}

fn flatten_subtask_ids_inner(tasks: &[Task], ids: &mut Vec<String>) {
    for task in tasks {
        if let Some(ref id) = task.id {
            ids.push(id.clone());
        }
        flatten_subtask_ids_inner(&task.subtasks, ids);
    }
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
            is_context: false,
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

/// Check if a task matches the given filter criteria
fn task_matches_filter(task: &Task, filter: &FilterState, project: &Project) -> bool {
    // Check state filter
    if let Some(sf) = &filter.state_filter {
        let state_ok = match sf {
            StateFilter::Active => task.state == TaskState::Active,
            StateFilter::Todo => task.state == TaskState::Todo,
            StateFilter::Blocked => task.state == TaskState::Blocked,
            StateFilter::Parked => task.state == TaskState::Parked,
            StateFilter::Ready => {
                (task.state == TaskState::Todo || task.state == TaskState::Active)
                    && !has_unresolved_deps(task, project)
            }
        };
        if !state_ok {
            return false;
        }
    }

    // Check tag filter
    if let Some(ref tag) = filter.tag_filter
        && !task.tags.iter().any(|t| t == tag)
    {
        return false;
    }

    true
}

/// Check if a task has unresolved (non-done) dependencies
fn has_unresolved_deps(task: &Task, project: &Project) -> bool {
    use crate::ops::task_ops;
    for m in &task.metadata {
        if let Metadata::Dep(deps) = m {
            for dep_id in deps {
                for (_, track) in &project.tracks {
                    if let Some(dep_task) = task_ops::find_task_in_track(track, dep_id)
                        && dep_task.state != TaskState::Done
                    {
                        return true;
                    }
                }
            }
        }
    }
    false
}

/// Check if a task or any of its subtasks (recursively) matches the filter
fn has_matching_descendant(task: &Task, filter: &FilterState, project: &Project) -> bool {
    for sub in &task.subtasks {
        if task_matches_filter(sub, filter, project) {
            return true;
        }
        if has_matching_descendant(sub, filter, project) {
            return true;
        }
    }
    false
}

/// Apply filter to the flat items list: remove non-matching tasks and mark context-only ancestors.
/// A task is kept if it matches the filter OR if it has a matching descendant (shown as context).
fn apply_filter(items: &mut Vec<FlatItem>, track: &Track, filter: &FilterState, project: &Project) {
    // First pass: determine which items match and which are context-only
    let mut keep = vec![false; items.len()];
    let mut context = vec![false; items.len()];

    for (i, item) in items.iter().enumerate() {
        if let FlatItem::Task { section, path, .. } = item
            && let Some(task) = resolve_task_from_flat(track, *section, path)
        {
            if task_matches_filter(task, filter, project) {
                keep[i] = true;
                // Mark all ancestors as context (they need to be shown for hierarchy)
                mark_ancestors_kept(items, i, &mut keep, &mut context);
            } else if has_matching_descendant(task, filter, project) {
                keep[i] = true;
                context[i] = true;
            }
        }
        // ParkedSeparator: keep if any parked task is kept (handled below)
    }

    // Keep ParkedSeparator only if at least one Parked task is kept
    for (i, item) in items.iter().enumerate() {
        if matches!(item, FlatItem::ParkedSeparator) {
            let has_parked = items[i + 1..].iter().enumerate().any(|(j, fi)| {
                matches!(
                    fi,
                    FlatItem::Task {
                        section: SectionKind::Parked,
                        ..
                    }
                ) && keep[i + 1 + j]
            });
            keep[i] = has_parked;
        }
    }

    // Apply: set is_context flags and remove non-kept items
    let mut idx = 0;
    items.retain_mut(|item| {
        let retained = keep[idx];
        if retained
            && let FlatItem::Task {
                is_context: ctx, ..
            } = item
        {
            *ctx = context[idx];
        }
        idx += 1;
        retained
    });
}

/// Mark ancestor items as kept (context) by walking up the path hierarchy
fn mark_ancestors_kept(
    items: &[FlatItem],
    child_idx: usize,
    keep: &mut [bool],
    context: &mut [bool],
) {
    if let FlatItem::Task { path, section, .. } = &items[child_idx] {
        if path.len() <= 1 {
            return; // top-level task, no ancestors
        }
        let child_section = *section;
        // Walk backwards to find ancestor items (shorter path prefixes in the same section)
        for ancestor_len in 1..path.len() {
            let ancestor_path = &path[..ancestor_len];
            for (j, item) in items[..child_idx].iter().enumerate().rev() {
                if let FlatItem::Task {
                    path: p,
                    section: s,
                    ..
                } = item
                    && *s == child_section
                    && p.as_slice() == ancestor_path
                {
                    if !keep[j] {
                        keep[j] = true;
                        context[j] = true;
                    }
                    break;
                }
            }
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
        View::Detail { track_id, .. } => ("track".to_string(), track_id.clone()),
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

/// Set the terminal window/tab title via OSC 0.
pub fn set_window_title(name: &str) {
    let _ = write!(io::stdout(), "\x1b]0;frame · {}\x07", name);
    let _ = io::stdout().flush();
}

/// Clear the terminal window/tab title (restore default).
pub fn clear_window_title() {
    let _ = write!(io::stdout(), "\x1b]0;\x07");
    let _ = io::stdout().flush();
}

/// Run the TUI application.
/// If `project_dir_override` is set, use that as the starting directory.
pub fn run(project_dir_override: Option<&str>) -> Result<(), Box<dyn std::error::Error>> {
    // Discover and load project
    let start_dir = match project_dir_override {
        Some(dir) => std::fs::canonicalize(dir)
            .map_err(|e| format!("cannot resolve -C path '{}': {}", dir, e))?,
        None => std::env::current_dir()?,
    };

    // If we can't find a project, check the registry for the picker
    let root = match discover_project(&start_dir) {
        Ok(root) => root,
        Err(_) => {
            // No project found — launch project picker
            return run_project_picker();
        }
    };
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

    // Auto-register and touch TUI timestamp
    crate::io::registry::register_project(&project.config.project.name, &project.root);
    crate::io::registry::touch_tui(&project.root);

    let mut app = App::new(project);

    // Restore saved UI state
    restore_ui_state(&mut app);

    // Start file watcher (non-fatal if it fails)
    let watcher = FrameWatcher::start(&app.project.frame_dir).ok();

    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;

    // Kitty keyboard protocol: enabled by default (detection via
    // supports_keyboard_enhancement() is unreliable). Can be overridden
    // in project.toml with [ui] kitty_keyboard = true/false.
    let kitty_setting = app.project.config.ui.kitty_keyboard.unwrap_or(true);
    let kitty_enabled = if kitty_setting {
        execute!(
            stdout,
            PushKeyboardEnhancementFlags(
                KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
                    | KeyboardEnhancementFlags::REPORT_EVENT_TYPES
                    | KeyboardEnhancementFlags::REPORT_ALL_KEYS_AS_ESCAPE_CODES
            )
        )
        .is_ok()
    } else {
        false
    };

    // Bracketed paste: terminal signals paste start/end so we get a single
    // Event::Paste(String) instead of individual key events for each character.
    let _ = execute!(stdout, EnableBracketedPaste);

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    terminal.clear()?;

    // Install panic hook to restore terminal on panic
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic_info| {
        let _ = write!(io::stdout(), "\x1b]0;\x07");
        let _ = io::stdout().flush();
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), DisableBracketedPaste);
        let _ = execute!(io::stdout(), PopKeyboardEnhancementFlags);
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
        original_hook(panic_info);
    }));

    // Record kitty protocol status on app for debug display
    app.kitty_enabled = kitty_enabled;

    // Set terminal window title
    set_window_title(&app.project.config.project.name);

    // Run event loop
    let result = run_event_loop(&mut terminal, &mut app, watcher);

    // Save UI state before exit
    save_ui_state(&app);

    // Restore terminal
    clear_window_title();
    disable_raw_mode()?;
    let _ = execute!(terminal.backend_mut(), DisableBracketedPaste);
    let _ = execute!(terminal.backend_mut(), PopKeyboardEnhancementFlags);
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    result
}

/// Launch the TUI in project-picker-only mode (when no project is found).
fn run_project_picker() -> Result<(), Box<dyn std::error::Error>> {
    let reg = crate::io::registry::read_registry();
    if reg.projects.is_empty() {
        println!("No projects registered.");
        println!();
        println!("Run `fr init` in a project directory to get started,");
        println!("or `fr projects add <path>` to register an existing project.");
        return Ok(());
    }

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

    let mut picker = ProjectPickerState::new(reg.projects, None);
    let theme = super::theme::Theme::default();

    let selected_path = loop {
        terminal.draw(|frame| {
            let area = frame.area();
            // Dark background
            frame.render_widget(
                ratatui::widgets::Block::default()
                    .style(ratatui::style::Style::default().bg(theme.background)),
                area,
            );
            render::project_picker::render_project_picker_standalone(frame, &picker, &theme, area);
        })?;

        if crossterm::event::poll(Duration::from_millis(250))?
            && let crossterm::event::Event::Key(key) = crossterm::event::read()?
            && (key.kind == crossterm::event::KeyEventKind::Press
                || (key.kind == crossterm::event::KeyEventKind::Repeat
                    && matches!(
                        key.code,
                        crossterm::event::KeyCode::Up
                            | crossterm::event::KeyCode::Down
                            | crossterm::event::KeyCode::Char('j')
                            | crossterm::event::KeyCode::Char('k')
                    )))
        {
            use crossterm::event::{KeyCode, KeyModifiers};
            match (key.modifiers, key.code) {
                (_, KeyCode::Char('q')) | (_, KeyCode::Esc) => break None,
                (_, KeyCode::Up) | (_, KeyCode::Char('k')) => picker.move_up(),
                (_, KeyCode::Down) | (_, KeyCode::Char('j')) => picker.move_down(),
                (_, KeyCode::Enter) => {
                    if let Some(entry) = picker.selected_entry() {
                        break Some(entry.path.clone());
                    }
                }
                (KeyModifiers::SHIFT, KeyCode::Char('X'))
                | (KeyModifiers::NONE, KeyCode::Char('X')) => {
                    picker.remove_selected();
                }
                (_, KeyCode::Char('s')) => picker.toggle_sort(),
                _ => {}
            }
        }
    };

    // Restore terminal
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    // If a project was selected, load and run it
    if let Some(path) = selected_path {
        let root_path = std::path::PathBuf::from(&path);
        if !root_path.join("frame").exists() {
            return Err(format!("project not found at {}", path).into());
        }
        crate::io::registry::touch_tui(&root_path);
        return run(Some(&path));
    }

    Ok(())
}

/// Format a KeyEvent into a compact debug string like "Left mod=CTRL|ALT" or "Char('a') mod=NONE"
fn format_key_debug(key: &crossterm::event::KeyEvent) -> String {
    use crossterm::event::KeyModifiers;
    let code = format!("{:?}", key.code);
    let mut mods = Vec::new();
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        mods.push("CTRL");
    }
    if key.modifiers.contains(KeyModifiers::ALT) {
        mods.push("ALT");
    }
    if key.modifiers.contains(KeyModifiers::SHIFT) {
        mods.push("SHIFT");
    }
    if key.modifiers.contains(KeyModifiers::SUPER) {
        mods.push("SUPER");
    }
    if key.modifiers.contains(KeyModifiers::HYPER) {
        mods.push("HYPER");
    }
    if key.modifiers.contains(KeyModifiers::META) {
        mods.push("META");
    }
    let mod_str = if mods.is_empty() {
        "NONE".to_string()
    } else {
        mods.join("|")
    };
    format!("{} mod={} state={:?}", code, mod_str, key.state)
}

fn run_event_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
    mut watcher: Option<FrameWatcher>,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut save_counter = 0u32;
    loop {
        // Reinitialize file watcher after project switch
        if app.watcher_needs_restart {
            app.watcher_needs_restart = false;
            watcher = FrameWatcher::start(&app.project.frame_dir).ok();
        }
        app.clear_expired_flash();

        // Flush expired pending moves (only in Navigate mode, like pending_reload)
        if app.mode == Mode::Navigate && !app.pending_moves.is_empty() {
            let modified = app.flush_expired_pending_moves();
            for tid in &modified {
                let _ = app.save_track(tid);
            }
        }

        terminal.draw(|frame| render::render(frame, app))?;

        // Poll for file watcher events
        if let Some(w) = watcher.as_ref() {
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
                    if matches!(
                        app.mode,
                        Mode::Edit | Mode::Move | Mode::Triage | Mode::Confirm | Mode::Command
                    ) {
                        // Queue reload for when we leave modal mode
                        app.pending_reload_paths.extend(all_paths);
                    } else {
                        handle_external_reload(app, &all_paths);
                    }
                }
            }
        }

        if event::poll(Duration::from_millis(250))? {
            let old_view = app.view.clone();
            let evt = event::read()?;
            let handled = match evt {
                Event::Key(key)
                    if key.kind == KeyEventKind::Press
                        || (key.kind == KeyEventKind::Repeat
                            && is_repeatable_key(&app.mode, &key)) =>
                {
                    // Capture raw key event for debug display
                    if app.key_debug {
                        app.last_key_event = Some(format_key_debug(&key));
                    }
                    input::handle_key(app, key);
                    true
                }
                Event::Paste(text) => {
                    input::handle_paste(app, &text);
                    true
                }
                _ => false,
            };

            if handled {
                // Flush all pending moves on view change
                if app.view != old_view && !app.pending_moves.is_empty() {
                    let modified = app.flush_all_pending_moves();
                    for tid in &modified {
                        let _ = app.save_track(tid);
                    }
                }

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
        }

        if app.should_quit {
            // Flush all pending moves before exit
            let modified = app.flush_all_pending_moves();
            for tid in &modified {
                let _ = app.save_track(tid);
            }
            break;
        }
    }
    Ok(())
}

/// Whether a key repeat event should be processed. In typing modes all keys
/// repeat; in navigation modes only movement keys repeat.
fn is_repeatable_key(mode: &Mode, key: &crossterm::event::KeyEvent) -> bool {
    use crossterm::event::KeyCode;
    match mode {
        Mode::Edit | Mode::Search | Mode::Triage | Mode::Command => true,
        _ => matches!(
            key.code,
            KeyCode::Up
                | KeyCode::Down
                | KeyCode::Left
                | KeyCode::Right
                | KeyCode::PageUp
                | KeyCode::PageDown
                | KeyCode::Home
                | KeyCode::End
                | KeyCode::Tab
                | KeyCode::BackTab
                | KeyCode::Char('j')
                | KeyCode::Char('k')
                | KeyCode::Char('h')
                | KeyCode::Char('l')
        ),
    }
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
    // Auto-clean after external reload
    run_auto_clean(app);
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
    // Auto-clean after reload
    run_auto_clean(app);
}

/// Run auto-clean on the project after external changes are detected.
/// Assigns missing IDs/dates and saves affected tracks. Shows status message if anything changed.
fn run_auto_clean(app: &mut App) {
    use crate::ops::clean::clean_project;

    let result = clean_project(&mut app.project);

    let has_changes = !result.ids_assigned.is_empty()
        || !result.dates_assigned.is_empty()
        || !result.duplicates_resolved.is_empty();

    if has_changes {
        // Collect affected track IDs
        let mut affected_tracks: std::collections::HashSet<String> =
            std::collections::HashSet::new();
        for id_a in &result.ids_assigned {
            affected_tracks.insert(id_a.track_id.clone());
        }
        for date_a in &result.dates_assigned {
            affected_tracks.insert(date_a.track_id.clone());
        }
        for dup in &result.duplicates_resolved {
            affected_tracks.insert(dup.track_id.clone());
        }

        // Save affected tracks
        for track_id in &affected_tracks {
            let _ = app.save_track(track_id);
        }

        // Add sync marker to undo stack so user can't undo past the external change
        app.undo_stack.push(crate::tui::undo::Operation::SyncMarker);

        // Show subtle status message
        let count = result.ids_assigned.len()
            + result.dates_assigned.len()
            + result.duplicates_resolved.len();
        app.status_message = Some(format!(
            "Auto-cleaned: {} fix{}",
            count,
            if count == 1 { "" } else { "es" }
        ));
    }
}
