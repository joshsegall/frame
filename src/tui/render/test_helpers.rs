use std::path::PathBuf;

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::layout::Rect;

use crate::model::{Project, ProjectConfig, ProjectInfo, TrackConfig};
use crate::parse::{parse_inbox, parse_track};
use crate::tui::app::{App, DetailRegion, DetailState, ReturnView, View};

pub const TERM_W: u16 = 80;
pub const TERM_H: u16 = 24;

/// Render into an in-memory buffer and return plain text (no styles).
pub fn render_to_string<F>(w: u16, h: u16, f: F) -> String
where
    F: FnOnce(&mut ratatui::Frame, Rect),
{
    let backend = TestBackend::new(w, h);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|frame| {
            let area = frame.area();
            f(frame, area);
        })
        .unwrap();

    let buf = terminal.backend().buffer().clone();
    let w = buf.area.width as usize;
    let lines: Vec<String> = buf
        .content
        .chunks(w)
        .map(|row| {
            let s: String = row.iter().map(|cell| cell.symbol()).collect();
            s.trim_end().to_string()
        })
        .collect();

    // Trim trailing blank lines
    let end = lines
        .iter()
        .rposition(|l| !l.is_empty())
        .map_or(0, |i| i + 1);
    lines[..end].join("\n")
}

/// An empty project with no tracks and no inbox.
pub fn minimal_project() -> Project {
    Project {
        root: PathBuf::from("/tmp/test-frame"),
        frame_dir: PathBuf::from("/tmp/test-frame/frame"),
        config: ProjectConfig {
            project: ProjectInfo {
                name: "Test".into(),
            },
            agent: Default::default(),
            tracks: vec![],
            clean: Default::default(),
            ids: Default::default(),
            ui: Default::default(),
        },
        tracks: vec![],
        inbox: None,
    }
}

/// Build a project with a single parsed track.
pub fn project_with_track(id: &str, name: &str, md: &str) -> Project {
    let track = parse_track(md);
    let mut project = minimal_project();
    project.config.tracks.push(TrackConfig {
        id: id.to_string(),
        name: name.to_string(),
        state: "active".into(),
        file: format!("tracks/{}.md", id),
    });
    project.tracks.push((id.to_string(), track));
    project
}

/// Build an App with a single track from markdown.
pub fn app_with_track(md: &str) -> App {
    let project = project_with_track("test", "Test", md);
    App::new(project)
}

/// Build an App with inbox items parsed from markdown.
pub fn app_with_inbox(md: &str) -> App {
    let (inbox, _warnings) = parse_inbox(md);
    let mut project = project_with_track("stub", "Stub", "# Stub\n\n## Backlog\n\n## Done\n");
    project.inbox = Some(inbox);
    App::new(project)
}

/// Build an App in Detail view for a specific task.
pub fn app_in_detail_view(md: &str, task_id: &str) -> App {
    let mut app = app_with_track(md);
    app.view = View::Detail {
        track_id: "test".into(),
        task_id: task_id.into(),
    };
    app.detail_state = Some(DetailState {
        region: DetailRegion::Title,
        scroll_offset: 0,
        regions: vec![
            DetailRegion::Title,
            DetailRegion::Tags,
            DetailRegion::Added,
            DetailRegion::Deps,
            DetailRegion::Spec,
            DetailRegion::Refs,
            DetailRegion::Note,
            DetailRegion::Subtasks,
        ],
        return_view: ReturnView::Track(0),
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
    app
}

/// Build an App from the complex_track.md fixture.
pub fn app_with_fixture() -> App {
    let md = include_str!("../../../tests/fixtures/complex_track.md");
    app_with_track(md)
}

/// Convenience constant for a simple track with a few tasks.
pub const SIMPLE_TRACK_MD: &str = "\
# Test Track

## Backlog

- [ ] `T-1` First task #core
- [>] `T-2` Second task #design
- [-] `T-3` Third task (blocked)

## Done

- [x] `T-4` Completed task
  - resolved: 2025-05-14
";

/// Empty inbox markdown.
pub const EMPTY_INBOX_MD: &str = "# Inbox\n";

/// Inbox with a few items.
pub const INBOX_MD: &str = "\
# Inbox

- First inbox item #bug
  Some body text here.

- Second item #design

- Third item
";
