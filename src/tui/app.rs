use std::collections::{HashMap, HashSet};
use std::io;
use std::time::Duration;

use crossterm::event::{self, Event, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use regex::Regex;

use crate::io::project_io::{discover_project, load_project};
use crate::model::{Project, SectionKind, Task, Track};

use super::input;
use super::render;
use super::theme::Theme;

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
}

/// Current interaction mode
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Navigate,
    Search,
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

    /// Get the active search regex for highlighting.
    /// In Search mode: compiles from current input. In Navigate: compiles from last_search.
    pub fn active_search_re(&self) -> Option<Regex> {
        let pattern = match self.mode {
            Mode::Search if !self.search_input.is_empty() => &self.search_input,
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
}

/// Save UI state to .state.json
pub fn save_ui_state(app: &App) {
    use crate::io::state::{TrackUiState, UiState, write_ui_state};

    let (view_str, active_track) = match &app.view {
        View::Track(idx) => (
            "track".to_string(),
            app.active_track_ids.get(*idx).cloned().unwrap_or_default(),
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
    };

    let _ = write_ui_state(&app.project.frame_dir, &ui_state);
}

/// Run the TUI application
pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    // Discover and load project
    let cwd = std::env::current_dir()?;
    let root = discover_project(&cwd)?;
    let project = load_project(&root)?;

    let mut app = App::new(project);

    // Restore saved UI state
    restore_ui_state(&mut app);

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
    let result = run_event_loop(&mut terminal, &mut app);

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
) -> Result<(), Box<dyn std::error::Error>> {
    let mut save_counter = 0u32;
    loop {
        terminal.draw(|frame| render::render(frame, app))?;

        if event::poll(Duration::from_millis(250))?
            && let Event::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
        {
            input::handle_key(app, key);
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
