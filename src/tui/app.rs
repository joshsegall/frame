use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
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
use ratatui::text::Line;

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
    /// Board view (kanban-style cross-track view)
    Board,
    /// Inbox
    Inbox,
    /// Recently completed tasks
    Recent,
    /// Detail view for a single task
    Detail { track_id: String, task_id: String },
    /// Project-wide search results
    Search,
}

/// Which column the cursor is in on the board view
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoardColumn {
    Ready,
    InProgress,
    Done,
}

impl BoardColumn {
    pub fn index(self) -> usize {
        match self {
            BoardColumn::Ready => 0,
            BoardColumn::InProgress => 1,
            BoardColumn::Done => 2,
        }
    }

    pub fn from_index(i: usize) -> Self {
        match i {
            0 => BoardColumn::Ready,
            1 => BoardColumn::InProgress,
            _ => BoardColumn::Done,
        }
    }
}

/// Board filtering mode
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BoardMode {
    Cc,
    All,
}

/// A single item in a board column's flat list
#[derive(Debug, Clone)]
pub enum BoardItem {
    TrackHeader {
        track_name: String,
    },
    Task {
        track_id: String,
        task_id: String,
        title: String,
        id_display: String,
        state: TaskState,
        tags: Vec<String>,
    },
}

/// Board view state
#[derive(Debug, Clone)]
pub struct BoardState {
    pub focus_column: BoardColumn,
    /// Cursor index within each column (independent)
    pub cursor: [usize; 3],
    /// Scroll offset for each column (independent)
    pub scroll: [usize; 3],
    pub mode: BoardMode,
    /// Number of visible columns in the current layout (set by renderer)
    pub visible_columns: usize,
    /// Tasks pinned to a column during grace period after state change.
    /// Maps (track_id, task_id) → (original effective state, deadline).
    pub column_pins: Vec<BoardColumnPin>,
}

/// Keeps a task visually pinned to its current board column during the grace period
/// after a state change (e.g. Todo→Active stays in Ready column briefly).
#[derive(Debug, Clone)]
pub struct BoardColumnPin {
    pub track_id: String,
    pub task_id: String,
    /// The state to use for column placement during the grace period
    pub pinned_state: TaskState,
    pub deadline: std::time::Instant,
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

/// Source of a search result (which collection it came from)
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SearchResultKind {
    Track { track_idx: usize, track_id: String },
    Inbox { item_index: usize },
    Archive { track_id: String },
}

/// A field annotation line shown below a search result when the match is in a non-title field
#[derive(Debug, Clone)]
pub struct MatchAnnotation {
    pub field: crate::ops::search::MatchField,
    pub snippet: String,
}

/// A single search result item displayed in the Search view
#[derive(Debug, Clone)]
pub struct SearchResultItem {
    pub kind: SearchResultKind,
    pub task_id: String,
    pub title: String,
    pub state: Option<TaskState>,
    pub tags: Vec<String>,
    pub annotations: Vec<MatchAnnotation>,
    pub title_matches: bool,
    pub id_matches: bool,
}

/// Grouped project search results
#[derive(Debug, Clone)]
pub struct SearchResults {
    pub query: String,
    pub regex: Regex,
    pub items: Vec<SearchResultItem>,
    /// (start_index, label, match_count) for group headers
    pub groups: Vec<(usize, String, usize)>,
    pub cursor: usize,
    pub scroll_offset: usize,
    pub return_view: View,
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
    Board,
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
    /// Sticky column for visual-row cursor movement (in visual-column space)
    pub sticky_col: Option<usize>,
    /// Total rendered lines (set during render, used for scroll clamping)
    pub total_lines: usize,
    /// Virtual cursor line for note view-mode scrolling (None = not scrolling)
    pub note_view_line: Option<usize>,
    /// Line index of the note header in rendered content (set during render)
    pub note_header_line: Option<usize>,
    /// Last line index belonging to note content, before subtasks (set during render)
    pub note_content_end: usize,
    /// Which regions have non-empty content (parallel to `regions`, set during render)
    pub regions_populated: Vec<bool>,
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
        /// Section the (top-level) task lives in, so a Parked/Done task is moved
        /// into the same section of the target rather than reopened.
        section: SectionKind,
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
    DeleteTask { track_id: String, task_id: String },
    BulkDeleteTasks { task_ids: Vec<(String, String)> },
    PruneRecovery,
    UnarchiveTrack { track_id: String },
    ImportTasks { track_id: String, file_path: String },
}

/// A section move waiting out its grace period.
///
/// **One shape for every direction, deliberately.** This was four variants named
/// for their destinations — `ToDone`, `ToBacklog`, `ToParked`, `FromParked` —
/// which made coverage a matter of whoever remembered to list a case. Three
/// separate defects came out of that: a Parked task marked done went to the
/// Backlog (`12c0b57`), a Done task parked stayed in Done (`5eb069f`), and a Done
/// task reopened anywhere but the Board or Recent views stayed in Done. Each was
/// fixed by widening one variant, and the next gap simply moved elsewhere.
///
/// Carrying `from` and `to` instead means the destination is *computed* — by
/// `task_ops::canonical_section` — rather than selected from a list, so there is
/// no case left to forget.
#[derive(Debug, Clone)]
pub struct PendingMove {
    /// Where the task sits now.
    pub from: SectionKind,
    /// Where it goes when the grace period expires.
    pub to: SectionKind,
    /// Whether flushing pushes its own [`Operation::SectionMove`] undo entry.
    ///
    /// Not derivable from `from`/`to`: it depends on what the *scheduler* already
    /// recorded. `Operation::Reopen` puts the task back in the Done section
    /// itself, at its original index, so the recent-view reopen needs nothing
    /// more. `Operation::StateChange` restores state and the resolved date and
    /// leaves the task where it is — so every move scheduled alongside one needs
    /// this, or undo lands the task in the right state in the wrong section.
    pub push_undo: bool,
    pub track_id: String,
    pub task_id: String,
    pub deadline: Instant,
    /// The task state before this pending move was created (for board view grace period)
    pub old_state: Option<TaskState>,
}

impl PendingMove {
    /// Whether the task is on its way out of the Done section.
    ///
    /// It is still physically in Done until the grace period expires, so the
    /// Done column and the Recent view keep showing it — otherwise the row
    /// vanishes the instant you reopen it and reappears elsewhere, which is the
    /// jump the grace period exists to prevent.
    pub fn leaves_done(&self) -> bool {
        self.from == SectionKind::Done
    }

    /// Whether the task is heading somewhere it will be displayed by its old
    /// state — Done or Parked — so the grace period should keep showing it that
    /// way rather than flipping the moment the key is pressed.
    pub fn settles_out_of_backlog(&self) -> bool {
        self.to != SectionKind::Backlog
    }
}

/// A pending subtask hide with a grace period (subtask stays visible briefly after being marked done)
#[derive(Debug, Clone)]
pub struct PendingSubtaskHide {
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
            self.entries.sort_by_key(|a| a.name.to_lowercase());
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
    /// Import file path prompt (from palette)
    ImportFilePath { track_id: String },
}

/// State for MOVE mode
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MoveState {
    /// Moving a task within a track's backlog (supports reparenting)
    Task {
        track_id: String,
        task_id: String,
        original_parent_id: Option<String>,
        original_section: SectionKind,
        original_sibling_index: usize,
        original_depth: usize,
        /// Expand keys that were force-expanded to keep the moving task visible.
        /// These are removed from the expanded set when the task moves away or
        /// when the move is confirmed/cancelled.
        force_expanded: HashSet<String>,
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
    /// Summary row showing "X/Y done" for hidden done subtasks
    DoneSummary {
        depth: usize,
        done_count: usize,
        total_count: usize,
        ancestor_last: Vec<bool>,
    },
}

/// Which file a save was for.
///
/// `Ord` so [`App::unsaved`] iterates in a stable order: the indicator and the
/// exit report must not reshuffle their file list between frames or runs.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SaveTarget {
    Track(String),
    Inbox,
}

impl SaveTarget {
    /// How this file is named to the user, in messages and the recovery log.
    pub fn label(&self) -> String {
        match self {
            SaveTarget::Track(id) => format!("track {id}"),
            SaveTarget::Inbox => "inbox".to_string(),
        }
    }
}

/// A file whose in-memory content did not reach disk.
///
/// Its presence in [`App::unsaved`] is what stops an external reload from
/// overwriting the only copy that exists — see [`App::reload_changed_files`].
#[derive(Debug, Clone)]
pub struct UnsavedFile {
    /// The most recent error, for the recovery log and the exit report.
    pub error: String,
    /// Save attempts so far, including the first. Retry backoff reads this.
    pub attempts: usize,
    /// When the next retry is due.
    pub next_retry_at: Instant,
    /// A failure no retry can clear — see [`is_permanent`].
    pub permanent: bool,
    /// Whether this has been announced: shown in the indicator and written to
    /// the recovery log.
    ///
    /// One-shot. Once set, no further recovery entry is written for this file
    /// however many retries fail after it — a file retrying against an
    /// unwritable volume for an hour is one incident, not sixty. It resets only
    /// by the file leaving [`App::unsaved`], so a genuinely new incident later
    /// is recorded again.
    pub surfaced: bool,
}

impl UnsavedFile {
    /// Whether this failure is worth telling the user about yet.
    ///
    /// A save failure is never a *short* wait — `acquire_default` already blocked
    /// five seconds before giving up, so brief contention produces a slow save
    /// and never reaches this set at all. What is worth suppressing is narrower:
    /// a failure at the five-second mark that the retry a second later clears.
    /// Announcing that would be a flash of alarm and a junk recovery entry for a
    /// problem that fixed itself.
    ///
    /// So a transient failure gets one retry to prove itself, and an error no
    /// retry can clear is announced immediately — waiting on a second attempt
    /// that cannot succeed would only delay the news.
    pub fn worth_announcing(&self) -> bool {
        self.permanent || self.attempts >= 2
    }
}

/// Whether `frame/` cannot be written to, and why.
///
/// Probing at startup turns a discovery made at quit — the worst possible moment,
/// with a session's work behind it — into one made before the user types
/// anything. It is a real write rather than a permissions inspection, because
/// what matters is whether a write succeeds: a full disk, a read-only mount and a
/// directory owned by someone else all fail here and not all of them show up in
/// the mode bits.
fn probe_unwritable(frame_dir: &Path) -> Option<String> {
    let probe = frame_dir.join(".write-probe");
    match std::fs::write(&probe, b"") {
        Ok(()) => {
            let _ = std::fs::remove_file(&probe);
            None
        }
        Err(e) => Some(e.to_string()),
    }
}

/// Where work that never reached disk is copied at exit.
///
/// A dotfile directly inside `frame/`, so `gitignore_pattern`'s `frame/.*` covers
/// it with no `.gitignore` change — nothing here is ever meant to be committed.
pub const RESCUE_DIR: &str = ".rescue";

/// The first retry delay. Doubles per failed attempt up to [`RETRY_BACKOFF_MAX`].
pub const RETRY_BACKOFF_START: Duration = Duration::from_secs(1);
/// The longest a retry will ever wait.
///
/// Contention clears in seconds; an unwritable volume does not clear until
/// someone acts on it. A minute keeps the second case from being probed
/// pointlessly without making the first slow.
pub const RETRY_BACKOFF_MAX: Duration = Duration::from_secs(60);

/// What the unsaved indicator should say, or `None` when there is nothing to say.
///
/// Computed from [`App::unsaved`] rather than stored, so it cannot drift from the
/// set that actually decides what is at risk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnsavedIndicator {
    /// Files announced so far. Always at least 1 when this exists.
    pub count: usize,
    /// The single file's name, when there is exactly one.
    pub only: Option<String>,
    /// Seconds until the next retry, if one is scheduled and pending.
    pub retry_in: Option<u64>,
    /// True when nothing further will be attempted without being asked.
    pub waiting_for_user: bool,
    /// The error, when a single permanent failure makes it worth naming.
    pub reason: Option<String>,
}

impl UnsavedIndicator {
    /// The full text, for a row with room for it.
    pub fn full(&self) -> String {
        let what = match (&self.only, self.count) {
            (Some(name), _) => format!("unsaved: {name}"),
            (None, n) => format!("unsaved: {n} files"),
        };
        if self.waiting_for_user {
            if let Some(reason) = &self.reason {
                return format!("{what} - {reason}");
            }
            return format!("{what} - R to retry");
        }
        match self.retry_in {
            Some(0) | None => format!("{what} - retrying"),
            Some(s) => format!("{what} - retry in {s}s"),
        }
    }

    /// The fallback for a row too narrow for [`Self::full`].
    pub fn short(&self) -> String {
        match (&self.only, self.count) {
            (Some(name), _) => format!("unsaved: {name}"),
            (None, n) => format!("unsaved: {n} files"),
        }
    }
}

/// How long to wait before the `n`th attempt: 1s, 2s, 4s, … capped at a minute.
fn retry_delay(attempts: usize) -> Duration {
    let shift = attempts.saturating_sub(1).min(6) as u32;
    (RETRY_BACKOFF_START * 2u32.saturating_pow(shift)).min(RETRY_BACKOFF_MAX)
}

