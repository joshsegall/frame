use std::io;
use std::time::Duration;

use crossterm::event::{self, Event, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

use crate::io::project_io::{discover_project, load_project};
use crate::model::Project;

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
    // Future phases: Edit, Move, Search
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

        App {
            project,
            view: initial_view,
            mode: Mode::Navigate,
            should_quit: false,
            theme,
            active_track_ids,
        }
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
}

/// Run the TUI application
pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    // Discover and load project
    let cwd = std::env::current_dir()?;
    let root = discover_project(&cwd)?;
    let project = load_project(&root)?;

    let mut app = App::new(project);

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
    loop {
        terminal.draw(|frame| render::render(frame, app))?;

        if event::poll(Duration::from_millis(250))?
            && let Event::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
        {
            input::handle_key(app, key);
        }

        if app.should_quit {
            break;
        }
    }
    Ok(())
}
