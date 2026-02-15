use crate::model::SectionKind;

use crate::tui::app::App;

/// An entry in the recent view (top-level done task with subtask tree)
pub struct RecentEntry {
    pub track_id: String,
    pub id: String,
    pub title: String,
    pub resolved: String,
    pub track_name: String,
    pub task: crate::model::task::Task,
    /// Whether this entry is from an archive file (not reopenable)
    pub is_archived: bool,
}

/// Build the sorted list of recent (done) entries from all tracks' Done sections + archive files.
pub fn build_recent_entries(app: &App) -> Vec<RecentEntry> {
    let mut entries: Vec<RecentEntry> = Vec::new();

    for (track_id, track) in &app.project.tracks {
        let track_name = app.track_name(track_id).to_string();
        for task in track.section_tasks(SectionKind::Done) {
            let resolved = task
                .metadata
                .iter()
                .find_map(|m| {
                    if let crate::model::task::Metadata::Resolved(d) = m {
                        Some(d.clone())
                    } else {
                        None
                    }
                })
                .unwrap_or_default();
            entries.push(RecentEntry {
                track_id: track_id.clone(),
                id: task.id.clone().unwrap_or_default(),
                title: task.title.clone(),
                resolved,
                track_name: track_name.clone(),
                task: task.clone(),
                is_archived: false,
            });
        }
    }

    // Load archived tasks from frame/archive/{track_id}.md files
    let archive_dir = app.project.frame_dir.join("archive");
    if archive_dir.is_dir() {
        for tc in &app.project.config.tracks {
            let archive_path = archive_dir.join(format!("{}.md", tc.id));
            if let Ok(text) = std::fs::read_to_string(&archive_path) {
                let lines: Vec<String> = text.lines().map(String::from).collect();
                // Skip non-task preamble (e.g. "# Archive — {track_id}" header)
                let start = lines
                    .iter()
                    .position(|l| {
                        let t = l.trim_start();
                        t.starts_with("- [") && t.len() >= 5 && t.as_bytes()[4] == b']'
                    })
                    .unwrap_or(lines.len());
                let (tasks, _) = crate::parse::parse_tasks(&lines, start, 0, 0);
                let track_name = app.track_name(&tc.id).to_string();
                for task in tasks {
                    let resolved = task
                        .metadata
                        .iter()
                        .find_map(|m| {
                            if let crate::model::task::Metadata::Resolved(d) = m {
                                Some(d.clone())
                            } else {
                                None
                            }
                        })
                        .unwrap_or_default();
                    entries.push(RecentEntry {
                        track_id: tc.id.clone(),
                        id: task.id.clone().unwrap_or_default(),
                        title: task.title.clone(),
                        resolved,
                        track_name: track_name.clone(),
                        task,
                        is_archived: true,
                    });
                }
            }
        }
    }

    // Sort by resolved date, most recent first
    entries.sort_by(|a, b| b.resolved.cmp(&a.resolved));
    entries
}

// ---------------------------------------------------------------------------
// Detail view functions

/// Open detail view for the task under cursor in Recent view
pub(super) fn open_recent_detail(app: &mut App) {
    let entries = build_recent_entries(app);
    let cursor = app.recent_cursor;
    if let Some(entry) = entries.get(cursor) {
        if entry.id.is_empty() {
            app.status_message = Some("No task to view".to_string());
            return;
        }
        if entry.is_archived {
            app.status_message = Some("Cannot view archived task".to_string());
            return;
        }
        app.open_detail(entry.track_id.clone(), entry.id.clone());
    }
}

/// Expand a task's subtree in the Recent view
pub(super) fn expand_recent(app: &mut App) {
    let entries = build_recent_entries(app);
    let cursor = app.recent_cursor;
    if let Some(entry) = entries.get(cursor)
        && !entry.task.subtasks.is_empty()
    {
        app.recent_expanded.insert(entry.id.clone());
    }
}

/// Collapse a task's subtree in the Recent view
pub(super) fn collapse_recent(app: &mut App) {
    let entries = build_recent_entries(app);
    let cursor = app.recent_cursor;
    if let Some(entry) = entries.get(cursor) {
        app.recent_expanded.remove(&entry.id);
    }
}