/// Whether an error will still be there on the next attempt.
///
/// A lock timeout is the transient case and the common one — another `fr` is
/// mid-write, and it will be finished shortly. The rest are conditions only a
/// person can clear, so retrying them on a timer accomplishes nothing; they wait
/// for an explicit retry instead.
fn is_permanent(error: &str) -> bool {
    let e = error.to_ascii_lowercase();
    e.contains("permission denied")
        || e.contains("read-only")
        || e.contains("no space")
        || e.contains("not found")
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
    /// This clone's actor token as read from `.actor` at startup (non-claiming):
    /// `Some("a")` tokened, `Some("null")` primary, `None` unclaimed. Display
    /// only — surfaced compactly on the Tracks overview header.
    pub actor_token: Option<String>,
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
    /// Files whose in-memory content did not reach disk. Empty is the normal state.
    ///
    /// Every save serializes the whole file from current in-memory state, so a
    /// failure loses nothing by itself and any later success writes everything
    /// accumulated since. What this set exists for is the one path that *can*
    /// lose it: an external reload replacing a track that never got written.
    pub unsaved: BTreeMap<SaveTarget, UnsavedFile>,
    /// `frame/` could not be written to at startup. Cleared by the first
    /// successful save, so a project that becomes writable mid-session recovers
    /// without a restart.
    pub frame_unwritable: bool,
    /// The last content known to be on disk for each file: what we loaded, or
    /// what we last successfully wrote.
    ///
    /// This is the common ancestor a three-way merge needs. Kept as text and
    /// parsed only when a merge actually runs, so the ordinary case costs one
    /// `String` per track and no parse.
    pub baselines: HashMap<SaveTarget, String>,
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
    /// Pending subtask hides (grace period before hiding done subtasks)
    pub pending_subtask_hides: Vec<PendingSubtaskHide>,
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
    /// Tab bar scroll offset: index of first visible track tab when in scroll mode
    pub tab_scroll: usize,
    /// Show startup hints in status bar until first real keypress
    pub show_startup_hints: bool,
    /// Effective note wrap setting (override > config > true)
    pub note_wrap: bool,
    /// Whether to show the recovery log overlay
    pub show_recovery_log: bool,
    /// Scroll offset for recovery log overlay
    pub recovery_log_scroll: usize,
    /// Cached recovery log lines for overlay display
    pub recovery_log_lines: Vec<String>,
    /// Total visual line count after wrapping (set by renderer)
    pub recovery_log_wrapped_count: usize,
    /// For each logical line, the visual line offset where it starts (set by renderer)
    pub recovery_log_line_offsets: Vec<usize>,

    /// Whether the results overlay is visible
    pub show_results_overlay: bool,
    /// Title for the results overlay
    pub results_overlay_title: String,
    /// Styled lines for the results overlay
    pub results_overlay_lines: Vec<Line<'static>>,
    /// Scroll offset for the results overlay
    pub results_overlay_scroll: usize,

    /// Project-wide search results (active when in View::Search or after jumping from it)
    pub project_search_results: Option<SearchResults>,
    /// History of project search queries (most recent first, max 200)
    pub project_search_history: Vec<String>,
    /// Current project search input text
    pub project_search_input: String,
    /// Position in project search history (None = new/draft)
    pub project_search_history_index: Option<usize>,
    /// Draft project search text (preserved while browsing history)
    pub project_search_draft: String,
    /// When true, Mode::Search is routed to project search handler instead of view search
    pub project_search_active: bool,
    /// Board view state
    pub board_state: BoardState,
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
        let note_wrap = project.config.ui.note_wrap;

        // Read-only: surface which clone we're on; never claims a token.
        let actor_token = crate::io::actors::read_actor_token(&project.frame_dir);

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

        let mut app = App {
            project,
            view: initial_view,
            mode: Mode::Navigate,
            should_quit: false,
            watcher_needs_restart: false,
            theme,
            actor_token,
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
            unsaved: BTreeMap::new(),
            frame_unwritable: false,
            baselines: HashMap::new(),
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
            pending_subtask_hides: Vec::new(),
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
            tab_scroll: 0,
            show_startup_hints: true,
            note_wrap,
            show_recovery_log: false,
            recovery_log_scroll: 0,
            recovery_log_lines: Vec::new(),
            recovery_log_wrapped_count: 0,
            recovery_log_line_offsets: Vec::new(),
            show_results_overlay: false,
            results_overlay_title: String::new(),
            results_overlay_lines: Vec::new(),
            results_overlay_scroll: 0,
            project_search_results: None,
            project_search_history: Vec::new(),
            project_search_input: String::new(),
            project_search_history_index: None,
            project_search_draft: String::new(),
            project_search_active: false,
            board_state: BoardState {
                focus_column: BoardColumn::Ready,
                cursor: [0; 3],
                scroll: [0; 3],
                mode: BoardMode::Cc,
                visible_columns: 3,
                column_pins: Vec::new(),
            },
        };

        // What was just loaded is, by definition, what is on disk — the common
        // ancestor for any merge that becomes necessary later in the session.
        for (id, track) in &app.project.tracks {
            app.baselines.insert(
                SaveTarget::Track(id.clone()),
                crate::parse::serialize_track(track),
            );
        }
        if let Some(inbox) = app.project.inbox.as_ref() {
            app.baselines
                .insert(SaveTarget::Inbox, crate::parse::serialize_inbox(inbox));
        }

        app
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

    /// Recursively collect a task and all its subtasks into a flat list.
    fn flatten_board_tasks(task: &Task) -> Vec<&Task> {
        let mut result = vec![task];
        for sub in &task.subtasks {
            result.extend(Self::flatten_board_tasks(sub));
        }
        result
    }

    /// Build the three board columns: [Ready, InProgress, Done]
    pub fn build_board_columns(&self) -> [Vec<BoardItem>; 3] {
        let cc_mode = self.board_state.mode == BoardMode::Cc;
        let tag_filter = self.filter_state.tag_filter.as_deref();
        let done_days = self.project.config.ui.board_done_days;

        let mut ready: Vec<BoardItem> = Vec::new();
        let mut in_progress: Vec<BoardItem> = Vec::new();
        let mut done_items: Vec<(String, BoardItem)> = Vec::new(); // (resolved_date, item)

        for track_id in &self.active_track_ids {
            let track = match Self::find_track_in_project(&self.project, track_id) {
                Some(t) => t,
                None => continue,
            };
            let track_name = self.track_name(track_id).to_string();

            let mut has_ready = false;
            let mut has_active = false;

            for top_task in track.backlog() {
                for task in Self::flatten_board_tasks(top_task) {
                    let task_id = match &task.id {
                        Some(id) => id.to_string(),
                        None => continue,
                    };

                    // task.id already carries the track prefix (e.g. "ST-001"),
                    // so render it directly without re-prefixing.
                    let id_display = task_id.clone();

                    // Apply tag filter
                    if let Some(tf) = tag_filter
                        && !task.tags.iter().any(|t| t == tf)
                    {
                        continue;
                    }

                    // Check if this task has a column pin (board grace period) or
                    // a pending section move. Either keeps the task in its original column.
                    let pin = self
                        .board_state
                        .column_pins
                        .iter()
                        .find(|p| p.track_id == *track_id && p.task_id == task_id);

                    let pending_move = self
                        .pending_moves
                        .iter()
                        .find(|pm| pm.track_id == *track_id && pm.task_id == task_id);

                    let effective_state = if let Some(p) = pin {
                        p.pinned_state
                    } else {
                        match pending_move {
                            Some(pm) if pm.settles_out_of_backlog() => {
                                pm.old_state.unwrap_or(task.state)
                            }
                            _ => task.state,
                        }
                    };

                    match effective_state {
                        TaskState::Todo => {
                            // Check all deps resolved (skip for pending-move tasks, they were already shown)
                            if pin.is_none()
                                && pending_move.is_none()
                                && !self.all_deps_resolved(task)
                            {
                                continue;
                            }
                            // CC mode filter
                            if cc_mode && !task.tags.iter().any(|t| t == "cc") {
                                continue;
                            }
                            if !has_ready {
                                ready.push(BoardItem::TrackHeader {
                                    track_name: track_name.clone(),
                                });
                                has_ready = true;
                            }
                            ready.push(BoardItem::Task {
                                track_id: track_id.clone(),
                                task_id: task_id.clone(),
                                title: task.title.clone(),
                                id_display,
                                state: task.state,
                                tags: task.tags.clone(),
                            });
                        }
                        TaskState::Active => {
                            if cc_mode && !task.tags.iter().any(|t| t == "cc") {
                                continue;
                            }
                            if !has_active {
                                in_progress.push(BoardItem::TrackHeader {
                                    track_name: track_name.clone(),
                                });
                                has_active = true;
                            }
                            in_progress.push(BoardItem::Task {
                                track_id: track_id.clone(),
                                task_id: task_id.clone(),
                                title: task.title.clone(),
                                id_display,
                                state: task.state,
                                tags: task.tags.clone(),
                            });
                        }
                        _ => {}
                    }
                }
            }

            // Collect done tasks from the Done section
            if done_days > 0 {
                for top_task in track.section_tasks(SectionKind::Done) {
                    for task in Self::flatten_board_tasks(top_task) {
                        let task_id = match &task.id {
                            Some(id) => id.to_string(),
                            None => continue,
                        };

                        // Check for a pending reopen (PendingMove::ToBacklog) — task was
                        // reopened but the section move hasn't fired yet (grace period).
                        let pending_reopen = self.pending_moves.iter().any(|pm| {
                            pm.leaves_done() && pm.track_id == *track_id && pm.task_id == task_id
                        });

                        if task.state != TaskState::Done && !pending_reopen {
                            continue;
                        }

                        // Apply tag filter
                        if let Some(tf) = tag_filter
                            && !task.tags.iter().any(|t| t == tf)
                        {
                            continue;
                        }

                        // CC mode: require #cc or #cc-added
                        if cc_mode && !task.tags.iter().any(|t| t == "cc" || t == "cc-added") {
                            continue;
                        }

                        // Check resolved date within done_days
                        let resolved_date = task.metadata.iter().find_map(|m| {
                            if let Metadata::Resolved(d) = m {
                                Some(d.clone())
                            } else {
                                None
                            }
                        });

                        let resolved_str = match &resolved_date {
                            Some(d) => d.clone(),
                            None => continue,
                        };

                        if !self.is_within_done_days(&resolved_str, done_days) {
                            continue;
                        }

                        // task.id already carries the track prefix; render directly.
                        let id_display = task_id.clone();

                        done_items.push((
                            resolved_str,
                            BoardItem::Task {
                                track_id: track_id.clone(),
                                task_id,
                                title: task.title.clone(),
                                id_display,
                                state: task.state,
                                tags: task.tags.clone(),
                            },
                        ));
                    }
                }
            }
        }

        // Sort done items by resolved date descending
        done_items.sort_by(|a, b| b.0.cmp(&a.0));
        let done: Vec<BoardItem> = done_items.into_iter().map(|(_, item)| item).collect();

        [ready, in_progress, done]
    }

    /// Check if all dependency targets of a task are done
    fn all_deps_resolved(&self, task: &Task) -> bool {
        for meta in &task.metadata {
            if let Metadata::Dep(deps) = meta {
                for dep_id in deps {
                    // Search all tracks for this dep
                    let mut found_done = false;
                    for (_, track) in &self.project.tracks {
                        if let Some(dep_task) =
                            crate::ops::task_ops::find_task_in_track(track, dep_id)
                        {
                            if dep_task.state == TaskState::Done {
                                found_done = true;
                            }
                            break;
                        }
                    }
                    if !found_done {
                        return false;
                    }
                }
            }
        }
        true
    }

    /// Check if a resolved date string is within the last N days
    fn is_within_done_days(&self, date_str: &str, days: u32) -> bool {
        let resolved = match chrono::NaiveDate::parse_from_str(date_str, "%Y-%m-%d") {
            Ok(d) => d,
            Err(_) => return false,
        };
        let today = chrono::Local::now().date_naive();
        let cutoff = today - chrono::Duration::days(i64::from(days));
        resolved >= cutoff
    }

    /// Get the (track_id, task_id) at the current board cursor position
    pub fn board_cursor_task_id(&self) -> Option<(String, String)> {
        let col_idx = self.board_state.focus_column.index();
        let columns = self.build_board_columns();
        let column = &columns[col_idx];
        let cursor = self.board_state.cursor[col_idx];
        match column.get(cursor) {
            Some(BoardItem::Task {
                track_id, task_id, ..
            }) => Some((track_id.clone(), task_id.clone())),
            _ => None,
        }
    }

    /// Count selectable tasks (excludes headers) in a board column
    pub fn board_task_count(&self, columns: &[Vec<BoardItem>], col: BoardColumn) -> usize {
        columns[col.index()]
            .iter()
            .filter(|item| matches!(item, BoardItem::Task { .. }))
            .count()
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

    /// Toggle note wrap on/off and persist the override to state
    pub fn toggle_note_wrap(&mut self) {
        self.note_wrap = !self.note_wrap;
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
        let source_index = move_task_between_sections(track, &pm.task_id, pm.from, pm.to)?;

        if pm.push_undo {
            self.undo_stack.push(Operation::SectionMove {
                track_id: pm.track_id.clone(),
                task_id: pm.task_id.clone(),
                from_section: pm.from,
                to_section: pm.to,
                from_index: source_index,
            });
        }

        // A task leaving Done is no longer resolved. The date is kept through the
        // grace period so the row holds its place in the Done column and the
        // Recent view while it is still visible there; this is where it goes.
        if pm.leaves_done() {
            let track = self.find_track_mut(&pm.track_id)?;
            let task = crate::ops::task_ops::find_task_mut_in_track(track, &pm.task_id)?;
            task.metadata.retain(|m| m.key() != "resolved");
            task.mark_dirty();
        }

        Some(pm.track_id.clone())
    }

    /// Cancel a pending subtask hide for a specific task.
    pub fn cancel_pending_subtask_hide(&mut self, track_id: &str, task_id: &str) {
        self.pending_subtask_hides
            .retain(|ph| ph.track_id != track_id || ph.task_id != task_id);
    }

    /// Flush expired subtask hides (remove entries past deadline — purely visual, no file save).
    pub fn flush_expired_subtask_hides(&mut self) {
        let now = Instant::now();
        self.pending_subtask_hides.retain(|ph| now < ph.deadline);
    }

    /// Reset all subtask hide deadlines (called on every keypress).
    pub fn reset_pending_subtask_hide_deadlines(&mut self) {
        let new_deadline = Instant::now() + std::time::Duration::from_secs(5);
        for ph in &mut self.pending_subtask_hides {
            ph.deadline = new_deadline;
        }
    }

    /// Reset the deadline on all pending moves (called on every keypress to keep
    /// tasks visible while the user is interacting).
    pub fn reset_pending_move_deadlines(&mut self) {
        let new_deadline = Instant::now() + std::time::Duration::from_secs(5);
        for pm in &mut self.pending_moves {
            pm.deadline = new_deadline;
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
        // Collect expiring column pins so we can flash tasks that move columns
        let expiring_pins: Vec<String> = self
            .board_state
            .column_pins
            .iter()
            .filter(|p| now >= p.deadline)
            .map(|p| p.task_id.clone())
            .collect();
        self.board_state.column_pins.retain(|p| now < p.deadline);
        if !expiring_pins.is_empty() {
            let ids: std::collections::HashSet<String> = expiring_pins.into_iter().collect();
            self.flash_tasks(ids);
        }

        // Flash tasks that are about to move columns via pending moves
        let moving_task_ids: std::collections::HashSet<String> = expired
            .iter()
            .filter(|pm| pm.leaves_done())
            .map(|pm| pm.task_id.clone())
            .collect();

        let mut modified = Vec::new();
        for pm in &expired {
            if let Some(tid) = self.execute_pending_move(pm)
                && !modified.contains(&tid)
            {
                modified.push(tid);
            }
        }

        if !moving_task_ids.is_empty() {
            self.flash_tasks(moving_task_ids);
        }

        modified
    }

    /// Flush all pending moves immediately (used on view change, quit). Returns modified track IDs.
    pub fn flush_all_pending_moves(&mut self) -> Vec<String> {
        let all: Vec<PendingMove> = std::mem::take(&mut self.pending_moves);
        self.board_state.column_pins.clear();
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
                ids.push(id.to_string());
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
                                .push(task_id.to_string());
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

    /// Resolve this working copy's minting namespace for an interactive mint,
    /// auto-claiming a token on first use. On a fresh auto-claim, sets a one-time
    /// status message. On failure (no token claimable), sets an error status and
    /// returns `Err(())` so the caller can abort the mint without creating
    /// anything.
    pub(crate) fn resolve_mint_namespace(
        &mut self,
    ) -> Result<Option<crate::model::task_id::Token>, ()> {
        match crate::io::actors::resolve_actor_token(&self.project.frame_dir) {
            Ok(resolved) => {
                if let Some(msg) = resolved.announcement {
                    self.status_message = Some(msg);
                }
                Ok(crate::model::task_id::actor_namespace(&resolved.token))
            }
            Err(e) => {
                self.status_message = Some(e);
                Err(())
            }
        }
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

    // ---- Saving -------------------------------------------------------
    //
    // The `_logged` forms are the only way in. The fallible ones are private
    // because a public `save_track` is an invitation to `let _ = ...`, which is
    // how 61 sites came to discard their errors: a failed save left the TUI
    // showing state that was not on disk, with nothing recorded anywhere.
    //
    // A failure is recorded, not announced. Mid-flow in a TUI a transient error
    // toast is noise the user cannot act on; the recovery log is durable and is
    // surfaced afterwards by `fr recovery` and `fr check`, which is where it can
    // be acted on. (A *sustained* failure — an unwritable frame/, or a lock held
    // past the timeout — deserves a persistent indicator rather than a toast;
    // that is tracked separately.)
    //
    // The lock is acquired by the entry points, never by the inner writes, so a
    // multi-file operation can hold one lock across all of them. `FileLock` is
    // not re-entrant, so an inner write that re-acquired would deadlock.

    /// Write the inbox. **Assumes the project lock is held.**
    fn save_inbox_locked(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let inbox = self.project.inbox.as_ref().ok_or("no inbox loaded")?;
        project_io::save_inbox(&self.project.frame_dir, inbox)?;
        self.last_save_at = Some(Instant::now());
        self.record_baseline(SaveTarget::Inbox);
        Ok(())
    }

    /// Write one track. **Assumes the project lock is held.**
    fn save_track_locked(&mut self, track_id: &str) -> Result<(), Box<dyn std::error::Error>> {
        let file = self
            .track_file(track_id)
            .ok_or("track not found")?
            .to_string();
        let track =
            Self::find_track_in_project(&self.project, track_id).ok_or("track not found")?;
        project_io::save_track(&self.project.frame_dir, &file, track)?;
        self.last_save_at = Some(Instant::now());
        // Record the new mtime so we know this is our write
        let path = self.project.frame_dir.join(&file);
        if let Ok(mtime) = std::fs::metadata(&path).and_then(|m| m.modified()) {
            self.track_mtimes.insert(track_id.to_string(), mtime);
        }
        self.record_baseline(SaveTarget::Track(track_id.to_string()));
        Ok(())
    }

    /// Note what a file's content is now that memory and disk agree.
    ///
    /// Serializing the in-memory model rather than re-reading the file is
    /// deliberate and equivalent: this runs immediately after that exact model
    /// was serialized and written, and serialization is deterministic.
    fn record_baseline(&mut self, target: SaveTarget) {
        // A write just succeeded, so whatever the startup probe found is over.
        self.frame_unwritable = false;
        let text = match &target {
            SaveTarget::Track(id) => {
                Self::find_track_in_project(&self.project, id).map(crate::parse::serialize_track)
            }
            SaveTarget::Inbox => self
                .project
                .inbox
                .as_ref()
                .map(crate::parse::serialize_inbox),
        };
        if let Some(text) = text {
            self.baselines.insert(target, text);
        }
    }

    /// Record a failed save: remember the file as unsaved, and announce it once
    /// it has proved it is not going to fix itself.
    ///
    /// The set is what protects the content — an entry here stops
    /// [`Self::reload_changed_files`] from overwriting a file whose only copy is
    /// in memory — and it is maintained on the *first* failure, because
    /// correctness cannot wait on a threshold.
    ///
    /// Announcing is gated separately ([`UnsavedFile::worth_announcing`]), and
    /// happens exactly once per incident: one recovery entry when it starts, one
    /// when it clears. A blip that the next retry resolves writes nothing at
    /// all, and a file failing for an hour is still one entry rather than a
    /// hundred crowding out everything else in the log.
    fn record_save_failure(&mut self, target: SaveTarget, e: &dyn std::fmt::Display) {
        let error = e.to_string();
        let entry = self
            .unsaved
            .entry(target.clone())
            .or_insert_with(|| UnsavedFile {
                error: error.clone(),
                attempts: 0,
                next_retry_at: Instant::now(),
                permanent: false,
                surfaced: false,
            });
        entry.error = error.clone();
        entry.attempts += 1;
        entry.permanent = is_permanent(&error);
        entry.next_retry_at = Instant::now() + retry_delay(entry.attempts);

        let announce = entry.worth_announcing() && !entry.surfaced;
        if !announce {
            return;
        }
        entry.surfaced = true;

        crate::io::recovery::log_recovery(
            &self.project.frame_dir,
            crate::io::recovery::RecoveryEntry {
                timestamp: chrono::Utc::now(),
                category: crate::io::recovery::RecoveryCategory::Write,
                description: format!("{} save failed", target.label()),
                fields: vec![("Error".to_string(), error.clone())],
                body: String::new(),
            },
        );

        // The indicator carries the fact; this carries the reason, once, at the
        // moment it becomes true.
        self.status_message = Some(format!("Cannot save {}: {error}", target.label()));
        self.status_is_error = true;
    }

    /// Write every file that never reached disk into `frame/.rescue/`.
    ///
    /// Returns the paths written. Called at exit, when the in-memory copy is
    /// about to stop existing and is the only one there is.
    ///
    /// **Best-effort by design.** The save failed because something was wrong
    /// with writing to this project, so the dump may well fail for the same
    /// reason; when it does the file is skipped and the exit report says so.
    /// There is no fallback location — a rescue copy somewhere the user will
    /// never look is not a rescue, and a temp directory that the OS may clear is
    /// worse than an honest report of what was lost.
    pub fn dump_unsaved(&self) -> Vec<PathBuf> {
        if self.unsaved.is_empty() {
            return Vec::new();
        }
        let dir = self.project.frame_dir.join(RESCUE_DIR);
        if std::fs::create_dir_all(&dir).is_err() {
            return Vec::new();
        }

        let mut written = Vec::new();
        for target in self.unsaved.keys() {
            let (name, text) = match target {
                SaveTarget::Track(id) => {
                    let Some(track) = Self::find_track_in_project(&self.project, id) else {
                        continue;
                    };
                    (
                        self.display_name(target),
                        crate::parse::serialize_track(track),
                    )
                }
                SaveTarget::Inbox => {
                    let Some(inbox) = self.project.inbox.as_ref() else {
                        continue;
                    };
                    ("inbox.md".to_string(), crate::parse::serialize_inbox(inbox))
                }
            };
            let path = dir.join(name);
            if std::fs::write(&path, text).is_ok() {
                written.push(path);
            }
        }
        written
    }

    /// What the unsaved indicator should show, or `None` for nothing.
    ///
    /// Reads only *announced* failures, so a transient one that the next retry
    /// clears never reaches the screen — see [`UnsavedFile::worth_announcing`].
    pub fn unsaved_indicator(&self) -> Option<UnsavedIndicator> {
        let announced: Vec<(&SaveTarget, &UnsavedFile)> =
            self.unsaved.iter().filter(|(_, f)| f.surfaced).collect();
        if announced.is_empty() {
            return None;
        }

        let count = announced.len();
        let only = (count == 1).then(|| self.display_name(announced[0].0));

        // Nothing is scheduled when every announced failure is one only the user
        // can clear.
        let waiting_for_user = announced.iter().all(|(_, f)| f.permanent);
        let now = Instant::now();
        let retry_in = announced
            .iter()
            .filter(|(_, f)| !f.permanent)
            .map(|(_, f)| f.next_retry_at.saturating_duration_since(now).as_secs())
            .min();
        let reason = (count == 1 && waiting_for_user).then(|| announced[0].1.error.clone());

        Some(UnsavedIndicator {
            count,
            only,
            retry_in,
            waiting_for_user,
            reason,
        })
    }

    /// The file name to show for a save target — `main.md`, not `track main`.
    fn display_name(&self, target: &SaveTarget) -> String {
        match target {
            SaveTarget::Inbox => "inbox.md".to_string(),
            SaveTarget::Track(id) => self
                .track_file(id)
                .and_then(|f| f.rsplit('/').next().map(str::to_string))
                .unwrap_or_else(|| id.clone()),
        }
    }

    /// Note that a file reached disk, so it is no longer outstanding.
    fn clear_save_failure(&mut self, target: &SaveTarget) {
        // Only an announced failure gets a resolution entry, so the log reads as
        // matched pairs rather than orphan "recovered" lines for blips nobody
        // was ever told about.
        if let Some(f) = self.unsaved.remove(target)
            && f.surfaced
        {
            crate::io::recovery::log_recovery(
                &self.project.frame_dir,
                crate::io::recovery::RecoveryEntry {
                    timestamp: chrono::Utc::now(),
                    category: crate::io::recovery::RecoveryCategory::Write,
                    description: format!("{} saved after {} attempts", target.label(), f.attempts),
                    fields: vec![("Last error".to_string(), f.error)],
                    body: String::new(),
                },
            );
        }
    }

    /// Re-attempt every outstanding save whose backoff has elapsed.
    ///
    /// Retrying costs nothing to prepare: a save serializes the whole file from
    /// current in-memory state, so there is no queue to replay and no ordering
    /// to reconstruct. One success writes the failed edit along with everything
    /// that accumulated after it.
    ///
    /// **The lock timeout here is zero, deliberately.** `acquire_default` blocks
    /// for five seconds, and a retry on the 250ms event tick using that would
    /// freeze the TUI for five seconds at a time during exactly the contention
    /// it is recovering from. `lock_file` makes one attempt before testing the
    /// elapsed time, so a zero timeout is a clean try-lock. The original saves
    /// keep the patient timeout; only this is impatient.
    ///
    /// One acquisition covers every outstanding file, so a retry cannot be
    /// interleaved by another writer partway through.
    pub fn retry_unsaved_saves(&mut self, force: bool) {
        if self.unsaved.is_empty() {
            return;
        }

        let now = Instant::now();
        let due: Vec<SaveTarget> = self
            .unsaved
            .iter()
            .filter(|(_, f)| force || (!f.permanent && f.next_retry_at <= now))
            .map(|(t, _)| t.clone())
            .collect();
        if due.is_empty() {
            return;
        }

        let Ok(lock) = FileLock::acquire(&self.project.frame_dir, Duration::from_millis(0)) else {
            // Still contended. Push each due file out by its own backoff rather
            // than hammering the lock every tick.
            for target in &due {
                if let Some(f) = self.unsaved.get_mut(target) {
                    f.attempts += 1;
                    f.next_retry_at = now + retry_delay(f.attempts);
                }
            }
            return;
        };

        for target in due {
            let result = match &target {
                SaveTarget::Track(id) => {
                    let id = id.clone();
                    self.save_track_locked(&id)
                }
                SaveTarget::Inbox => self.save_inbox_locked(),
            };
            match result {
                Ok(()) => self.clear_save_failure(&target),
                Err(e) => self.record_save_failure(target, &e),
            }
        }

        drop(lock);
    }

    /// Retry now, resetting every backoff — the `R` key and the palette action.
    ///
    /// Also covers the permanent failures the timer skips: the user is the one
    /// who clears those conditions, so their asking is the signal that it is
    /// worth trying again.
    pub fn force_retry_unsaved(&mut self) {
        let outstanding = self.unsaved.len();
        if outstanding == 0 {
            self.status_message = Some("Nothing waiting to be saved".into());
            self.status_is_error = false;
            return;
        }
        for f in self.unsaved.values_mut() {
            f.attempts = 0;
            f.next_retry_at = Instant::now();
        }
        self.retry_unsaved_saves(true);

        if self.unsaved.is_empty() {
            self.status_message = Some(format!(
                "Saved {outstanding} outstanding file{}",
                if outstanding == 1 { "" } else { "s" }
            ));
            self.status_is_error = false;
        } else {
            let first = self
                .unsaved
                .values()
                .next()
                .map(|f| f.error.clone())
                .unwrap_or_default();
            self.status_message = Some(format!("Still cannot save: {first}"));
            self.status_is_error = true;
        }
    }

    /// Save one track, recording any failure.
    pub fn save_track_logged(&mut self, track_id: &str) {
        let target = SaveTarget::Track(track_id.to_string());
        match FileLock::acquire_default(&self.project.frame_dir) {
            Ok(_lock) => match self.save_track_locked(track_id) {
                Ok(()) => self.clear_save_failure(&target),
                Err(e) => self.record_save_failure(target, &e),
            },
            Err(e) => self.record_save_failure(target, &e),
        }
    }

    /// Save the inbox, recording any failure.
    pub fn save_inbox_logged(&mut self) {
        match FileLock::acquire_default(&self.project.frame_dir) {
            Ok(_lock) => match self.save_inbox_locked() {
                Ok(()) => self.clear_save_failure(&SaveTarget::Inbox),
                Err(e) => self.record_save_failure(SaveTarget::Inbox, &e),
            },
            Err(e) => self.record_save_failure(SaveTarget::Inbox, &e),
        }
    }

    /// Save several tracks, and optionally the inbox, under a **single** lock.
    ///
    /// Use this for any operation that is only complete once more than one file
    /// is written — a cross-track move, a triage. Saving them one at a time takes
    /// and releases the lock between each, leaving a window another process can
    /// write into: the ordering is then correct but not atomic.
    ///
    /// Each failure is recorded individually; one failing does not stop the rest,
    /// because a partial write is still better than abandoning the remainder.
    pub fn save_batch_logged(&mut self, track_ids: &[&str], inbox: bool) {
        let lock = match FileLock::acquire_default(&self.project.frame_dir) {
            Ok(lock) => lock,
            Err(e) => {
                for id in track_ids {
                    self.record_save_failure(SaveTarget::Track(id.to_string()), &e);
                }
                if inbox {
                    self.record_save_failure(SaveTarget::Inbox, &e);
                }
                return;
            }
        };

        for id in track_ids {
            let target = SaveTarget::Track(id.to_string());
            match self.save_track_locked(id) {
                Ok(()) => self.clear_save_failure(&target),
                Err(e) => self.record_save_failure(target, &e),
            }
        }
        if inbox {
            match self.save_inbox_locked() {
                Ok(()) => self.clear_save_failure(&SaveTarget::Inbox),
                Err(e) => self.record_save_failure(SaveTarget::Inbox, &e),
            }
        }

        drop(lock);
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
            let task_id = task.id.as_ref()?.to_string();
            Some((track_id, task_id, *section))
        } else {
            None
        }
    }

    /// Merge an external change into an unsaved file instead of replacing it.
    ///
    /// Reached when a file changed externally while our copy had not reached
    /// disk. Both sides then hold content that exists nowhere else, so the
    /// unconditional replace this method exists to prevent silently destroyed
    /// work the user could see on screen. The lock timeout makes the collision
    /// likely rather than exotic: a save that failed because another `fr` held
    /// the lock is followed by exactly that process writing the file.
    ///
    /// Neither side is a safe default. With several sessions on one project, an
    /// agent's write is as real as a human's and the agent cannot tell it was
    /// dropped. So the two are merged ([`crate::ops::reconcile`]) — a track on
    /// task identity, where only a task both sides changed differently falls
    /// back to keeping ours and its other version goes to the recovery log; the
    /// inbox by content, where nothing is ever set aside.
    ///
    /// Without a baseline there is no ancestor to merge against, so the fallback
    /// covers the whole file — the same floor as before the merge existed.
    ///
    /// The mtime is deliberately *not* updated. `track_changed_on_disk` reads it
    /// to decide whether memory and disk have diverged, and after this they have.
    fn preserve_unreplaced(&mut self, target: &SaveTarget, path: &std::path::Path) {
        let Ok(text) = std::fs::read_to_string(path) else {
            return;
        };

        if let SaveTarget::Track(track_id) = target
            && let Some(base_text) = self.baselines.get(target).cloned()
            && let Some(ours) = Self::find_track_in_project(&self.project, track_id)
        {
            let base = parse_track(&base_text);
            let theirs = parse_track(&text);
            let result = crate::ops::reconcile::reconcile_track(&base, ours, &theirs);

            for conflict in &result.conflicts {
                crate::io::recovery::log_recovery(
                    &self.project.frame_dir,
                    crate::io::recovery::RecoveryEntry {
                        timestamp: chrono::Utc::now(),
                        category: crate::io::recovery::RecoveryCategory::Write,
                        description: format!(
                            "concurrent edit to {} in {} — kept the in-memory version",
                            conflict.key,
                            target.label()
                        ),
                        fields: vec![("Reason".to_string(), format!("{:?}", conflict.reason))],
                        body: conflict.theirs.join("\n"),
                    },
                );
            }

            let took = result.took_theirs;
            let deleted = result.deleted;
            self.replace_track(track_id, result.track);
            if took > 0 || deleted > 0 {
                self.announce_merge(target, took + deleted);
            }
            return;
        }

        if matches!(target, SaveTarget::Inbox)
            && let Some(base_text) = self.baselines.get(target).cloned()
            && let Some(ours) = self.project.inbox.as_ref()
        {
            let (base, _) = parse_inbox(&base_text);
            let (theirs, _) = parse_inbox(&text);
            let result = crate::ops::reconcile::reconcile_inbox(&base, ours, &theirs);

            // Nothing to log: the inbox merge never sets a side's content aside.
            let changed = result.took_theirs + result.deleted;
            self.project.inbox = Some(result.inbox);
            if changed > 0 {
                self.announce_merge(target, changed);
            }
            return;
        }

        // No ancestor to merge against: keep ours whole and preserve theirs.
        crate::io::recovery::log_recovery(
            &self.project.frame_dir,
            crate::io::recovery::RecoveryEntry {
                timestamp: chrono::Utc::now(),
                category: crate::io::recovery::RecoveryCategory::Write,
                description: format!(
                    "external change to unsaved {} — kept the in-memory version",
                    target.label()
                ),
                fields: vec![("Path".to_string(), path.display().to_string())],
                body: text,
            },
        );
    }

    /// Tell the user their unsaved file absorbed someone else's changes.
    fn announce_merge(&mut self, target: &SaveTarget, changed: usize) {
        self.status_message = Some(format!(
            "Merged {} external change{} into unsaved {}",
            changed,
            if changed == 1 { "" } else { "s" },
            target.label()
        ));
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

            // Compute relative path from frame_dir to distinguish files in
            // subdirectories (e.g., archive/main.md vs tracks/main.md).
            let rel_path = path
                .strip_prefix(&self.project.frame_dir)
                .ok()
                .and_then(|p| p.to_str())
                .map(|s| s.to_string());

            if is_inbox_path(&file_name, rel_path.as_deref()) {
                if self.unsaved.contains_key(&SaveTarget::Inbox) {
                    self.preserve_unreplaced(&SaveTarget::Inbox, path);
                    continue;
                }
                if let Ok(text) = std::fs::read_to_string(path) {
                    let (inbox, dropped) = parse_inbox(&text);
                    if !dropped.is_empty() {
                        crate::io::recovery::log_recovery(
                            &self.project.frame_dir,
                            crate::io::recovery::RecoveryEntry {
                                timestamp: chrono::Utc::now(),
                                category: crate::io::recovery::RecoveryCategory::Parser,
                                description: "dropped lines".to_string(),
                                fields: vec![("Source".to_string(), "inbox.md".to_string())],
                                body: dropped.join("\n"),
                            },
                        );
                    }
                    self.project.inbox = Some(inbox);
                }
                continue;
            }

            if file_name == "project.toml" {
                // Config changes — skip for now (would need full re-init)
                continue;
            }
            if let Some((track_id, _track_file)) =
                resolve_track_for_path(&self.project.config.tracks, &file_name, rel_path.as_deref())
                && let Ok(text) = std::fs::read_to_string(path)
            {
                let target = SaveTarget::Track(track_id.clone());
                if self.unsaved.contains_key(&target) {
                    self.preserve_unreplaced(&target, path);
                    continue;
                }

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

        // Auto-assign IDs and dates to any newly-loaded tasks. Passive load must
        // not auto-claim a token; an unclaimed clone mints nothing (strict null
        // policy) until an explicit action resolves a token.
        let scope = crate::io::actors::id_scope(&self.project.frame_dir);
        let modified_tracks = crate::ops::clean::ensure_ids_and_dates(&mut self.project, scope);
        for track_id in &modified_tracks {
            self.save_track_logged(track_id);
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

    /// Check if a detail region has non-empty content for the given task
    pub fn is_detail_region_populated(task: &Task, region: DetailRegion) -> bool {
        use crate::model::Metadata;
        match region {
            DetailRegion::Title => true,
            DetailRegion::Tags => !task.tags.is_empty(),
            DetailRegion::Added => true, // only in regions list if present
            DetailRegion::Subtasks => true, // only in regions list if present
            DetailRegion::Deps => task
                .metadata
                .iter()
                .any(|m| matches!(m, Metadata::Dep(v) if !v.is_empty())),
            DetailRegion::Spec => task.metadata.iter().any(|m| matches!(m, Metadata::Spec(_))),
            DetailRegion::Refs => task
                .metadata
                .iter()
                .any(|m| matches!(m, Metadata::Ref(v) if !v.is_empty())),
            DetailRegion::Note => task
                .metadata
                .iter()
                .any(|m| matches!(m, Metadata::Note(s) if !s.is_empty())),
        }
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
                View::Board => ReturnView::Board,
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
            sticky_col: None,
            total_lines: 0,
            note_view_line: None,
            note_header_line: None,
            note_content_end: 0,
            regions_populated: Vec::new(),
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

        // Build set of subtask IDs still in grace period (visible despite being done)
        let now = Instant::now();
        let grace_ids: HashSet<String> = self
            .pending_subtask_hides
            .iter()
            .filter(|ph| ph.track_id == track_id && now < ph.deadline)
            .map(|ph| ph.task_id.clone())
            .collect();

        let mut items = Vec::new();

        // Backlog tasks
        let backlog = track.backlog();
        flatten_tasks(
            backlog,
            SectionKind::Backlog,
            0,
            &mut items,
            expanded,
            &[],
            &grace_ids,
        );

        // Parked section (if non-empty)
        let parked = track.parked();
        if !parked.is_empty() {
            items.push(FlatItem::ParkedSeparator);
            flatten_tasks(
                parked,
                SectionKind::Parked,
                0,
                &mut items,
                expanded,
                &[],
                &grace_ids,
            );
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
            ids.push(id.to_string());
        }
        flatten_subtask_ids_inner(&task.subtasks, ids);
    }
}

/// Generate a unique key for a task's expand/collapse state
pub fn task_expand_key(task: &Task, section: SectionKind, path: &[usize]) -> String {
    if let Some(id) = &task.id {
        id.to_string()
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
    grace_ids: &HashSet<String>,
) {
    flatten_tasks_inner(
        tasks,
        section,
        depth,
        items,
        expanded,
        ancestor_last,
        &[],
        grace_ids,
    );
}

#[allow(clippy::too_many_arguments)]
fn flatten_tasks_inner(
    tasks: &[Task],
    section: SectionKind,
    depth: usize,
    items: &mut Vec<FlatItem>,
    expanded: Option<&HashSet<String>>,
    ancestor_last: &[bool],
    parent_path: &[usize],
    grace_ids: &HashSet<String>,
) {
    let count = tasks.len();

    // For subtasks (depth > 0), determine which are visible vs hidden
    if depth > 0 {
        let total_count = count;
        let mut visible_indices: Vec<usize> = Vec::new();
        let mut done_count = 0usize;

        for (i, task) in tasks.iter().enumerate() {
            let is_done = task.state == TaskState::Done;
            if is_done {
                done_count += 1;
                // Visible during grace period
                let in_grace = task.id.as_ref().is_some_and(|id| grace_ids.contains(&**id));
                if in_grace {
                    visible_indices.push(i);
                }
            } else {
                visible_indices.push(i);
            }
        }

        let hidden_count = done_count.saturating_sub(
            // done tasks that are in grace (still visible)
            tasks
                .iter()
                .filter(|t| {
                    t.state == TaskState::Done
                        && t.id.as_ref().is_some_and(|id| grace_ids.contains(&**id))
                })
                .count(),
        );

        // Insert DoneSummary if any subtasks are actually hidden
        if hidden_count > 0 {
            items.push(FlatItem::DoneSummary {
                depth,
                done_count,
                total_count,
                ancestor_last: ancestor_last.to_vec(),
            });
        }

        // Flatten only visible subtasks
        let visible_count = visible_indices.len();
        for (vi, &real_idx) in visible_indices.iter().enumerate() {
            let task = &tasks[real_idx];
            // is_last_sibling: last visible subtask, and no DoneSummary comes after
            // (DoneSummary is before visible subtasks, so last visible is truly last)
            let is_last = vi == visible_count - 1;
            let has_children = !task.subtasks.is_empty();

            let mut path = parent_path.to_vec();
            path.push(real_idx); // use real index to preserve resolve_task_from_flat correctness

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
                    grace_ids,
                );
            }
        }
    } else {
        // Top-level tasks: no done-subtask hiding at this level
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
                    grace_ids,
                );
            }
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

    // Keep DoneSummary if its parent task is kept
    for i in 0..items.len() {
        if let FlatItem::DoneSummary { depth, .. } = &items[i] {
            let summary_depth = *depth;
            // Walk backwards to find the nearest Task at depth-1 (the parent)
            for j in (0..i).rev() {
                if let FlatItem::Task { depth: d, .. } = &items[j]
                    && *d == summary_depth.saturating_sub(1)
                {
                    keep[i] = keep[j];
                    break;
                }
            }
        }
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
        "board" => app.view = View::Board,
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

    // Restore board state
    if let Some(mode_str) = &ui_state.board_mode {
        app.board_state.mode = match mode_str.as_str() {
            "all" => BoardMode::All,
            _ => BoardMode::Cc,
        };
    }
    if let Some(col) = ui_state.board_focus_column {
        app.board_state.focus_column = BoardColumn::from_index(col);
    }

    // Restore per-track state
    for (track_id, track_ui) in &ui_state.tracks {
        let state = app.get_track_state(track_id);
        state.cursor = track_ui.cursor;
        state.scroll_offset = track_ui.scroll_offset;
        state.expanded = track_ui.expanded.clone();
    }

    // Search history is restored, but the active search itself is not: a
    // pattern from a previous session is not the search you are running now.
    // Restore search history
    app.search_history = ui_state.search_history;

    // Restore project search history
    app.project_search_history = ui_state.project_search_history;

    // Restore note wrap override
    if let Some(wrap_override) = ui_state.note_wrap_override {
        app.note_wrap = wrap_override;
    }
}

/// Save UI state to .state.json
pub fn save_ui_state(app: &App) {
    use crate::io::state::{TrackUiState, UiState, write_ui_state};

    let view_to_save = if app.view == View::Search {
        // On quit from Search view, save the return_view instead
        app.project_search_results
            .as_ref()
            .map(|sr| sr.return_view.clone())
            .unwrap_or(View::Recent)
    } else {
        app.view.clone()
    };
    let (view_str, active_track) = match &view_to_save {
        View::Track(idx) => (
            "track".to_string(),
            app.active_track_ids.get(*idx).cloned().unwrap_or_default(),
        ),
        View::Detail { track_id, .. } => ("track".to_string(), track_id.clone()),
        View::Tracks => ("tracks".to_string(), String::new()),
        View::Board => ("board".to_string(), String::new()),
        View::Inbox => ("inbox".to_string(), String::new()),
        View::Recent => ("recent".to_string(), String::new()),
        View::Search => ("recent".to_string(), String::new()),
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

    let note_wrap_override = if app.note_wrap != app.project.config.ui.note_wrap {
        Some(app.note_wrap)
    } else {
        None
    };

    let board_mode = Some(match app.board_state.mode {
        BoardMode::Cc => "cc".to_string(),
        BoardMode::All => "all".to_string(),
    });

    let ui_state = UiState {
        view: view_str,
        active_track,
        tracks,
        search_history: app.search_history.clone(),
        note_wrap_override,
        project_search_history: app.project_search_history.clone(),
        board_mode,
        board_focus_column: Some(app.board_state.focus_column.index()),
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

    // Auto-assign IDs and dates so all tasks are interactive from the start.
    // Startup must not auto-claim a token; an unclaimed clone mints nothing
    // (strict null policy) until an explicit action resolves a token.
    let scope = crate::io::actors::id_scope(&project.frame_dir);
    let modified_tracks = crate::ops::clean::ensure_ids_and_dates(&mut project, scope);
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
                    // No `App` exists yet, so there is nothing to hold a status
                    // on — but the failure still has to be recorded rather than
                    // dropped.
                    if let Err(e) = project_io::save_track(&project.frame_dir, file, track) {
                        crate::io::recovery::log_recovery(
                            &project.frame_dir,
                            crate::io::recovery::RecoveryEntry {
                                timestamp: chrono::Utc::now(),
                                category: crate::io::recovery::RecoveryCategory::Write,
                                description: format!(
                                    "startup id/date assignment: track {track_id} save failed"
                                ),
                                fields: vec![("Error".to_string(), e.to_string())],
                                body: String::new(),
                            },
                        );
                    }
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

    // Find out whether writes will work at all before the user types anything,
    // rather than at the first save — or, as it used to be, at quit.
    //
    // Here rather than in `App::new` because a constructor that writes to disk
    // is a surprise to every caller, and every test fixture builds an `App`
    // against a directory that does not exist.
    if let Some(reason) = probe_unwritable(&app.project.frame_dir) {
        app.status_message = Some(format!("Cannot write to frame/: {reason}"));
        app.status_is_error = true;
        app.frame_unwritable = true;
    }

    // Set terminal window title
    set_window_title(&app.project.config.project.name);

    // Run event loop
    let result = run_event_loop(&mut terminal, &mut app, watcher);

    // Save UI state before exit
    save_ui_state(&app);

    // Copy anything that never reached disk while the in-memory copy still
    // exists. Must happen before the terminal is restored so the report below
    // can say where it went.
    let rescued = app.dump_unsaved();
    let unsaved_report = unsaved_exit_report(&app, &rescued);

    // Restore terminal
    clear_window_title();
    disable_raw_mode()?;
    let _ = execute!(terminal.backend_mut(), DisableBracketedPaste);
    let _ = execute!(terminal.backend_mut(), PopKeyboardEnhancementFlags);
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    result?;

    // Only now, with the alternate screen gone, is there anywhere for this to be
    // read. Exiting silently is the failure item 9 was written about: the user
    // quits, sees a clean exit, and the work is gone.
    if let Some(report) = unsaved_report {
        eprint!("{report}");
        return Err(format!(
            "quit with {} unsaved file{}",
            app.unsaved.len(),
            if app.unsaved.len() == 1 { "" } else { "s" }
        )
        .into());
    }

    Ok(())
}

/// The exit report for work that never reached disk, or `None` when all is well.
///
/// Names each file and why it failed, then where the rescue copy went — or says
/// plainly that there is none, which is the case worth being loudest about.
fn unsaved_exit_report(app: &App, rescued: &[PathBuf]) -> Option<String> {
    if app.unsaved.is_empty() {
        return None;
    }

    let mut out = String::new();
    out.push_str(&format!(
        "\n{} file{} could not be saved:\n",
        app.unsaved.len(),
        if app.unsaved.len() == 1 { "" } else { "s" }
    ));
    for (target, f) in &app.unsaved {
        out.push_str(&format!("  {} — {}\n", app.display_name(target), f.error));
    }

    if rescued.is_empty() {
        out.push_str(
            "\nNo rescue copy could be written either — the same problem that \
             stopped the save.\nThe contents are gone; nothing further can be \
             recovered from this session.\n",
        );
    } else {
        out.push_str(&format!(
            "\nCopies of the unsaved work were written to:\n  {}\n",
            app.project.frame_dir.join(RESCUE_DIR).display()
        ));
        out.push_str("Move them into place once the cause is fixed.\n");
    }
    out.push_str(&format!(
        "Details: {}\n",
        crate::io::recovery::recovery_log_path(&app.project.frame_dir).display()
    ));
    Some(out)
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

        // Flush expired pending moves and column pins (only in Navigate mode)
        if app.mode == Mode::Navigate
            && (!app.pending_moves.is_empty() || !app.board_state.column_pins.is_empty())
        {
            let modified = app.flush_expired_pending_moves();
            for tid in &modified {
                app.save_track_logged(tid);
            }
        }

        // Flush expired subtask hides (purely visual, no file save needed)
        if !app.pending_subtask_hides.is_empty() {
            app.flush_expired_subtask_hides();
        }

        // Re-attempt anything that did not reach disk. Cheap and usually
        // successful: the common failure is another `fr` holding the lock, which
        // it releases as soon as its own write is done.
        app.retry_unsaved_saves(false);

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
                // Reset grace period on any keypress so tasks don't move
                // out from under the user while they're interacting
                if !app.pending_moves.is_empty() {
                    app.reset_pending_move_deadlines();
                }
                if !app.pending_subtask_hides.is_empty() {
                    app.reset_pending_subtask_hide_deadlines();
                }

                // Flush all pending moves on view change
                if app.view != old_view && !app.pending_moves.is_empty() {
                    let modified = app.flush_all_pending_moves();
                    for tid in &modified {
                        app.save_track_logged(tid);
                    }
                }

                // Clear subtask hide grace periods on view/tab change
                if app.view != old_view {
                    app.pending_subtask_hides.clear();
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
                app.save_track_logged(tid);
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
    // Clear subtask hide grace entries for affected tracks
    let affected_track_ids: HashSet<String> = paths
        .iter()
        .filter_map(|p| {
            let file_name = p.file_name()?.to_str()?;
            let rel = p
                .strip_prefix(&app.project.frame_dir)
                .ok()
                .and_then(|r| r.to_str());
            resolve_track_for_path(&app.project.config.tracks, file_name, rel).map(|(id, _)| id)
        })
        .collect();
    app.pending_subtask_hides
        .retain(|ph| !affected_track_ids.contains(&ph.track_id));

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
    run_auto_clean(app, paths);
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
    run_auto_clean(app, &deduped);
}

/// Why auto-clean must not run for this external change, if it must not.
///
/// Auto-clean exists to normalise *human* edits — ticking a checkbox in an
/// editor should get a `resolved:` date filled in silently. But the watcher
/// reports only "this file changed", and git rewriting a track file looks
/// identical to a hand edit. Cleaning then fights the git operation: every done
/// task the checkout restored without a `resolved:` gets stamped with *today*,
/// the next `git restore` removes it, the watcher fires again, and the loop
/// repeats until one side gives up.
///
/// Two signals say git did this, not a person:
///
/// - a multi-step operation is unfinished (rebase, merge, cherry-pick, …), so
///   more rewrites are still coming; and
/// - a changed file is byte-identical to the index, which is what `git restore`,
///   `git checkout` and `git stash` leave behind but an editor save does not.
///
/// The second is what catches a bare `git restore`, which sets no marker file.
fn git_write_back_block(frame_dir: &Path, changed: &[PathBuf]) -> Option<&'static str> {
    use crate::io::git;
    if git::operation_in_progress(frame_dir) {
        return Some("git operation in progress");
    }
    if !git::index_clean_paths(frame_dir, changed).is_empty() {
        return Some("files restored by git");
    }
    None
}

/// Run auto-clean on the project after external changes are detected.
/// Assigns missing IDs/dates and saves affected tracks. Shows status message if anything changed.
///
/// Skipped entirely — rather than cleaned in memory and left unsaved — when
/// [`git_write_back_block`] fires, so that what the TUI displays keeps matching
/// what is on disk.
fn run_auto_clean(app: &mut App, changed: &[PathBuf]) {
    use crate::ops::clean::clean_project;

    if !app.project.config.clean.auto_clean {
        return;
    }
    if let Some(reason) = git_write_back_block(&app.project.frame_dir, changed) {
        app.status_message = Some(format!("Auto-clean skipped: {reason}"));
        return;
    }

    // Passive auto-clean after external changes — no auto-claim, and an
    // unclaimed clone mints nothing (strict null policy).
    let scope = crate::io::actors::id_scope(&app.project.frame_dir);
    let result = clean_project(&mut app.project, scope);

    let has_changes = !result.ids_assigned.is_empty()
        || !result.dates_assigned.is_empty()
        || !result.duplicates_resolved.is_empty()
        || !result.sections_reconciled.is_empty()
        || !result.tasks_archived.is_empty();

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
        for rec in &result.sections_reconciled {
            affected_tracks.insert(rec.track_id.clone());
        }
        for arc in &result.tasks_archived {
            affected_tracks.insert(arc.track_id.clone());
        }

        // Save affected tracks
        for track_id in &affected_tracks {
            app.save_track_logged(track_id);
        }

        // Add sync marker to undo stack so user can't undo past the external change
        app.undo_stack.push(crate::tui::undo::Operation::SyncMarker);

        // Show subtle status message
        let count = result.ids_assigned.len()
            + result.dates_assigned.len()
            + result.duplicates_resolved.len()
            + result.sections_reconciled.len()
            + result.tasks_archived.len();
        app.status_message = Some(format!(
            "Auto-cleaned: {} fix{}",
            count,
            if count == 1 { "" } else { "es" }
        ));
    }
}

// ---------------------------------------------------------------------------
// File-watcher path resolution helpers
// ---------------------------------------------------------------------------

/// Check whether a changed file is the real top-level inbox.md (not archive/inbox.md).
fn is_inbox_path(file_name: &str, rel_path: Option<&str>) -> bool {
    file_name == "inbox.md" && rel_path.is_none_or(|r| r == "inbox.md")
}

/// Resolve a changed file path to a track config entry.
/// Uses the relative path from frame_dir when available (preferred — exact match).
/// Falls back to filename-only matching when the path can't be relativized.
fn resolve_track_for_path(
    tracks: &[crate::model::config::TrackConfig],
    file_name: &str,
    rel_path: Option<&str>,
) -> Option<(String, String)> {
    tracks
        .iter()
        .find(|tc| {
            if let Some(rel) = rel_path {
                tc.file == rel
            } else {
                tc.file == file_name || tc.file.ends_with(&format!("/{}", file_name))
            }
        })
        .map(|tc| (tc.id.clone(), tc.file.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::config::TrackConfig;

    fn sample_tracks() -> Vec<TrackConfig> {
        vec![
            TrackConfig {
                id: "main".to_string(),
                name: "Main".to_string(),
                state: "active".to_string(),
                file: "tracks/main.md".to_string(),
            },
            TrackConfig {
                id: "research".to_string(),
                name: "Research".to_string(),
                state: "active".to_string(),
                file: "tracks/research.md".to_string(),
            },
        ]
    }

    // --- is_inbox_path ---

    #[test]
    fn inbox_top_level_matches() {
        assert!(is_inbox_path("inbox.md", Some("inbox.md")));
    }

    #[test]
    fn inbox_archive_does_not_match() {
        assert!(!is_inbox_path("inbox.md", Some("archive/inbox.md")));
    }

    #[test]
    fn inbox_no_rel_path_matches() {
        // Fallback when strip_prefix fails — accept it
        assert!(is_inbox_path("inbox.md", None));
    }

    #[test]
    fn non_inbox_does_not_match() {
        assert!(!is_inbox_path("main.md", Some("tracks/main.md")));
    }

    // --- resolve_track_for_path ---

    #[test]
    fn track_file_matches_by_rel_path() {
        let tracks = sample_tracks();
        let result = resolve_track_for_path(&tracks, "main.md", Some("tracks/main.md"));
        assert_eq!(
            result,
            Some(("main".to_string(), "tracks/main.md".to_string()))
        );
    }

    #[test]
    fn archive_file_does_not_match_track() {
        let tracks = sample_tracks();
        let result = resolve_track_for_path(&tracks, "main.md", Some("archive/main.md"));
        assert_eq!(result, None);
    }

    #[test]
    fn archive_tracks_subdir_does_not_match() {
        let tracks = sample_tracks();
        let result = resolve_track_for_path(&tracks, "main.md", Some("archive/_tracks/main.md"));
        assert_eq!(result, None);
    }

    #[test]
    fn different_track_name_in_archive() {
        let tracks = sample_tracks();
        let result = resolve_track_for_path(&tracks, "research.md", Some("archive/research.md"));
        assert_eq!(result, None);
    }

    #[test]
    fn correct_track_file_matches() {
        let tracks = sample_tracks();
        let result = resolve_track_for_path(&tracks, "research.md", Some("tracks/research.md"));
        assert_eq!(
            result,
            Some(("research".to_string(), "tracks/research.md".to_string()))
        );
    }

    #[test]
    fn fallback_filename_matching_when_no_rel_path() {
        let tracks = sample_tracks();
        // When rel_path is None, falls back to filename suffix matching
        let result = resolve_track_for_path(&tracks, "main.md", None);
        assert_eq!(
            result,
            Some(("main".to_string(), "tracks/main.md".to_string()))
        );
    }

    #[test]
    fn unrelated_md_file_does_not_match() {
        let tracks = sample_tracks();
        let result = resolve_track_for_path(&tracks, "notes.md", Some("notes.md"));
        assert_eq!(result, None);
    }

    #[test]
    fn flat_track_config_matches_exactly() {
        // Track config with file directly in frame_dir (no subdirectory)
        let tracks = vec![TrackConfig {
            id: "main".to_string(),
            name: "Main".to_string(),
            state: "active".to_string(),
            file: "main.md".to_string(),
        }];
        let result = resolve_track_for_path(&tracks, "main.md", Some("main.md"));
        assert_eq!(result, Some(("main".to_string(), "main.md".to_string())));
    }

    #[test]
    fn flat_config_archive_does_not_match() {
        let tracks = vec![TrackConfig {
            id: "main".to_string(),
            name: "Main".to_string(),
            state: "active".to_string(),
            file: "main.md".to_string(),
        }];
        let result = resolve_track_for_path(&tracks, "main.md", Some("archive/main.md"));
        assert_eq!(result, None);
    }

    // --- flatten_board_tasks ---

    #[test]
    fn flatten_board_tasks_includes_subtasks() {
        use crate::model::TaskState;

        let mut parent = Task::new(TaskState::Todo, Some("EFF-001".into()), "Parent".into());
        let child1 = Task::new(
            TaskState::Active,
            Some("EFF-001.1".into()),
            "Child 1".into(),
        );
        let mut child2 = Task::new(TaskState::Todo, Some("EFF-001.2".into()), "Child 2".into());
        let grandchild = Task::new(
            TaskState::Active,
            Some("EFF-001.2.1".into()),
            "Grandchild".into(),
        );
        child2.subtasks.push(grandchild);
        parent.subtasks.push(child1);
        parent.subtasks.push(child2);

        let flat = App::flatten_board_tasks(&parent);
        let ids: Vec<&str> = flat.iter().filter_map(|t| t.id.as_deref()).collect();
        assert_eq!(ids, ["EFF-001", "EFF-001.1", "EFF-001.2", "EFF-001.2.1"]);
        assert_eq!(flat[1].state, TaskState::Active);
        assert_eq!(flat[3].state, TaskState::Active);
    }

    // --- build_board_columns with subtasks ---

    #[test]
    fn board_columns_include_active_subtasks() {
        use crate::model::config::{CleanConfig, IdConfig, ProjectConfig, ProjectInfo, UiConfig};
        use crate::model::project::Project;
        use crate::parse::parse_track;

        let track_md = "\
# Test Track

## Backlog

- [ ] `T-001` Parent task
  - [>] `T-001.1` Active subtask
  - [ ] `T-001.2` Todo subtask
- [>] `T-002` Top-level active

## Done
";
        let track = parse_track(track_md);
        let config = ProjectConfig {
            project: ProjectInfo {
                name: "test".into(),
            },
            agent: Default::default(),
            tracks: vec![TrackConfig {
                id: "test".into(),
                name: "Test".into(),
                state: "active".into(),
                file: "tracks/test.md".into(),
            }],
            clean: CleanConfig::default(),
            ids: IdConfig::default(),
            ui: UiConfig::default(),
        };
        let project = Project {
            root: std::path::PathBuf::from("/tmp/test"),
            frame_dir: std::path::PathBuf::from("/tmp/test/frame"),
            config,
            tracks: vec![("test".into(), track)],
            inbox: None,
        };
        let mut app = App::new(project);
        // Switch to All mode (default is Cc which filters for #cc tags)
        app.board_state.mode = BoardMode::All;
        let [ready, in_progress, done] = app.build_board_columns();

        // Collect task IDs from each column
        let ready_ids: Vec<&str> = ready
            .iter()
            .filter_map(|item| match item {
                BoardItem::Task { task_id, .. } => Some(task_id.as_str()),
                _ => None,
            })
            .collect();
        let active_ids: Vec<&str> = in_progress
            .iter()
            .filter_map(|item| match item {
                BoardItem::Task { task_id, .. } => Some(task_id.as_str()),
                _ => None,
            })
            .collect();

        // T-001 is Todo with all deps resolved → Ready
        // T-001.2 is Todo with all deps resolved → Ready
        assert!(ready_ids.contains(&"T-001"), "T-001 should be in Ready");
        assert!(ready_ids.contains(&"T-001.2"), "T-001.2 should be in Ready");

        // T-001.1 is Active → In Progress
        // T-002 is Active → In Progress
        assert!(
            active_ids.contains(&"T-001.1"),
            "T-001.1 (active subtask) should be in In Progress, got: {:?}",
            active_ids
        );
        assert!(
            active_ids.contains(&"T-002"),
            "T-002 should be in In Progress"
        );

        // Done should be empty
        let done_ids: Vec<&str> = done
            .iter()
            .filter_map(|item| match item {
                BoardItem::Task { task_id, .. } => Some(task_id.as_str()),
                _ => None,
            })
            .collect();
        assert!(done_ids.is_empty(), "Done should be empty");
    }

    // --- board id_display does not double the track prefix (regression) ---

    #[test]
    fn board_id_display_does_not_double_prefix() {
        use crate::model::config::{CleanConfig, IdConfig, ProjectConfig, ProjectInfo, UiConfig};
        use crate::model::project::Project;
        use crate::parse::parse_track;

        let track_md = "\
# Stuff

## Backlog

- [ ] `ST-001` Parent task
  - [ ] `ST-001.2` Todo subtask
- [>] `ST-002` Top-level active

## Done
";
        let track = parse_track(track_md);
        let mut prefixes = indexmap::IndexMap::new();
        prefixes.insert("stuff".to_string(), "ST".to_string());
        let config = ProjectConfig {
            project: ProjectInfo {
                name: "test".into(),
            },
            agent: Default::default(),
            tracks: vec![TrackConfig {
                id: "stuff".into(),
                name: "Stuff".into(),
                state: "active".into(),
                file: "tracks/stuff.md".into(),
            }],
            clean: CleanConfig::default(),
            ids: IdConfig { prefixes },
            ui: UiConfig::default(),
        };
        let project = Project {
            root: std::path::PathBuf::from("/tmp/test"),
            frame_dir: std::path::PathBuf::from("/tmp/test/frame"),
            config,
            tracks: vec![("stuff".into(), track)],
            inbox: None,
        };
        let mut app = App::new(project);
        app.board_state.mode = BoardMode::All;
        let [ready, in_progress, _done] = app.build_board_columns();

        // Map task_id -> id_display across both populated columns.
        let displays: std::collections::HashMap<String, String> = ready
            .iter()
            .chain(in_progress.iter())
            .filter_map(|item| match item {
                BoardItem::Task {
                    task_id,
                    id_display,
                    ..
                } => Some((task_id.clone(), id_display.clone())),
                _ => None,
            })
            .collect();

        // Top-level id is rendered as stored, not doubled.
        assert_eq!(
            displays.get("ST-001").map(String::as_str),
            Some("ST-001"),
            "expected ST-001, not ST-ST-001"
        );
        // Subtask id is rendered as stored, not doubled.
        assert_eq!(
            displays.get("ST-001.2").map(String::as_str),
            Some("ST-001.2"),
            "expected ST-001.2, not ST-ST-001.2"
        );
        // Active top-level id is also un-doubled.
        assert_eq!(
            displays.get("ST-002").map(String::as_str),
            Some("ST-002"),
            "expected ST-002, not ST-ST-002"
        );
    }

    // --- inverse dep index resolves tokened ids on both ends ---

    #[test]
    fn build_dep_index_resolves_tokened_ids() {
        use crate::model::config::{CleanConfig, IdConfig, ProjectConfig, ProjectInfo, UiConfig};
        use crate::model::project::Project;
        use crate::parse::parse_track;

        // EFF-b2 depends on the tokened EFF-a14; EFF-c3 depends on the null
        // EFF-14. Both edges must resolve to the canonical-form target id.
        let track = parse_track(
            "\
# Eff

## Backlog

- [ ] `EFF-a14` Upstream tokened
- [ ] `EFF-14` Upstream null
- [ ] `EFF-b2` Downstream of tokened
  - dep: EFF-a14
- [ ] `EFF-c3` Downstream of null
  - dep: EFF-14

## Done
",
        );
        let config = ProjectConfig {
            project: ProjectInfo {
                name: "test".into(),
            },
            agent: Default::default(),
            tracks: vec![TrackConfig {
                id: "eff".into(),
                name: "Eff".into(),
                state: "active".into(),
                file: "tracks/eff.md".into(),
            }],
            clean: CleanConfig::default(),
            ids: IdConfig::default(),
            ui: UiConfig::default(),
        };
        let project = Project {
            root: std::path::PathBuf::from("/tmp/test"),
            frame_dir: std::path::PathBuf::from("/tmp/test/frame"),
            config,
            tracks: vec![("eff".into(), track)],
            inbox: None,
        };

        let index = App::build_dep_index(&project);
        // The tokened upstream id maps to its tokened dependent, distinct from
        // the null-namespace edge.
        assert_eq!(
            index.get("EFF-a14").map(Vec::as_slice),
            Some(&["EFF-b2".to_string()][..])
        );
        assert_eq!(
            index.get("EFF-14").map(Vec::as_slice),
            Some(&["EFF-c3".to_string()][..])
        );
    }

    // ---- Saving --------------------------------------------------------

    /// A project on disk, so the save paths have somewhere real to write.
    fn app_on_disk(dir: &std::path::Path) -> App {
        use crate::model::config::{CleanConfig, IdConfig, ProjectConfig, ProjectInfo, UiConfig};
        let frame_dir = dir.join("frame");
        std::fs::create_dir_all(frame_dir.join("tracks")).unwrap();
        std::fs::write(
            frame_dir.join("tracks/a.md"),
            "# A\n\n## Backlog\n\n- [ ] `A-001` One\n\n## Done\n",
        )
        .unwrap();
        std::fs::write(frame_dir.join("inbox.md"), "# Inbox\n").unwrap();
        let config = ProjectConfig {
            project: ProjectInfo {
                name: "saves".into(),
            },
            agent: Default::default(),
            tracks: vec![TrackConfig {
                id: "a".into(),
                name: "A".into(),
                state: "active".into(),
                file: "tracks/a.md".into(),
            }],
            clean: CleanConfig::default(),
            ids: IdConfig::default(),
            ui: UiConfig::default(),
        };
        let project = crate::model::project::Project {
            root: dir.to_path_buf(),
            frame_dir,
            config,
            tracks: vec![(
                "a".into(),
                crate::parse::parse_track("# A\n\n## Backlog\n\n- [ ] `A-001` One\n\n## Done\n"),
            )],
            inbox: Some(crate::parse::parse_inbox("# Inbox\n").0),
        };
        App::new(project)
    }

    // ---- UI state persistence ------------------------------------------

    /// A search is view state, not a preference: restoring the pattern brought
    /// back the `/pattern` status bar, the match highlighting and the `n`/`N`
    /// and `Esc` rebinds in a session where nobody had searched for anything --
    /// and without the match index or count, so it was not even the search you
    /// left. The rest of the restore has to keep working.
    #[test]
    fn restore_drops_a_persisted_search_but_keeps_the_view() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut app = app_on_disk(tmp.path());
        std::fs::write(
            app.project.frame_dir.join(".state.json"),
            r#"{"view":"recent","last_search":"widget","search_history":["widget"]}"#,
        )
        .unwrap();

        restore_ui_state(&mut app);

        assert!(app.last_search.is_none(), "search must not come back");
        assert_eq!(app.view, View::Recent, "the rest of the restore still runs");
        assert_eq!(app.search_history, vec!["widget".to_string()]);
    }

    #[test]
    fn save_does_not_write_the_active_search() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut app = app_on_disk(tmp.path());
        app.last_search = Some("widget".into());

        save_ui_state(&app);

        let text = std::fs::read_to_string(app.project.frame_dir.join(".state.json")).unwrap();
        assert!(
            !text.contains("last_search"),
            "search pattern leaked into .state.json:\n{text}"
        );
    }

    // ---- Auto-clean write-back -----------------------------------------

    /// Point `app` at a track holding one done task with no `resolved:` — the
    /// shape auto-clean backfills, and the shape a git checkout keeps restoring.
    fn with_dateless_done_task(app: &mut App) -> std::path::PathBuf {
        const TRACK: &str = "# A\n\n## Backlog\n\n## Done\n\n- [x] `A-001` Finished long ago\n  - added: 2026-01-06\n";
        let path = app.project.frame_dir.join("tracks").join("a.md");
        std::fs::write(&path, TRACK).unwrap();
        app.project.tracks = vec![("a".into(), crate::parse::parse_track(TRACK))];
        path
    }

    #[test]
    fn auto_clean_backfills_a_resolved_date_by_default() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut app = app_on_disk(tmp.path());
        let path = with_dateless_done_task(&mut app);

        run_auto_clean(&mut app, &[]);

        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("resolved:"), "auto-clean still runs:\n{text}");
    }

    /// `clean.auto_clean` shipped in the config template documented as "run clean
    /// after file reload in TUI" but was never read, so turning it off did
    /// nothing.
    #[test]
    fn auto_clean_disabled_writes_nothing() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut app = app_on_disk(tmp.path());
        let path = with_dateless_done_task(&mut app);
        let before = std::fs::read_to_string(&path).unwrap();

        app.project.config.clean.auto_clean = false;
        run_auto_clean(&mut app, &[]);

        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            before,
            "auto_clean = false must leave the file alone"
        );
    }

    /// The reported bug: the TUI re-added `resolved:` backfills after every
    /// `git restore`, fighting a rebase. A file git just put back must not be
    /// cleaned — and must not be cleaned *in memory* either, or the TUI would
    /// display invented dates that are not on disk.
    #[test]
    fn auto_clean_skips_files_git_restored() {
        let tmp = tempfile::TempDir::new().unwrap();
        let Some(frame_dir) = crate::io::git::testutil::repo_with_committed_track(tmp.path())
        else {
            return; // git unavailable
        };
        let root = frame_dir.parent().unwrap().to_path_buf();
        let mut app = app_on_disk(&root);
        let path = with_dateless_done_task(&mut app);

        // Commit the dateless done task, so restoring returns to exactly this.
        assert!(crate::io::git::testutil::commit_all(&root));
        let before = std::fs::read_to_string(&path).unwrap();

        run_auto_clean(&mut app, std::slice::from_ref(&path));

        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            before,
            "a git-restored file must not be rewritten"
        );
        assert!(
            !crate::parse::serialize_track(&app.project.tracks[0].1).contains("resolved:"),
            "in-memory state must match disk, not carry an unsaved backfill"
        );
        assert!(
            app.status_message
                .as_deref()
                .is_some_and(|m| m.contains("Auto-clean skipped")),
            "the skip should be visible: {:?}",
            app.status_message
        );
    }

    /// The point of item 5: a failed save is recorded, not discarded. 61 sites
    /// used to `let _ = ...` it away, leaving the TUI showing state that was not
    /// on disk with nothing written down anywhere.
    #[test]
    fn save_failure_is_recorded() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut app = app_on_disk(tmp.path());

        // A track id that resolves to no file — the shape `let _ =` hid.
        app.save_track_logged("nonexistent");

        let log = crate::io::recovery::recovery_log_path(&app.project.frame_dir);
        let text = std::fs::read_to_string(&log).expect("a failure should be logged");
        assert!(text.contains("nonexistent"), "{text}");
        assert!(text.contains("save failed"), "{text}");
    }

    /// The bug this set exists for. A save fails, so the only copy of the edit
    /// is in memory; the file then changes externally — which is *likely*, not
    /// exotic, because the usual reason a save fails is another `fr` holding the
    /// lock, and that process goes on to write the file. The reload used to
    /// replace the track unconditionally, destroying the edit with nothing but
    /// an error string in the recovery log to show for it.
    #[test]
    fn external_change_does_not_overwrite_an_unsaved_track() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut app = app_on_disk(tmp.path());
        let path = app.project.frame_dir.join("tracks/a.md");

        // An edit that lives only in memory, and a save that did not land.
        let track = app.find_track_mut("a").unwrap();
        let tasks = track.section_tasks_mut(SectionKind::Backlog).unwrap();
        tasks[0].title = "One, edited in the TUI".into();
        tasks[0].source_text = Some(vec!["- [ ] `A-001` One, edited in the TUI".to_string()]);
        app.record_save_failure(SaveTarget::Track("a".into()), &"lock timeout".to_string());

        // Another writer lands a different version of the same file.
        std::fs::write(
            &path,
            "# A\n\n## Backlog\n\n- [ ] `A-001` One, edited elsewhere\n\n## Done\n",
        )
        .unwrap();
        app.reload_changed_files(std::slice::from_ref(&path));

        let title = &app.find_track_mut("a").unwrap().backlog()[0].title;
        assert_eq!(
            title, "One, edited in the TUI",
            "the unsaved in-memory edit must survive an external write"
        );

        // Theirs is not dropped either — it goes to the recovery log in full.
        let log = crate::io::recovery::recovery_log_path(&app.project.frame_dir);
        let text = std::fs::read_to_string(&log).unwrap();
        assert!(
            text.contains("One, edited elsewhere"),
            "the external version must be preserved, not discarded: {text}"
        );
    }

    /// The case the merge exists for, end to end through the reload path: we
    /// edited one task, another writer added a different one. Keeping ours
    /// wholesale would drop their task — which, with several agent sessions on
    /// one project, is a write nobody would ever notice going missing.
    #[test]
    fn an_external_addition_merges_into_an_unsaved_track() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut app = app_on_disk(tmp.path());
        let path = app.project.frame_dir.join("tracks/a.md");

        let tasks = app
            .find_track_mut("a")
            .unwrap()
            .section_tasks_mut(SectionKind::Backlog)
            .unwrap();
        tasks[0].title = "One, edited in the TUI".into();
        tasks[0].source_text = Some(vec!["- [ ] `A-001` One, edited in the TUI".to_string()]);
        app.record_save_failure(SaveTarget::Track("a".into()), &"lock timeout".to_string());

        // They add a task without touching ours.
        std::fs::write(
            &path,
            "# A\n\n## Backlog\n\n- [ ] `A-001` One\n- [ ] `A-002` Theirs\n\n## Done\n",
        )
        .unwrap();
        app.reload_changed_files(std::slice::from_ref(&path));

        let backlog = app.find_track_mut("a").unwrap().backlog().to_vec();
        let titles: Vec<&str> = backlog.iter().map(|t| t.title.as_str()).collect();
        assert!(
            titles.contains(&"One, edited in the TUI"),
            "our edit must survive: {titles:?}"
        );
        assert!(
            titles.contains(&"Theirs"),
            "their addition must survive too: {titles:?}"
        );

        // Nothing was in dispute, so nothing needed preserving out-of-band.
        let log = crate::io::recovery::recovery_log_path(&app.project.frame_dir);
        let text = std::fs::read_to_string(&log).unwrap_or_default();
        assert!(
            !text.contains("concurrent edit"),
            "a clean merge should record no conflict: {text}"
        );
    }

    /// The inbox has no IDs, so it merges by content rather than by identity —
    /// but through the same reload path, and with the same guarantee: a capture
    /// on either side survives.
    #[test]
    fn an_external_capture_merges_into_an_unsaved_inbox() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut app = app_on_disk(tmp.path());
        let path = app.project.frame_dir.join("inbox.md");

        // Ours: a capture that never reached disk.
        let inbox = app.project.inbox.as_mut().unwrap();
        inbox.items.push(crate::model::inbox::InboxItem::new(
            "captured here".to_string(),
        ));
        app.record_save_failure(SaveTarget::Inbox, &"lock timeout".to_string());

        // Theirs: a different capture, written by another process.
        std::fs::write(&path, "# Inbox\n\n- captured elsewhere\n").unwrap();
        app.reload_changed_files(std::slice::from_ref(&path));

        let titles: Vec<&str> = app
            .project
            .inbox
            .as_ref()
            .unwrap()
            .items
            .iter()
            .map(|i| i.title.as_str())
            .collect();
        assert!(titles.contains(&"captured here"), "{titles:?}");
        assert!(titles.contains(&"captured elsewhere"), "{titles:?}");

        // The inbox merge never sets a side aside, so it logs nothing.
        let log = crate::io::recovery::recovery_log_path(&app.project.frame_dir);
        let text = std::fs::read_to_string(&log).unwrap_or_default();
        assert!(
            !text.contains("kept the in-memory version"),
            "the inbox should merge rather than pick a winner: {text}"
        );
    }

    /// The guard is keyed on the file, not on "something is unsaved somewhere":
    /// an unrelated track still reloads normally.
    #[test]
    fn external_change_still_reloads_a_saved_track() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut app = app_on_disk(tmp.path());
        let path = app.project.frame_dir.join("tracks/a.md");

        app.record_save_failure(SaveTarget::Inbox, &"lock timeout".to_string());

        std::fs::write(
            &path,
            "# A\n\n## Backlog\n\n- [ ] `A-001` One, edited elsewhere\n\n## Done\n",
        )
        .unwrap();
        app.reload_changed_files(std::slice::from_ref(&path));

        assert_eq!(
            app.find_track_mut("a").unwrap().backlog()[0].title,
            "One, edited elsewhere",
            "a track with no outstanding save must still pick up external changes"
        );
    }

    /// A successful save retires the entry, so the guard stops firing and normal
    /// reload behaviour resumes.
    #[test]
    fn a_successful_save_clears_the_unsaved_entry() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut app = app_on_disk(tmp.path());

        app.record_save_failure(SaveTarget::Track("a".into()), &"lock timeout".to_string());
        assert!(!app.unsaved.is_empty());

        app.save_track_logged("a");
        assert!(
            app.unsaved.is_empty(),
            "the file reached disk; it is no longer outstanding"
        );
    }

    /// One file failing in a batch must not mark the others unsaved — the badge
    /// and the exit report both read this set as "what is actually at risk".
    #[test]
    fn a_partial_batch_leaves_only_the_failed_file() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut app = app_on_disk(tmp.path());

        app.save_batch_logged(&["a", "nonexistent"], true);

        assert_eq!(
            app.unsaved.keys().collect::<Vec<_>>(),
            vec![&SaveTarget::Track("nonexistent".into())],
            "only the write that failed is outstanding"
        );
    }

    // ---- Exit ------------------------------------------------------------

    /// At exit the in-memory copy stops existing. If it never reached disk and
    /// nothing is written down, the work is simply gone — which is the failure
    /// this whole item is about.
    #[test]
    fn unsaved_work_is_dumped_at_exit() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut app = app_on_disk(tmp.path());

        let tasks = app
            .find_track_mut("a")
            .unwrap()
            .section_tasks_mut(SectionKind::Backlog)
            .unwrap();
        tasks[0].title = "Never reached disk".into();
        tasks[0].dirty = true;
        app.record_save_failure(
            SaveTarget::Track("a".into()),
            &"Read-only file system".to_string(),
        );

        let written = app.dump_unsaved();
        assert_eq!(written.len(), 1, "the unsaved track should be dumped");
        let text = std::fs::read_to_string(&written[0]).unwrap();
        assert!(
            text.contains("Never reached disk"),
            "the dump must hold the in-memory content: {text}"
        );
        assert!(
            written[0].starts_with(app.project.frame_dir.join(RESCUE_DIR)),
            "dumped to {:?}",
            written[0]
        );
    }

    #[test]
    fn nothing_outstanding_dumps_nothing() {
        let tmp = tempfile::TempDir::new().unwrap();
        let app = app_on_disk(tmp.path());
        assert!(app.dump_unsaved().is_empty());
        assert!(
            !app.project.frame_dir.join(RESCUE_DIR).exists(),
            "a clean exit should not leave a rescue directory behind"
        );
    }

    #[test]
    fn the_exit_report_names_each_file_and_where_the_copy_went() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut app = app_on_disk(tmp.path());
        app.record_save_failure(
            SaveTarget::Track("a".into()),
            &"Read-only file system".to_string(),
        );

        let rescued = app.dump_unsaved();
        let report = unsaved_exit_report(&app, &rescued).expect("there is something to report");
        assert!(report.contains("a.md"), "{report}");
        assert!(report.contains("Read-only file system"), "{report}");
        assert!(report.contains(RESCUE_DIR), "{report}");
        assert!(report.contains(".recovery.log"), "{report}");
    }

    /// The loudest case: the dump failed too, so there is nothing anywhere. Say
    /// so plainly rather than pointing at a directory that does not exist.
    #[test]
    fn the_exit_report_admits_when_there_is_no_rescue_copy() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut app = app_on_disk(tmp.path());
        app.record_save_failure(
            SaveTarget::Track("a".into()),
            &"Read-only file system".to_string(),
        );

        let report = unsaved_exit_report(&app, &[]).unwrap();
        assert!(report.contains("No rescue copy"), "{report}");
        assert!(
            !report.contains("Move them into place"),
            "must not point at copies that do not exist: {report}"
        );
    }

    #[test]
    fn a_clean_exit_reports_nothing() {
        let tmp = tempfile::TempDir::new().unwrap();
        let app = app_on_disk(tmp.path());
        assert!(unsaved_exit_report(&app, &[]).is_none());
    }

    /// The rescue directory is working-copy-local, so `fr check`'s leak guard
    /// has to know about it — and the `frame/.*` gitignore pattern has to cover
    /// it, which it does only because the name starts with a dot.
    #[test]
    fn the_rescue_directory_is_treated_as_working_copy_local() {
        assert!(
            crate::io::project_io::LOCAL_ONLY_FRAME_FILES.contains(&RESCUE_DIR),
            "the leak guard must cover the rescue directory"
        );
        assert!(
            RESCUE_DIR.starts_with('.'),
            "`frame/.*` only covers it if it is a dotfile"
        );
    }

    // ---- Startup probe ---------------------------------------------------

    #[test]
    fn a_writable_project_probes_clean() {
        let tmp = tempfile::TempDir::new().unwrap();
        let app = app_on_disk(tmp.path());
        assert!(!app.frame_unwritable);
        assert!(probe_unwritable(&app.project.frame_dir).is_none());
        assert!(
            !app.project.frame_dir.join(".write-probe").exists(),
            "the probe must clean up after itself"
        );
    }

    #[test]
    fn an_unwritable_project_is_detected_before_any_edit() {
        let tmp = tempfile::TempDir::new().unwrap();
        let app = app_on_disk(tmp.path());
        let frame_dir = app.project.frame_dir.clone();

        let mut perms = std::fs::metadata(&frame_dir).unwrap().permissions();
        let original = perms.clone();
        perms.set_readonly(true);
        std::fs::set_permissions(&frame_dir, perms).unwrap();

        let probed = probe_unwritable(&frame_dir);

        std::fs::set_permissions(&frame_dir, original).unwrap();
        assert!(
            probed.is_some(),
            "a read-only frame/ should be caught at startup"
        );
    }

    // ---- Surfacing -------------------------------------------------------

    /// The narrow case worth suppressing: a failure the very next retry clears.
    /// It should reach neither the screen nor the log — a flash of alarm and a
    /// junk entry for a problem that fixed itself.
    #[test]
    fn a_failure_the_retry_clears_is_never_announced() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut app = app_on_disk(tmp.path());

        {
            let _held = FileLock::acquire_default(&app.project.frame_dir).unwrap();
            app.save_track_logged("a");
        }
        assert!(!app.unsaved.is_empty(), "the save should have failed");
        assert!(
            app.unsaved_indicator().is_none(),
            "one failure is not yet worth announcing"
        );

        app.retry_unsaved_saves(true);
        assert!(app.unsaved.is_empty());

        let log = crate::io::recovery::recovery_log_path(&app.project.frame_dir);
        assert!(
            !log.exists(),
            "a blip resolved by the next retry should leave nothing behind"
        );
    }

    /// A second failure means it is not fixing itself, so it is announced —
    /// once, however many further attempts fail.
    #[test]
    fn a_sustained_failure_is_announced_exactly_once() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut app = app_on_disk(tmp.path());
        let target = SaveTarget::Track("a".into());

        app.record_save_failure(target.clone(), &"lock timeout".to_string());
        assert!(app.unsaved_indicator().is_none());

        app.record_save_failure(target.clone(), &"lock timeout".to_string());
        assert!(
            app.unsaved_indicator().is_some(),
            "a failure that survives a retry should be announced"
        );

        for _ in 0..20 {
            app.record_save_failure(target.clone(), &"lock timeout".to_string());
        }

        let log = crate::io::recovery::recovery_log_path(&app.project.frame_dir);
        let text = std::fs::read_to_string(&log).unwrap();
        assert_eq!(
            text.matches("save failed").count(),
            1,
            "22 failures are one incident, not 22 log entries:\n{text}"
        );
    }

    /// Nothing about waiting clears a read-only filesystem, so there is no point
    /// holding the news back for a retry that cannot succeed.
    #[test]
    fn a_permanent_failure_is_announced_immediately() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut app = app_on_disk(tmp.path());

        app.record_save_failure(
            SaveTarget::Track("a".into()),
            &"Permission denied (os error 13)".to_string(),
        );

        let ind = app
            .unsaved_indicator()
            .expect("should be announced at once");
        assert!(ind.waiting_for_user, "no timer will clear this");
        assert!(ind.full().contains("Permission denied"), "{}", ind.full());
    }

    #[test]
    fn the_indicator_names_one_file_and_counts_several() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut app = app_on_disk(tmp.path());

        for _ in 0..2 {
            app.record_save_failure(SaveTarget::Track("a".into()), &"lock timeout".to_string());
        }
        let ind = app.unsaved_indicator().unwrap();
        assert_eq!(ind.only.as_deref(), Some("a.md"));
        assert!(ind.short().contains("a.md"), "{}", ind.short());

        for _ in 0..2 {
            app.record_save_failure(SaveTarget::Inbox, &"lock timeout".to_string());
        }
        let ind = app.unsaved_indicator().unwrap();
        assert_eq!(ind.count, 2);
        assert!(ind.short().contains("2 files"), "{}", ind.short());
    }

    /// The indicator clears only when *every* outstanding file has saved, not
    /// when any one does.
    #[test]
    fn the_indicator_stays_up_while_anything_is_outstanding() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut app = app_on_disk(tmp.path());

        for _ in 0..2 {
            app.record_save_failure(SaveTarget::Track("a".into()), &"lock timeout".to_string());
            app.record_save_failure(SaveTarget::Inbox, &"lock timeout".to_string());
        }
        assert_eq!(app.unsaved_indicator().unwrap().count, 2);

        app.save_inbox_logged();
        assert_eq!(
            app.unsaved_indicator().map(|i| i.count),
            Some(1),
            "one file saving does not clear the warning"
        );

        app.save_track_logged("a");
        assert!(
            app.unsaved_indicator().is_none(),
            "with nothing outstanding the indicator goes away"
        );
    }

    /// An announced incident gets a closing entry, so the log reads as pairs
    /// rather than an unexplained failure that never resolves.
    #[test]
    fn a_resolved_incident_is_recorded() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut app = app_on_disk(tmp.path());

        for _ in 0..2 {
            app.record_save_failure(SaveTarget::Track("a".into()), &"lock timeout".to_string());
        }
        app.save_track_logged("a");

        let log = crate::io::recovery::recovery_log_path(&app.project.frame_dir);
        let text = std::fs::read_to_string(&log).unwrap();
        assert!(text.contains("save failed"), "{text}");
        assert!(text.contains("saved after"), "the resolution too: {text}");
    }

    // ---- Retry -----------------------------------------------------------

    #[test]
    fn retry_backoff_doubles_and_stops_at_a_minute() {
        assert_eq!(retry_delay(1), Duration::from_secs(1));
        assert_eq!(retry_delay(2), Duration::from_secs(2));
        assert_eq!(retry_delay(3), Duration::from_secs(4));
        assert_eq!(retry_delay(7), RETRY_BACKOFF_MAX);
        assert_eq!(
            retry_delay(1000),
            RETRY_BACKOFF_MAX,
            "backoff must not run away"
        );
    }

    #[test]
    fn a_lock_timeout_is_transient_but_a_permission_error_is_not() {
        assert!(!is_permanent("lock timeout after 5s"));
        assert!(!is_permanent("Resource temporarily unavailable"));
        assert!(is_permanent("Permission denied (os error 13)"));
        assert!(is_permanent("Read-only file system"));
        assert!(is_permanent("No space left on device"));
    }

    /// The whole point of retrying: a save that failed on contention lands as
    /// soon as the lock frees, with no user action and nothing to replay.
    #[test]
    fn a_retry_lands_the_edit_once_the_lock_frees() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut app = app_on_disk(tmp.path());
        let path = app.project.frame_dir.join("tracks/a.md");

        // Another writer holds the lock; our save fails.
        let held = FileLock::acquire_default(&app.project.frame_dir).unwrap();
        let tasks = app
            .find_track_mut("a")
            .unwrap()
            .section_tasks_mut(SectionKind::Backlog)
            .unwrap();
        tasks[0].title = "Edited while contended".into();
        tasks[0].dirty = true;
        app.record_save_failure(SaveTarget::Track("a".into()), &"lock timeout".to_string());

        app.retry_unsaved_saves(true);
        assert!(
            !app.unsaved.is_empty(),
            "the lock is still held, so the retry must not have succeeded"
        );
        assert!(
            !std::fs::read_to_string(&path)
                .unwrap()
                .contains("Edited while contended"),
            "nothing should have been written while contended"
        );

        drop(held);

        app.retry_unsaved_saves(true);
        assert!(app.unsaved.is_empty(), "the retry should have landed it");
        assert!(
            std::fs::read_to_string(&path)
                .unwrap()
                .contains("Edited while contended"),
            "the edit reaches disk without the user doing anything"
        );
    }

    /// A retry must never block the event loop. `acquire_default` waits five
    /// seconds; using it here would freeze the TUI for five seconds a tick
    /// during exactly the contention being recovered from.
    #[test]
    fn a_retry_does_not_wait_on_a_held_lock() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut app = app_on_disk(tmp.path());
        let _held = FileLock::acquire_default(&app.project.frame_dir).unwrap();
        app.record_save_failure(SaveTarget::Track("a".into()), &"lock timeout".to_string());

        let started = Instant::now();
        app.retry_unsaved_saves(true);
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "retry blocked for {:?}; it must use a try-lock",
            started.elapsed()
        );
    }

    /// Backoff has to grow even when the lock itself is what we cannot get,
    /// otherwise a contended project is probed on every 250ms tick.
    #[test]
    fn a_failed_retry_pushes_the_next_one_further_out() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut app = app_on_disk(tmp.path());
        let _held = FileLock::acquire_default(&app.project.frame_dir).unwrap();
        app.record_save_failure(SaveTarget::Track("a".into()), &"lock timeout".to_string());

        let first = app.unsaved.values().next().unwrap().attempts;
        app.retry_unsaved_saves(true);
        let second = app.unsaved.values().next().unwrap().attempts;
        assert!(second > first, "a failed retry counts as an attempt");
    }

    /// A permanent error is not retried on the timer — nothing about waiting
    /// clears a read-only filesystem — but asking explicitly still tries.
    #[test]
    fn a_permanent_failure_waits_for_an_explicit_retry() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut app = app_on_disk(tmp.path());

        app.record_save_failure(
            SaveTarget::Track("a".into()),
            &"Read-only file system".to_string(),
        );
        let before = app.unsaved.values().next().unwrap().attempts;

        app.retry_unsaved_saves(false);
        assert_eq!(
            app.unsaved.values().next().map(|f| f.attempts),
            Some(before),
            "the timer must skip a failure retrying cannot clear"
        );

        // The file is actually writable here, so an explicit retry succeeds.
        app.force_retry_unsaved();
        assert!(
            app.unsaved.is_empty(),
            "an explicit retry still attempts it"
        );
    }

    /// The timer must respect the backoff, or the cadence is meaningless.
    #[test]
    fn the_timer_skips_a_file_whose_backoff_has_not_elapsed() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut app = app_on_disk(tmp.path());

        app.record_save_failure(SaveTarget::Track("a".into()), &"lock timeout".to_string());
        // The entry is due one second out, so an immediate tick does nothing.
        app.retry_unsaved_saves(false);
        assert!(
            !app.unsaved.is_empty(),
            "a retry fired before its backoff elapsed"
        );

        app.unsaved
            .values_mut()
            .for_each(|f| f.next_retry_at = Instant::now());
        app.retry_unsaved_saves(false);
        assert!(app.unsaved.is_empty(), "the retry should fire once due");
    }

    // ---- Section moves and undo ------------------------------------------

    /// Drive a key through the real input layer, the way a user would.
    fn press(app: &mut App, c: char) {
        use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
        crate::tui::input::handle_key(
            app,
            KeyEvent {
                code: KeyCode::Char(c),
                modifiers: KeyModifiers::NONE,
                kind: KeyEventKind::Press,
                state: KeyEventState::NONE,
            },
        );
    }

    fn with_parked_task(app: &mut App) {
        let track = app.find_track_mut("a").unwrap();
        track.ensure_section(SectionKind::Parked);
        let t = crate::model::task::Task::new(
            crate::model::task::TaskState::Parked,
            Some("A-009".parse().unwrap()),
            "Parked one".to_string(),
        );
        track
            .section_tasks_mut(SectionKind::Parked)
            .unwrap()
            .push(t);
    }

    fn section_of(app: &mut App, task_id: &str) -> Option<SectionKind> {
        let track = App::find_track_in_project(&app.project, "a")?;
        crate::ops::task_ops::top_level_section(track, task_id)
    }

    /// Undo has to put the task back in the section it came from, not just
    /// restore its state.
    ///
    /// `Operation::StateChange` restores state and the resolved date and leaves
    /// the task where it sits — so a section move scheduled alongside one needs
    /// an undo entry of its own. The un-park move used to skip it, on a comment
    /// claiming StateChange covered the reversal, and undoing an un-park left a
    /// `[~]` task sitting in the Backlog.
    #[test]
    fn undo_after_unparking_puts_the_task_back_in_parked() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut app = app_on_disk(tmp.path());
        with_parked_task(&mut app);

        app.view = View::Detail {
            track_id: "a".into(),
            task_id: "A-009".into(),
        };
        press(&mut app, 'o'); // un-park
        app.flush_all_pending_moves();
        assert_eq!(
            section_of(&mut app, "A-009"),
            Some(SectionKind::Backlog),
            "un-parking moves it to the Backlog"
        );

        app.undo_stack.undo(&mut app.project.tracks, None);
        assert_eq!(
            section_of(&mut app, "A-009"),
            Some(SectionKind::Parked),
            "undo must restore the section, not just the state"
        );
    }

    /// The view-dependent hole: reopening outside the Board and Recent views had
    /// no section move at all, so the task stayed in `## Done` as `[ ]`.
    #[test]
    fn reopening_from_the_detail_view_moves_the_task_out_of_done() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut app = app_on_disk(tmp.path());
        {
            let track = app.find_track_mut("a").unwrap();
            track.ensure_section(SectionKind::Done);
            let t = crate::model::task::Task::new(
                crate::model::task::TaskState::Done,
                Some("A-008".parse().unwrap()),
                "Finished".to_string(),
            );
            track.section_tasks_mut(SectionKind::Done).unwrap().push(t);
        }

        app.view = View::Detail {
            track_id: "a".into(),
            task_id: "A-008".into(),
        };
        press(&mut app, 'o');
        app.flush_all_pending_moves();

        assert_eq!(
            section_of(&mut app, "A-008"),
            Some(SectionKind::Backlog),
            "a reopened task must leave the Done section from any view"
        );
    }

    /// A task on its way out of Done keeps its resolved date for the grace
    /// period — the Done column and Recent both sort on it — and loses it when
    /// the move fires.
    #[test]
    fn the_resolved_date_survives_the_grace_period_and_not_the_move() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut app = app_on_disk(tmp.path());
        {
            let track = app.find_track_mut("a").unwrap();
            track.ensure_section(SectionKind::Done);
            let mut t = crate::model::task::Task::new(
                crate::model::task::TaskState::Done,
                Some("A-008".parse().unwrap()),
                "Finished".to_string(),
            );
            t.metadata
                .push(crate::model::task::Metadata::Resolved("2026-01-01".into()));
            track.section_tasks_mut(SectionKind::Done).unwrap().push(t);
        }

        app.view = View::Detail {
            track_id: "a".into(),
            task_id: "A-008".into(),
        };
        press(&mut app, 'o');

        let has_resolved = |app: &mut App| {
            let track = app.find_track_mut("a").unwrap();
            crate::ops::task_ops::find_task_mut_in_track(track, "A-008")
                .unwrap()
                .metadata
                .iter()
                .any(|m| m.key() == "resolved")
        };
        assert!(has_resolved(&mut app), "kept during the grace period");

        app.flush_all_pending_moves();
        assert!(!has_resolved(&mut app), "stripped when the move fires");
    }

    #[test]
    fn successful_save_records_nothing() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut app = app_on_disk(tmp.path());

        app.save_track_logged("a");
        app.save_inbox_logged();

        let log = crate::io::recovery::recovery_log_path(&app.project.frame_dir);
        assert!(
            !log.exists(),
            "a clean save must not write to the recovery log"
        );
    }

    /// A batch takes one lock for every write, so a multi-file operation cannot
    /// be interleaved by another writer partway through. All the writes land.
    #[test]
    fn batch_save_writes_everything_under_one_lock() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut app = app_on_disk(tmp.path());

        if let Some(inbox) = app.project.inbox.as_mut() {
            let mut item = crate::model::inbox::InboxItem::new("captured".into());
            item.dirty = true;
            inbox.items.push(item);
        }
        app.save_batch_logged(&["a"], true);

        let inbox = std::fs::read_to_string(app.project.frame_dir.join("inbox.md")).unwrap();
        assert!(inbox.contains("captured"), "inbox written: {inbox}");
        assert!(app.project.frame_dir.join("tracks/a.md").exists());

        let log = crate::io::recovery::recovery_log_path(&app.project.frame_dir);
        assert!(!log.exists(), "nothing failed, so nothing logged");
    }

    /// One failure in a batch does not abandon the rest — a partial write beats
    /// giving up on the remainder — and each failure is recorded separately.
    #[test]
    fn batch_save_reports_each_failure_and_continues() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut app = app_on_disk(tmp.path());

        app.save_batch_logged(&["nonexistent", "a"], false);

        let log = crate::io::recovery::recovery_log_path(&app.project.frame_dir);
        let text = std::fs::read_to_string(&log).expect("the bad track should be logged");
        assert!(text.contains("nonexistent"), "{text}");
        // The good one still landed.
        assert!(app.project.frame_dir.join("tracks/a.md").exists());
    }
}
