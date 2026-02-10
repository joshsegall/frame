mod init;
pub use init::cmd_init;

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Mutex;

use regex::Regex;

/// Global override for project directory (set by -C flag)
static PROJECT_DIR_OVERRIDE: Mutex<Option<PathBuf>> = Mutex::new(None);

use crate::cli::commands::*;
use crate::cli::output::*;
use crate::io::config_io;
use crate::io::lock::FileLock;
use crate::io::project_io::{self, ProjectError};
use crate::io::registry;
use crate::model::inbox::Inbox;
use crate::model::project::Project;
use crate::model::task::{Metadata, Task, TaskState};
use crate::model::track::{Track, TrackNode};
use crate::ops::{check, clean, import, inbox_ops, search, task_ops, track_ops};

// ---------------------------------------------------------------------------
// Dispatch
// ---------------------------------------------------------------------------

pub fn dispatch(cli: Cli) -> Result<(), Box<dyn std::error::Error>> {
    let json = cli.json;

    // Store -C override for load_project_cwd()
    if let Some(ref dir) = cli.project_dir {
        let abs = std::fs::canonicalize(dir)
            .map_err(|e| format!("cannot resolve -C path '{}': {}", dir, e))?;
        PROJECT_DIR_OVERRIDE.lock().unwrap().replace(abs);
    }

    match cli.command {
        None => {
            eprintln!("TUI not yet implemented. Use a subcommand (try `fr --help`).");
            Ok(())
        }
        Some(cmd) => match cmd {
            // Init is handled in main.rs before project discovery
            Commands::Init(args) => cmd_init(args),

            // Project registry (doesn't require a project context)
            Commands::Projects(args) => cmd_projects(args, json),

            // Read commands
            Commands::List(args) => cmd_list(args, json),
            Commands::Show(args) => cmd_show(args, json),
            Commands::Ready(args) => cmd_ready(args, json),
            Commands::Blocked => cmd_blocked(json),
            Commands::Search(args) => cmd_search(args),
            Commands::Inbox(args) => {
                if args.text.is_some() {
                    cmd_inbox_add(args)
                } else {
                    cmd_inbox_list(json)
                }
            }
            Commands::Tracks => cmd_tracks(json),
            Commands::Stats(args) => cmd_stats(args, json),
            Commands::Recent(args) => cmd_recent(args, json),
            Commands::Deps(args) => cmd_deps(args),
            Commands::Check => cmd_check(json),

            // Write commands
            Commands::Add(args) => cmd_add(args),
            Commands::Push(args) => cmd_push(args),
            Commands::Sub(args) => cmd_sub(args),
            Commands::State(args) => cmd_state(args),
            Commands::Tag(args) => cmd_tag(args),
            Commands::Dep(args) => cmd_dep(args),
            Commands::Note(args) => cmd_note(args),
            Commands::Ref(args) => cmd_ref(args),
            Commands::Spec(args) => cmd_spec(args),
            Commands::Title(args) => cmd_title(args),
            Commands::Mv(args) => cmd_mv(args),
            Commands::Triage(args) => cmd_triage(args),

            // Track management
            Commands::Track(args) => cmd_track(args),

            // Maintenance
            Commands::Clean(args) => cmd_clean(args),
            Commands::Import(args) => cmd_import(args),
        },
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn load_project_cwd() -> Result<Project, ProjectError> {
    let start = match PROJECT_DIR_OVERRIDE.lock().unwrap().as_ref() {
        Some(dir) => dir.clone(),
        None => std::env::current_dir().map_err(ProjectError::IoError)?,
    };
    let root = project_io::discover_project(&start)?;
    let project = project_io::load_project(&root)?;

    // Auto-register and touch CLI timestamp
    registry::register_project(&project.config.project.name, &project.root);
    registry::touch_cli(&project.root);

    Ok(project)
}

/// Find the track config and prefix for a given track ID.
fn track_prefix<'a>(project: &'a Project, track_id: &str) -> Option<&'a str> {
    project
        .config
        .ids
        .prefixes
        .get(track_id)
        .map(|s| s.as_str())
}

/// Find a mutable track reference by ID in the project.
fn find_track_mut<'a>(project: &'a mut Project, track_id: &str) -> Option<&'a mut Track> {
    project
        .tracks
        .iter_mut()
        .find(|(id, _)| id == track_id)
        .map(|(_, track)| track)
}

/// Find an immutable track reference by ID.
fn find_track<'a>(project: &'a Project, track_id: &str) -> Option<&'a Track> {
    project
        .tracks
        .iter()
        .find(|(id, _)| id == track_id)
        .map(|(_, track)| track)
}

/// Get the file path for a track from config.
fn track_file<'a>(project: &'a Project, track_id: &str) -> Option<&'a str> {
    project
        .config
        .tracks
        .iter()
        .find(|tc| tc.id == track_id)
        .map(|tc| tc.file.as_str())
}

/// Save a track back to disk.
fn save_track(project: &Project, track_id: &str) -> Result<(), ProjectError> {
    let file = track_file(project, track_id).ok_or(ProjectError::NotAProject)?;
    let track = find_track(project, track_id).ok_or(ProjectError::NotAProject)?;
    project_io::save_track(&project.frame_dir, file, track)
}

/// Check if a task has unresolved (non-done) deps
fn has_unresolved_deps(task: &Task, project: &Project) -> bool {
    for m in &task.metadata {
        if let Metadata::Dep(deps) = m {
            for dep_id in deps {
                // Find the dep task and check if it's done
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

/// Find which track a task ID belongs to
fn find_task_track<'a>(project: &'a Project, task_id: &str) -> Option<&'a str> {
    for (track_id, track) in &project.tracks {
        if task_ops::find_task_in_track(track, task_id).is_some() {
            return Some(track_id.as_str());
        }
    }
    None
}

/// Get all done tasks with resolved dates across all tracks, sorted by date (newest first)
fn collect_recent_tasks(project: &Project) -> Vec<(String, &Task)> {
    let mut tasks = Vec::new();
    for (track_id, track) in &project.tracks {
        for node in &track.nodes {
            if let TrackNode::Section {
                tasks: section_tasks,
                ..
            } = node
            {
                collect_done_tasks(section_tasks, track_id, &mut tasks);
            }
        }
    }
    // Sort by resolved date, newest first
    tasks.sort_by(|a, b| {
        let date_a = resolved_date(a.1);
        let date_b = resolved_date(b.1);
        date_b.cmp(&date_a)
    });
    tasks
}

fn collect_done_tasks<'a>(tasks: &'a [Task], track_id: &str, result: &mut Vec<(String, &'a Task)>) {
    for task in tasks {
        if task.state == TaskState::Done {
            result.push((track_id.to_string(), task));
        }
        collect_done_tasks(&task.subtasks, track_id, result);
    }
}

fn resolved_date(task: &Task) -> String {
    for m in &task.metadata {
        if let Metadata::Resolved(d) = m {
            return d.clone();
        }
    }
    String::new()
}

// ---------------------------------------------------------------------------
// Read command handlers
// ---------------------------------------------------------------------------

fn cmd_list(args: ListArgs, json: bool) -> Result<(), Box<dyn std::error::Error>> {
    let project = load_project_cwd()?;
    let state_filter = args
        .state
        .as_deref()
        .map(parse_task_state)
        .transpose()
        .map_err(Box::<dyn std::error::Error>::from)?;
    let tag_filter = args.tag.as_deref();

    if json {
        let mut results = Vec::new();
        for (track_id, track) in &project.tracks {
            if let Some(ref filter_track) = args.track {
                if track_id != filter_track {
                    continue;
                }
            } else if !args.all {
                let is_active = project
                    .config
                    .tracks
                    .iter()
                    .any(|tc| tc.id == *track_id && tc.state == "active");
                if !is_active {
                    continue;
                }
            }
            let tasks = collect_filtered_tasks(track, state_filter, tag_filter);
            results.push(TaskListJson {
                track: track_id.clone(),
                tasks: tasks.iter().map(|t| task_to_json(t)).collect(),
            });
        }
        println!("{}", serde_json::to_string_pretty(&results)?);
    } else {
        let mut first = true;
        for (track_id, track) in &project.tracks {
            if let Some(ref filter_track) = args.track {
                if track_id != filter_track {
                    continue;
                }
            } else if !args.all {
                let is_active = project
                    .config
                    .tracks
                    .iter()
                    .any(|tc| tc.id == *track_id && tc.state == "active");
                if !is_active {
                    continue;
                }
            }
            if !first {
                println!();
            }
            first = false;
            let lines = format_track_listing(track_id, track, state_filter, tag_filter);
            for line in &lines {
                println!("{}", line);
            }
        }
    }
    Ok(())
}

fn collect_filtered_tasks<'a>(
    track: &'a Track,
    state_filter: Option<TaskState>,
    tag_filter: Option<&str>,
) -> Vec<&'a Task> {
    let mut result = Vec::new();
    let backlog = track.backlog();
    let parked = track.parked();
    let done = track.done();

    let filter = |task: &&Task| -> bool {
        if let Some(sf) = state_filter
            && task.state != sf
        {
            return false;
        }
        if let Some(tf) = tag_filter
            && !task.tags.iter().any(|t| t == tf)
        {
            return false;
        }
        true
    };

    result.extend(backlog.iter().filter(filter));
    result.extend(parked.iter().filter(filter));
    // Include done tasks only if explicitly filtered for
    if state_filter == Some(TaskState::Done) {
        result.extend(done.iter().filter(filter));
    }

    result
}

fn cmd_show(args: ShowArgs, json: bool) -> Result<(), Box<dyn std::error::Error>> {
    let project = load_project_cwd()?;

    for (_, track) in &project.tracks {
        if let Some(task) = task_ops::find_task_in_track(track, &args.id) {
            if json {
                let mut tj = task_to_json(task);
                // JSON always includes ancestors
                tj.ancestors = collect_ancestor_ids(&args.id)
                    .iter()
                    .filter_map(|aid| task_ops::find_task_in_track(track, aid))
                    .map(task_to_json)
                    .collect();
                println!("{}", serde_json::to_string_pretty(&tj)?);
            } else if args.context {
                let ancestors: Vec<&Task> = collect_ancestor_ids(&args.id)
                    .iter()
                    .filter_map(|aid| task_ops::find_task_in_track(track, aid))
                    .collect();
                for line in format_task_detail_with_context(&ancestors, task) {
                    println!("{}", line);
                }
            } else {
                for line in format_task_detail(task) {
                    println!("{}", line);
                }
            }
            return Ok(());
        }
    }

    Err(format!("task not found: {}", args.id).into())
}

/// Collect ancestor task IDs from a dotted ID, root-first.
/// e.g. "FOO-001.1.2" → ["FOO-001", "FOO-001.1"]
fn collect_ancestor_ids(task_id: &str) -> Vec<String> {
    let mut ancestors = Vec::new();
    let mut id = task_id.to_string();
    while let Some(dot_pos) = id.rfind('.') {
        id = id[..dot_pos].to_string();
        ancestors.push(id.clone());
    }
    ancestors.reverse();
    ancestors
}

fn cmd_ready(args: ReadyArgs, json: bool) -> Result<(), Box<dyn std::error::Error>> {
    let project = load_project_cwd()?;
    let mut ready_tasks: Vec<(String, &Task)> = Vec::new();

    let target_tracks: Vec<&str> = if args.cc {
        // cc mode: only cc-focus track, only cc-tagged tasks
        match &project.config.agent.cc_focus {
            Some(focus) => vec![focus.as_str()],
            None => return Err("no cc-focus track configured".into()),
        }
    } else if let Some(ref track_id) = args.track {
        vec![track_id.as_str()]
    } else {
        // All active tracks
        project
            .config
            .tracks
            .iter()
            .filter(|tc| tc.state == "active")
            .map(|tc| tc.id.as_str())
            .collect()
    };

    for track_id in &target_tracks {
        if let Some(track) = find_track(&project, track_id) {
            let backlog = track.backlog();
            for task in backlog {
                collect_ready_tasks(task, track_id, &project, &args, &mut ready_tasks);
            }
        }
    }

    if json {
        let focus_track = if args.cc {
            project.config.agent.cc_focus.clone()
        } else {
            None
        };
        let output = ReadyJson {
            focus_track,
            tasks: ready_tasks
                .iter()
                .map(|(tid, task)| TaskWithTrackJson {
                    track: tid.clone(),
                    task: task_to_json(task),
                })
                .collect(),
        };
        println!("{}", serde_json::to_string_pretty(&output)?);
    } else {
        for (track_id, task) in &ready_tasks {
            let line = format_task_line(task);
            println!("[{}] {}", track_id, line);
        }
    }
    Ok(())
}

fn collect_ready_tasks<'a>(
    task: &'a Task,
    track_id: &str,
    project: &'a Project,
    args: &ReadyArgs,
    result: &mut Vec<(String, &'a Task)>,
) {
    if task.state == TaskState::Todo && !has_unresolved_deps(task, project) {
        let mut include = true;
        if args.cc && !task.tags.iter().any(|t| t == "cc") {
            include = false;
        }
        if let Some(ref tag) = args.tag
            && !task.tags.iter().any(|t| t == tag)
        {
            include = false;
        }
        if include {
            result.push((track_id.to_string(), task));
        }
    }
    // Also check subtasks
    for sub in &task.subtasks {
        collect_ready_tasks(sub, track_id, project, args, result);
    }
}

fn cmd_blocked(json: bool) -> Result<(), Box<dyn std::error::Error>> {
    let project = load_project_cwd()?;
    let mut blocked_tasks: Vec<(String, &Task)> = Vec::new();

    for (track_id, track) in &project.tracks {
        let is_active = project
            .config
            .tracks
            .iter()
            .any(|tc| tc.id == *track_id && tc.state == "active");
        if !is_active {
            continue;
        }
        for node in &track.nodes {
            if let TrackNode::Section { tasks, .. } = node {
                for task in tasks {
                    collect_blocked_tasks(task, track_id, &mut blocked_tasks);
                }
            }
        }
    }

    if json {
        let output: Vec<TaskWithTrackJson> = blocked_tasks
            .iter()
            .map(|(tid, task)| TaskWithTrackJson {
                track: tid.clone(),
                task: task_to_json(task),
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&output)?);
    } else {
        for (track_id, task) in &blocked_tasks {
            let line = format_task_line(task);
            let deps = task_deps(task);
            if deps.is_empty() {
                println!("[{}] {}", track_id, line);
            } else {
                println!("[{}] {} (blocked by: {})", track_id, line, deps.join(", "));
            }
        }
    }
    Ok(())
}

fn collect_blocked_tasks<'a>(task: &'a Task, track_id: &str, result: &mut Vec<(String, &'a Task)>) {
    if task.state == TaskState::Blocked {
        result.push((track_id.to_string(), task));
    }
    for sub in &task.subtasks {
        collect_blocked_tasks(sub, track_id, result);
    }
}

fn task_deps(task: &Task) -> Vec<String> {
    let mut deps = Vec::new();
    for m in &task.metadata {
        if let Metadata::Dep(d) = m {
            deps.extend(d.iter().cloned());
        }
    }
    deps
}

fn cmd_search(args: SearchArgs) -> Result<(), Box<dyn std::error::Error>> {
    let project = load_project_cwd()?;
    let re = Regex::new(&args.pattern)?;
    let hits = search::search_tasks(&project, &re, args.track.as_deref());

    // Deduplicate by task_id (multiple field matches for same task)
    let mut seen = HashSet::new();
    for hit in &hits {
        if seen.insert((&hit.track_id, &hit.task_id)) {
            // Find the task to get its title
            if let Some(track) = find_track(&project, &hit.track_id) {
                if let Some(task) = task_ops::find_task_in_track(track, &hit.task_id) {
                    let line = format_task_line(task);
                    println!("[{}] {}", hit.track_id, line);
                } else {
                    println!(
                        "[{}] {} (in {})",
                        hit.track_id,
                        hit.task_id,
                        hit.field_name()
                    );
                }
            }
        }
    }

    // Search inbox too if no track filter
    if args.track.is_none()
        && let Some(ref inbox) = project.inbox
    {
        let inbox_hits = search::search_inbox(inbox, &re);
        let mut seen_items = HashSet::new();
        for hit in &inbox_hits {
            if seen_items.insert(hit.item_index)
                && let Some(item) = inbox.items.get(hit.item_index)
            {
                let tags = if item.tags.is_empty() {
                    String::new()
                } else {
                    format!(
                        " {}",
                        item.tags
                            .iter()
                            .map(|t| format!("#{}", t))
                            .collect::<Vec<_>>()
                            .join(" ")
                    )
                };
                println!("[inbox:{}] {}{}", hit.item_index + 1, item.title, tags);
            }
        }
    }

    Ok(())
}

/// Extension trait to get field name for search hits
trait FieldName {
    fn field_name(&self) -> &'static str;
}

impl FieldName for search::SearchHit {
    fn field_name(&self) -> &'static str {
        match self.field {
            search::MatchField::Id => "id",
            search::MatchField::Title => "title",
            search::MatchField::Tag => "tag",
            search::MatchField::Note => "note",
            search::MatchField::Dep => "dep",
            search::MatchField::Ref => "ref",
            search::MatchField::Spec => "spec",
            search::MatchField::Body => "body",
        }
    }
}

fn cmd_inbox_list(json: bool) -> Result<(), Box<dyn std::error::Error>> {
    let project = load_project_cwd()?;
    let inbox = project.inbox.as_ref().ok_or("no inbox.md found")?;

    if json {
        let items: Vec<InboxItemJson> = inbox
            .items
            .iter()
            .enumerate()
            .map(|(i, item)| InboxItemJson {
                index: i + 1,
                title: item.title.clone(),
                tags: item.tags.clone(),
                body: item.body.clone(),
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&items)?);
    } else {
        if inbox.items.is_empty() {
            println!("(inbox is empty)");
        }
        for (i, item) in inbox.items.iter().enumerate() {
            let tags = if item.tags.is_empty() {
                String::new()
            } else {
                format!(
                    " {}",
                    item.tags
                        .iter()
                        .map(|t| format!("#{}", t))
                        .collect::<Vec<_>>()
                        .join(" ")
                )
            };
            println!("{:>3}  {}{}", i + 1, item.title, tags);
            if let Some(ref body) = item.body {
                for line in body.lines() {
                    println!("     {}", line);
                }
            }
        }
    }
    Ok(())
}

fn cmd_tracks(json: bool) -> Result<(), Box<dyn std::error::Error>> {
    let project = load_project_cwd()?;

    if json {
        let mut infos = Vec::new();
        for tc in &project.config.tracks {
            let stats = find_track(&project, &tc.id)
                .map(track_ops::task_counts)
                .unwrap_or_default();
            let is_cc = project.config.agent.cc_focus.as_deref() == Some(&tc.id);
            infos.push(TrackInfoJson {
                id: tc.id.clone(),
                name: tc.name.clone(),
                state: tc.state.clone(),
                cc_focus: if is_cc { Some(true) } else { None },
                stats: stats_to_json(&stats),
            });
        }
        println!("{}", serde_json::to_string_pretty(&infos)?);
    } else {
        // Gather entries grouped by state
        let mut active_entries = Vec::new();
        let mut shelved_entries = Vec::new();
        let mut archived_entries = Vec::new();

        for tc in &project.config.tracks {
            let prefix = project
                .config
                .ids
                .prefixes
                .get(&tc.id)
                .cloned()
                .unwrap_or_default();
            let is_cc = project.config.agent.cc_focus.as_deref() == Some(&tc.id);
            let entry = (
                tc.id.clone(),
                tc.name.clone(),
                prefix,
                tc.file.clone(),
                is_cc,
            );
            match tc.state.as_str() {
                "active" => active_entries.push(entry),
                "shelved" => shelved_entries.push(entry),
                _ => archived_entries.push(entry),
            }
        }

        // Compute column widths across all entries
        let all_entries: Vec<_> = active_entries
            .iter()
            .chain(shelved_entries.iter())
            .chain(archived_entries.iter())
            .collect();
        let name_w = all_entries
            .iter()
            .map(|(_, name, _, _, _)| name.len())
            .max()
            .unwrap_or(0)
            .max(4); // "name"
        let id_w = all_entries
            .iter()
            .map(|(id, _, _, _, _)| id.len())
            .max()
            .unwrap_or(0)
            .max(2); // "id"
        let pfx_w = all_entries
            .iter()
            .map(|(_, _, pfx, _, _)| pfx.len())
            .max()
            .unwrap_or(0)
            .max(3); // "pfx"
        let file_w = all_entries
            .iter()
            .map(|(_, _, _, file, _)| file.len())
            .max()
            .unwrap_or(0)
            .max(4); // "file"

        let print_header = |label: &str| {
            println!(
                " {:<name_w$}  {:<id_w$}  {:<pfx_w$}  {:<file_w$}",
                label,
                "id",
                "pfx",
                "file",
                name_w = name_w,
                id_w = id_w,
                pfx_w = pfx_w,
                file_w = file_w,
            );
        };

        let print_row = |name: &str, id: &str, pfx: &str, file: &str, is_cc: bool| {
            let cc_str = if is_cc { "  cc" } else { "" };
            println!(
                " {:<name_w$}  {:<id_w$}  {:<pfx_w$}  {:<file_w$}{}",
                name,
                id,
                pfx,
                file,
                cc_str,
                name_w = name_w,
                id_w = id_w,
                pfx_w = pfx_w,
                file_w = file_w,
            );
        };

        if !active_entries.is_empty() {
            print_header("Active");
            for (id, name, pfx, file, is_cc) in &active_entries {
                print_row(name, id, pfx, file, *is_cc);
            }
        }

        if !shelved_entries.is_empty() {
            if !active_entries.is_empty() {
                println!();
            }
            print_header("Shelved");
            for (id, name, pfx, file, is_cc) in &shelved_entries {
                print_row(name, id, pfx, file, *is_cc);
            }
        }

        if !archived_entries.is_empty() {
            if !active_entries.is_empty() || !shelved_entries.is_empty() {
                println!();
            }
            print_header("Archived");
            for (id, name, pfx, file, is_cc) in &archived_entries {
                print_row(name, id, pfx, file, *is_cc);
            }
        }
    }
    Ok(())
}

fn cmd_stats(args: StatsArgs, json: bool) -> Result<(), Box<dyn std::error::Error>> {
    let project = load_project_cwd()?;
    let mut active_entries = Vec::new();
    let mut shelved_entries = Vec::new();
    let mut totals = track_ops::TrackStats::default();

    for tc in &project.config.tracks {
        let is_active = tc.state == "active";
        if !is_active && !args.all {
            continue;
        }
        let stats = find_track(&project, &tc.id)
            .map(track_ops::task_counts)
            .unwrap_or_default();
        let prefix = project
            .config
            .ids
            .prefixes
            .get(&tc.id)
            .cloned()
            .unwrap_or_default();

        totals.active += stats.active;
        totals.blocked += stats.blocked;
        totals.todo += stats.todo;
        totals.parked += stats.parked;
        totals.done += stats.done;

        let entry = (tc.id.clone(), tc.name.clone(), prefix, stats);
        if is_active {
            active_entries.push(entry);
        } else {
            shelved_entries.push(entry);
        }
    }

    if json {
        let all_entries: Vec<_> = active_entries
            .iter()
            .chain(shelved_entries.iter())
            .collect();
        let output = StatsJson {
            tracks: all_entries
                .iter()
                .map(|(id, name, _, stats)| TrackStatsEntryJson {
                    id: id.clone(),
                    name: name.clone(),
                    stats: stats_to_json(stats),
                })
                .collect(),
            totals: stats_to_json(&totals),
        };
        println!("{}", serde_json::to_string_pretty(&output)?);
    } else {
        // Compute column widths across all entries
        let all_entries: Vec<_> = active_entries
            .iter()
            .chain(shelved_entries.iter())
            .collect();
        let name_w = all_entries
            .iter()
            .map(|(_, name, _, _)| name.len())
            .max()
            .unwrap_or(0)
            .max(5); // "Total"
        let pfx_w = all_entries
            .iter()
            .map(|(_, _, pfx, _)| pfx.len())
            .max()
            .unwrap_or(0)
            .max(3); // "pfx"

        let print_header = |label: &str| {
            println!(
                " {:<name_w$}  {:<pfx_w$}  {:>4}  {:>4}  {:>4}  {:>4}  {:>4}",
                label,
                "pfx",
                "[ ]",
                "[>]",
                "[-]",
                "[x]",
                "[~]",
                name_w = name_w,
                pfx_w = pfx_w,
            );
        };

        let print_row = |name: &str, pfx: &str, stats: &track_ops::TrackStats| {
            println!(
                " {:<name_w$}  {:<pfx_w$}  {:>4}  {:>4}  {:>4}  {:>4}  {:>4}",
                name,
                pfx,
                stats.todo,
                stats.active,
                stats.blocked,
                stats.done,
                stats.parked,
                name_w = name_w,
                pfx_w = pfx_w,
            );
        };

        if !active_entries.is_empty() {
            print_header("Active");
            for (_, name, pfx, stats) in &active_entries {
                print_row(name, pfx, stats);
            }
        }

        if !shelved_entries.is_empty() {
            if !active_entries.is_empty() {
                println!();
            }
            print_header("Shelved");
            for (_, name, pfx, stats) in &shelved_entries {
                print_row(name, pfx, stats);
            }
        }

        println!();
        println!(
            " {:<name_w$}  {:<pfx_w$}  {:>4}  {:>4}  {:>4}  {:>4}  {:>4}",
            "Total",
            "",
            totals.todo,
            totals.active,
            totals.blocked,
            totals.done,
            totals.parked,
            name_w = name_w,
            pfx_w = pfx_w,
        );
    }
    Ok(())
}

fn cmd_recent(args: RecentArgs, json: bool) -> Result<(), Box<dyn std::error::Error>> {
    let project = load_project_cwd()?;
    let recent = collect_recent_tasks(&project);
    let limited: Vec<_> = recent.into_iter().take(args.limit).collect();

    if json {
        let items: Vec<TaskWithTrackJson> = limited
            .iter()
            .map(|(tid, task)| TaskWithTrackJson {
                track: tid.clone(),
                task: task_to_json(task),
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&items)?);
    } else {
        let mut current_date = String::new();
        for (track_id, task) in &limited {
            let date = resolved_date(task);
            if date != current_date {
                if !current_date.is_empty() {
                    println!();
                }
                println!("{}", date);
                current_date = date;
            }
            let id_str = task.id.as_deref().unwrap_or("???");
            println!(
                "  [{}] {} {} ({})",
                task.state.checkbox_char(),
                id_str,
                task.title,
                track_id
            );
        }
    }
    Ok(())
}

fn cmd_deps(args: DepsArgs) -> Result<(), Box<dyn std::error::Error>> {
    let project = load_project_cwd()?;

    // Find the task
    let mut found_task = None;
    for (_, track) in &project.tracks {
        if let Some(task) = task_ops::find_task_in_track(track, &args.id) {
            found_task = Some(task);
            break;
        }
    }

    let task = found_task.ok_or_else(|| format!("task not found: {}", args.id))?;
    println!("{}", format_task_line(task));

    // Print dependency tree
    let deps = task_deps(task);
    if deps.is_empty() {
        println!("  (no dependencies)");
    } else {
        print_dep_tree(&project, &deps, 1, &mut HashSet::new());
    }
    Ok(())
}

fn print_dep_tree(
    project: &Project,
    dep_ids: &[String],
    indent: usize,
    visited: &mut HashSet<String>,
) {
    let prefix = "  ".repeat(indent);
    for dep_id in dep_ids {
        if !visited.insert(dep_id.clone()) {
            println!("{}└─ {} (circular)", prefix, dep_id);
            continue;
        }
        let mut found = false;
        for (_, track) in &project.tracks {
            if let Some(dep_task) = task_ops::find_task_in_track(track, dep_id) {
                let sc = dep_task.state.checkbox_char();
                println!("{}└─ [{}] {} {}", prefix, sc, dep_id, dep_task.title);
                let sub_deps = task_deps(dep_task);
                if !sub_deps.is_empty() {
                    print_dep_tree(project, &sub_deps, indent + 1, visited);
                }
                found = true;
                break;
            }
        }
        if !found {
            println!("{}└─ {} (not found)", prefix, dep_id);
        }
    }
}

fn cmd_check(json: bool) -> Result<(), Box<dyn std::error::Error>> {
    let project = load_project_cwd()?;
    let result = check::check_project(&project);

    if json {
        println!("{}", serde_json::to_string_pretty(&result)?);
    } else {
        if !result.errors.is_empty() {
            println!("Errors:");
            for err in &result.errors {
                match err {
                    check::CheckError::DanglingDep {
                        track_id,
                        task_id,
                        dep_id,
                    } => {
                        println!("  [{}] {} has dangling dep: {}", track_id, task_id, dep_id);
                    }
                    check::CheckError::BrokenRef {
                        track_id,
                        task_id,
                        path,
                    } => {
                        println!("  [{}] {} has broken ref: {}", track_id, task_id, path);
                    }
                    check::CheckError::BrokenSpec {
                        track_id,
                        task_id,
                        path,
                    } => {
                        println!("  [{}] {} has broken spec: {}", track_id, task_id, path);
                    }
                    check::CheckError::DuplicateId { task_id, track_ids } => {
                        println!(
                            "  {} is duplicated in tracks: {}",
                            task_id,
                            track_ids.join(", ")
                        );
                    }
                }
            }
        }
        if !result.warnings.is_empty() {
            if !result.errors.is_empty() {
                println!();
            }
            println!("Warnings:");
            for warn in &result.warnings {
                match warn {
                    check::CheckWarning::MissingId { track_id, title } => {
                        println!("  [{}] task missing ID: \"{}\"", track_id, title);
                    }
                    check::CheckWarning::MissingAddedDate { track_id, task_id } => {
                        println!("  [{}] {} missing added date", track_id, task_id);
                    }
                    check::CheckWarning::MissingResolvedDate { track_id, task_id } => {
                        println!("  [{}] {} (done) missing resolved date", track_id, task_id);
                    }
                    check::CheckWarning::DoneInBacklog { track_id, task_id } => {
                        println!(
                            "  [{}] {} is done but in backlog section",
                            track_id, task_id
                        );
                    }
                }
            }
        }
        if result.valid {
            println!("✓ project is valid");
        } else {
            println!("✗ project has errors");
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Write command handlers
// ---------------------------------------------------------------------------

fn cmd_add(args: AddArgs) -> Result<(), Box<dyn std::error::Error>> {
    let mut project = load_project_cwd()?;
    let _lock = FileLock::acquire_default(&project.frame_dir)?;

    let prefix = track_prefix(&project, &args.track)
        .ok_or_else(|| format!("no ID prefix configured for track '{}'", args.track))?
        .to_string();

    let position = if let Some(ref after_id) = args.after {
        task_ops::InsertPosition::After(after_id.clone())
    } else {
        task_ops::InsertPosition::Bottom
    };

    let track = find_track_mut(&mut project, &args.track)
        .ok_or_else(|| format!("track not found: {}", args.track))?;

    let id = task_ops::add_task(track, args.title.clone(), position, &prefix)?;

    // If --found-from, add a note
    if let Some(ref from_id) = args.found_from {
        task_ops::set_note(track, &id, format!("Found while working on {}", from_id))?;
    }

    save_track(&project, &args.track)?;
    println!("{}", id);
    Ok(())
}

fn cmd_push(args: PushArgs) -> Result<(), Box<dyn std::error::Error>> {
    let mut project = load_project_cwd()?;
    let _lock = FileLock::acquire_default(&project.frame_dir)?;

    let prefix = track_prefix(&project, &args.track)
        .ok_or_else(|| format!("no ID prefix configured for track '{}'", args.track))?
        .to_string();

    let track = find_track_mut(&mut project, &args.track)
        .ok_or_else(|| format!("track not found: {}", args.track))?;

    let id = task_ops::add_task(
        track,
        args.title.clone(),
        task_ops::InsertPosition::Top,
        &prefix,
    )?;

    save_track(&project, &args.track)?;
    println!("{}", id);
    Ok(())
}

fn cmd_sub(args: SubArgs) -> Result<(), Box<dyn std::error::Error>> {
    let mut project = load_project_cwd()?;
    let _lock = FileLock::acquire_default(&project.frame_dir)?;

    // Find which track the parent task is in
    let track_id = find_task_track(&project, &args.id)
        .ok_or_else(|| format!("task not found: {}", args.id))?
        .to_string();

    let track = find_track_mut(&mut project, &track_id)
        .ok_or_else(|| format!("track not found: {}", track_id))?;

    let sub_id = task_ops::add_subtask(track, &args.id, args.title)?;

    save_track(&project, &track_id)?;
    println!("{}", sub_id);
    Ok(())
}

fn cmd_inbox_add(args: InboxCmd) -> Result<(), Box<dyn std::error::Error>> {
    let mut project = load_project_cwd()?;
    let _lock = FileLock::acquire_default(&project.frame_dir)?;

    let text = args.text.unwrap(); // We know it's Some from dispatch
    let inbox = project.inbox.get_or_insert_with(|| Inbox {
        header_lines: vec!["# Inbox".to_string(), String::new()],
        items: Vec::new(),
        source_lines: vec!["# Inbox".to_string(), String::new()],
    });

    inbox_ops::add_inbox_item(inbox, text.clone(), args.tag, args.note);

    project_io::save_inbox(&project.frame_dir, inbox)?;
    println!("added to inbox");
    Ok(())
}

fn cmd_state(args: StateArgs) -> Result<(), Box<dyn std::error::Error>> {
    use crate::model::track::SectionKind;

    let mut project = load_project_cwd()?;
    let _lock = FileLock::acquire_default(&project.frame_dir)?;

    let new_state = parse_task_state(&args.state).map_err(Box::<dyn std::error::Error>::from)?;

    let track_id = find_task_track(&project, &args.id)
        .ok_or_else(|| format!("task not found: {}", args.id))?
        .to_string();

    let track = find_track_mut(&mut project, &track_id)
        .ok_or_else(|| format!("track not found: {}", track_id))?;

    let task = task_ops::find_task_mut_in_track(track, &args.id)
        .ok_or_else(|| format!("task not found: {}", args.id))?;

    task_ops::set_state(task, new_state);

    // If setting to Done and task is a top-level Backlog task, move to Done section immediately
    if new_state == TaskState::Done {
        let track = find_track_mut(&mut project, &track_id)
            .ok_or_else(|| format!("track not found: {}", track_id))?;
        if task_ops::is_top_level_in_section(track, &args.id, SectionKind::Backlog) {
            task_ops::move_task_between_sections(
                track,
                &args.id,
                SectionKind::Backlog,
                SectionKind::Done,
            );
        }
    }

    save_track(&project, &track_id)?;
    println!("{} → {}", args.id, args.state);
    Ok(())
}

fn cmd_tag(args: TagArgs) -> Result<(), Box<dyn std::error::Error>> {
    let mut project = load_project_cwd()?;
    let _lock = FileLock::acquire_default(&project.frame_dir)?;

    let track_id = find_task_track(&project, &args.id)
        .ok_or_else(|| format!("task not found: {}", args.id))?
        .to_string();

    let track = find_track_mut(&mut project, &track_id)
        .ok_or_else(|| format!("track not found: {}", track_id))?;

    match args.action.as_str() {
        "add" => task_ops::add_tag(track, &args.id, &args.tag)?,
        "rm" => task_ops::remove_tag(track, &args.id, &args.tag)?,
        other => return Err(format!("unknown action '{}' (expected: add, rm)", other).into()),
    }

    save_track(&project, &track_id)?;
    println!("{} tag {} {}", args.id, args.action, args.tag);
    Ok(())
}

fn cmd_dep(args: DepArgs) -> Result<(), Box<dyn std::error::Error>> {
    let mut project = load_project_cwd()?;
    let _lock = FileLock::acquire_default(&project.frame_dir)?;

    let track_id = find_task_track(&project, &args.id)
        .ok_or_else(|| format!("task not found: {}", args.id))?
        .to_string();

    match args.action.as_str() {
        "add" => {
            let all_tracks_snapshot: Vec<_> = project.tracks.clone();
            let track = find_track_mut(&mut project, &track_id)
                .ok_or_else(|| format!("track not found: {}", track_id))?;
            task_ops::add_dep(track, &args.id, &args.dep_id, &all_tracks_snapshot)?;
        }
        "rm" => {
            let track = find_track_mut(&mut project, &track_id)
                .ok_or_else(|| format!("track not found: {}", track_id))?;
            task_ops::remove_dep(track, &args.id, &args.dep_id)?;
        }
        other => return Err(format!("unknown action '{}' (expected: add, rm)", other).into()),
    }

    save_track(&project, &track_id)?;
    println!("{} dep {} {}", args.id, args.action, args.dep_id);
    Ok(())
}

fn cmd_note(args: NoteArgs) -> Result<(), Box<dyn std::error::Error>> {
    let mut project = load_project_cwd()?;
    let _lock = FileLock::acquire_default(&project.frame_dir)?;

    let track_id = find_task_track(&project, &args.id)
        .ok_or_else(|| format!("task not found: {}", args.id))?
        .to_string();

    let track = find_track_mut(&mut project, &track_id)
        .ok_or_else(|| format!("track not found: {}", track_id))?;

    task_ops::set_note(track, &args.id, args.text)?;

    save_track(&project, &track_id)?;
    println!("{} note updated", args.id);
    Ok(())
}

fn cmd_ref(args: RefArgs) -> Result<(), Box<dyn std::error::Error>> {
    let mut project = load_project_cwd()?;
    let _lock = FileLock::acquire_default(&project.frame_dir)?;

    let track_id = find_task_track(&project, &args.id)
        .ok_or_else(|| format!("task not found: {}", args.id))?
        .to_string();

    let track = find_track_mut(&mut project, &track_id)
        .ok_or_else(|| format!("track not found: {}", track_id))?;

    task_ops::add_ref(track, &args.id, &args.path)?;

    save_track(&project, &track_id)?;
    println!("{} ref added: {}", args.id, args.path);
    Ok(())
}

fn cmd_spec(args: SpecArgs) -> Result<(), Box<dyn std::error::Error>> {
    let mut project = load_project_cwd()?;
    let _lock = FileLock::acquire_default(&project.frame_dir)?;

    let track_id = find_task_track(&project, &args.id)
        .ok_or_else(|| format!("task not found: {}", args.id))?
        .to_string();

    let track = find_track_mut(&mut project, &track_id)
        .ok_or_else(|| format!("track not found: {}", track_id))?;

    task_ops::set_spec(track, &args.id, args.path.clone())?;

    save_track(&project, &track_id)?;
    println!("{} spec set: {}", args.id, args.path);
    Ok(())
}

fn cmd_title(args: TitleArgs) -> Result<(), Box<dyn std::error::Error>> {
    let mut project = load_project_cwd()?;
    let _lock = FileLock::acquire_default(&project.frame_dir)?;

    let track_id = find_task_track(&project, &args.id)
        .ok_or_else(|| format!("task not found: {}", args.id))?
        .to_string();

    let track = find_track_mut(&mut project, &track_id)
        .ok_or_else(|| format!("track not found: {}", track_id))?;

    task_ops::edit_title(track, &args.id, args.title.clone())?;

    save_track(&project, &track_id)?;
    println!("{} title updated", args.id);
    Ok(())
}

fn cmd_mv(args: MvArgs) -> Result<(), Box<dyn std::error::Error>> {
    let mut project = load_project_cwd()?;
    let _lock = FileLock::acquire_default(&project.frame_dir)?;

    // Validate flag conflicts
    if args.promote && args.parent.is_some() {
        return Err("--promote and --parent are conflicting flags".into());
    }

    let source_track_id = find_task_track(&project, &args.id)
        .ok_or_else(|| format!("task not found: {}", args.id))?
        .to_string();

    // Handle --promote
    if args.promote {
        let prefix = track_prefix(&project, &source_track_id)
            .ok_or_else(|| format!("no ID prefix configured for track '{}'", source_track_id))?
            .to_string();

        let track_idx = project
            .tracks
            .iter()
            .position(|(id, _)| id == &source_track_id)
            .ok_or_else(|| format!("track not found: {}", source_track_id))?;

        // Verify the task is not already top-level
        let location =
            task_ops::find_task_location_any_section(&project.tracks[track_idx].1, &args.id)
                .ok_or_else(|| format!("task not found: {}", args.id))?;
        if location.parent_id.is_none() {
            return Err("task is already top-level".into());
        }

        // Determine placement: after the former parent, or use --top/--after/position
        let sibling_index = if args.top {
            0
        } else if let Some(ref after_id) = args.after {
            let backlog = project.tracks[track_idx].1.backlog();
            backlog
                .iter()
                .position(|t| t.id.as_deref() == Some(after_id.as_str()))
                .map(|i| i + 1)
                .ok_or_else(|| format!("after target not found: {}", after_id))?
        } else {
            // Default: insert after the former parent
            let parent_id = location.parent_id.as_ref().unwrap();
            let parent_loc =
                task_ops::find_task_location_any_section(&project.tracks[track_idx].1, parent_id)
                    .ok_or_else(|| format!("parent not found: {}", parent_id))?;
            parent_loc.sibling_index + 1
        };

        // Split tracks to get mutable track + other tracks for dep updates
        let (left, right) = project.tracks.split_at_mut(track_idx);
        let (track_entry, rest) = right.split_first_mut().unwrap();
        let mut other_tracks: Vec<(String, Track)> =
            left.iter().map(|(id, t)| (id.clone(), t.clone())).collect();
        other_tracks.extend(rest.iter().map(|(id, t)| (id.clone(), t.clone())));

        let result = task_ops::reparent_task(
            &mut track_entry.1,
            &args.id,
            None,
            sibling_index,
            &prefix,
            &mut other_tracks,
        )?;

        save_track(&project, &source_track_id)?;
        println!("{} → {} (promoted)", args.id, result.new_root_id);
        return Ok(());
    }

    // Handle --parent
    if let Some(ref parent_id) = args.parent {
        let prefix = track_prefix(&project, &source_track_id)
            .ok_or_else(|| format!("no ID prefix configured for track '{}'", source_track_id))?
            .to_string();

        let track_idx = project
            .tracks
            .iter()
            .position(|(id, _)| id == &source_track_id)
            .ok_or_else(|| format!("track not found: {}", source_track_id))?;

        // Split tracks to get mutable track + other tracks for dep updates
        let (left, right) = project.tracks.split_at_mut(track_idx);
        let (track_entry, rest) = right.split_first_mut().unwrap();
        let mut other_tracks: Vec<(String, Track)> =
            left.iter().map(|(id, t)| (id.clone(), t.clone())).collect();
        other_tracks.extend(rest.iter().map(|(id, t)| (id.clone(), t.clone())));

        let result = task_ops::reparent_task(
            &mut track_entry.1,
            &args.id,
            Some(parent_id),
            usize::MAX,
            &prefix,
            &mut other_tracks,
        )?;

        save_track(&project, &source_track_id)?;
        println!("{} → {} (under {})", args.id, result.new_root_id, parent_id);
        return Ok(());
    }

    if let Some(ref target_track_id) = args.track {
        // Cross-track move
        let target_prefix = track_prefix(&project, target_track_id)
            .ok_or_else(|| format!("no ID prefix configured for track '{}'", target_track_id))?
            .to_string();

        // Get mutable references to both tracks
        let (source_idx, target_idx) = {
            let si = project
                .tracks
                .iter()
                .position(|(id, _)| id == &source_track_id)
                .ok_or_else(|| format!("track not found: {}", source_track_id))?;
            let ti = project
                .tracks
                .iter()
                .position(|(id, _)| id == target_track_id)
                .ok_or_else(|| format!("track not found: {}", target_track_id))?;
            (si, ti)
        };

        let position = if args.top {
            task_ops::InsertPosition::Top
        } else if let Some(ref after_id) = args.after {
            task_ops::InsertPosition::After(after_id.clone())
        } else {
            task_ops::InsertPosition::Bottom
        };

        // We need to split the tracks to get two mutable references
        let (left, right) = if source_idx < target_idx {
            let (left, right) = project.tracks.split_at_mut(target_idx);
            (&mut left[source_idx].1, &mut right[0].1)
        } else {
            let (left, right) = project.tracks.split_at_mut(source_idx);
            (&mut right[0].1, &mut left[target_idx].1)
        };

        let (source_track, target_track) = if source_idx < target_idx {
            (left, right)
        } else {
            (right, left)
        };

        let new_id = task_ops::move_task_to_track(
            source_track,
            target_track,
            &args.id,
            position,
            &target_prefix,
            &mut [], // We don't update deps in other tracks for simplicity
        )?;

        // Save both tracks
        save_track(&project, &source_track_id)?;
        save_track(&project, target_track_id)?;
        println!("{} → {} ({})", args.id, new_id, target_track_id);
    } else {
        // Same-track reorder
        let position = if args.top {
            task_ops::InsertPosition::Top
        } else if let Some(ref after_id) = args.after {
            task_ops::InsertPosition::After(after_id.clone())
        } else if let Some(pos) = args.position {
            // Numeric position: convert to After or Top
            let track = find_track(&project, &source_track_id)
                .ok_or_else(|| format!("track not found: {}", source_track_id))?;
            let backlog = track.backlog();
            if pos == 0 {
                task_ops::InsertPosition::Top
            } else if pos >= backlog.len() {
                task_ops::InsertPosition::Bottom
            } else {
                // Insert after the task currently at position pos-1 (skipping self)
                let mut target_idx = pos;
                if let Some(self_pos) = backlog
                    .iter()
                    .position(|t| t.id.as_deref() == Some(&args.id))
                    && self_pos < pos
                {
                    // Moving down: the task at the target position shifts up after removal
                    // So we actually want to be after the task currently at `pos`
                    target_idx = pos;
                }
                if target_idx < backlog.len() {
                    if let Some(ref after_task_id) = backlog[target_idx].id {
                        task_ops::InsertPosition::After(after_task_id.clone())
                    } else {
                        task_ops::InsertPosition::Bottom
                    }
                } else {
                    task_ops::InsertPosition::Bottom
                }
            }
        } else {
            return Err(
                "specify --top, --after <id>, --track <track>, or a numeric position".into(),
            );
        };

        let track = find_track_mut(&mut project, &source_track_id)
            .ok_or_else(|| format!("track not found: {}", source_track_id))?;

        task_ops::move_task(track, &args.id, position)?;
        save_track(&project, &source_track_id)?;
        println!("{} moved", args.id);
    }

    Ok(())
}

fn cmd_triage(args: TriageArgs) -> Result<(), Box<dyn std::error::Error>> {
    let mut project = load_project_cwd()?;
    let _lock = FileLock::acquire_default(&project.frame_dir)?;

    let prefix = track_prefix(&project, &args.track)
        .ok_or_else(|| format!("no ID prefix configured for track '{}'", args.track))?
        .to_string();

    let position = if args.top {
        task_ops::InsertPosition::Top
    } else if let Some(ref after_id) = args.after {
        task_ops::InsertPosition::After(after_id.clone())
    } else {
        task_ops::InsertPosition::Bottom
    };

    // Convert 1-based index to 0-based
    let index = args.index.checked_sub(1).ok_or("index must be >= 1")?;

    // Find track index to avoid double mutable borrow
    let track_idx = project
        .tracks
        .iter()
        .position(|(id, _)| id == &args.track)
        .ok_or_else(|| format!("track not found: {}", args.track))?;

    let inbox = project.inbox.as_mut().ok_or("no inbox.md found")?;
    let track = &mut project.tracks[track_idx].1;

    let task_id = inbox_ops::triage(inbox, index, track, position, &prefix)?;

    // Save both inbox and track
    if let Some(ref inbox) = project.inbox {
        project_io::save_inbox(&project.frame_dir, inbox)?;
    }
    save_track(&project, &args.track)?;
    println!("{}", task_id);
    Ok(())
}

// ---------------------------------------------------------------------------
// Track management handlers
// ---------------------------------------------------------------------------

fn cmd_track(args: TrackCmd) -> Result<(), Box<dyn std::error::Error>> {
    match args.action {
        TrackAction::New(a) => cmd_track_new(a),
        TrackAction::Shelve(a) => cmd_track_state_change(a.id, "shelve"),
        TrackAction::Activate(a) => cmd_track_state_change(a.id, "activate"),
        TrackAction::Archive(a) => cmd_track_state_change(a.id, "archive"),
        TrackAction::Delete(a) => cmd_track_delete(a.id),
        TrackAction::Mv(a) => cmd_track_mv(a),
        TrackAction::CcFocus(a) => cmd_track_cc_focus(a),
        TrackAction::Rename(a) => cmd_track_rename(a),
    }
}

fn cmd_track_new(args: TrackNewArgs) -> Result<(), Box<dyn std::error::Error>> {
    let mut project = load_project_cwd()?;
    let _lock = FileLock::acquire_default(&project.frame_dir)?;

    let (mut config, mut doc) = config_io::read_config(&project.frame_dir)?;

    let track = track_ops::new_track(
        &project.frame_dir,
        &mut doc,
        &mut config,
        &args.id,
        &args.name,
    )?;

    config_io::write_config(&project.frame_dir, &doc)?;
    project.config = config;
    project.tracks.push((args.id.clone(), track));

    println!("created track: {} ({})", args.name, args.id);
    Ok(())
}

fn cmd_track_state_change(
    track_id: String,
    action: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let project = load_project_cwd()?;
    let _lock = FileLock::acquire_default(&project.frame_dir)?;

    let (mut config, mut doc) = config_io::read_config(&project.frame_dir)?;

    // Capture the track's file path before state change (needed for archive file move)
    let track_file = config
        .tracks
        .iter()
        .find(|t| t.id == track_id)
        .map(|t| t.file.clone());

    match action {
        "shelve" => track_ops::shelve_track(&mut doc, &mut config, &track_id)?,
        "activate" => track_ops::activate_track(&mut doc, &mut config, &track_id)?,
        "archive" => track_ops::archive_track(&mut doc, &mut config, &track_id)?,
        _ => unreachable!(),
    }

    config_io::write_config(&project.frame_dir, &doc)?;

    // Move the track file to archive/_tracks/ after archiving
    if action == "archive"
        && let Some(file) = track_file
    {
        track_ops::archive_track_file(&project.frame_dir, &track_id, &file)?;
    }

    println!("{} → {}d", track_id, action);
    Ok(())
}

fn cmd_track_mv(args: TrackMvArgs) -> Result<(), Box<dyn std::error::Error>> {
    let mut project = load_project_cwd()?;
    let _lock = FileLock::acquire_default(&project.frame_dir)?;

    track_ops::reorder_tracks(&mut project.config, &args.id, args.position)?;

    // Rewrite the config with the new order
    // We need to regenerate the TOML since reorder_tracks only modifies in-memory config
    let config_text = toml::to_string_pretty(&project.config)?;
    let config_path = project.frame_dir.join("project.toml");
    std::fs::write(&config_path, config_text)?;

    println!("{} moved to position {}", args.id, args.position);
    Ok(())
}

fn cmd_track_cc_focus(args: TrackIdArg) -> Result<(), Box<dyn std::error::Error>> {
    let project = load_project_cwd()?;
    let _lock = FileLock::acquire_default(&project.frame_dir)?;

    let (mut config, mut doc) = config_io::read_config(&project.frame_dir)?;
    track_ops::set_cc_focus(&mut doc, &mut config, &args.id)?;
    config_io::write_config(&project.frame_dir, &doc)?;

    println!("cc-focus → {}", args.id);
    Ok(())
}

fn cmd_track_delete(track_id: String) -> Result<(), Box<dyn std::error::Error>> {
    let project = load_project_cwd()?;
    let _lock = FileLock::acquire_default(&project.frame_dir)?;

    // Check if track exists and is empty
    let track =
        find_track(&project, &track_id).ok_or_else(|| format!("track not found: {}", track_id))?;

    if !track_ops::is_track_empty_by_id(&project.frame_dir, track, &track_id) {
        let count = track_ops::total_task_count(track);
        return Err(format!(
            "track \"{}\" has {} tasks. Use `fr track archive` instead.",
            track_id, count
        )
        .into());
    }

    let (mut config, mut doc) = config_io::read_config(&project.frame_dir)?;
    track_ops::delete_track(&project.frame_dir, &mut doc, &mut config, &track_id)?;
    config_io::write_config(&project.frame_dir, &doc)?;

    println!("deleted track \"{}\"", track_id);
    Ok(())
}

fn cmd_track_rename(args: TrackRenameArgs) -> Result<(), Box<dyn std::error::Error>> {
    let mut project = load_project_cwd()?;
    let _lock = FileLock::acquire_default(&project.frame_dir)?;

    if args.name.is_none() && args.new_id.is_none() && args.prefix.is_none() {
        return Err("specify at least one of --name, --id, or --prefix".into());
    }

    let (mut config, mut doc) = config_io::read_config(&project.frame_dir)?;

    // Handle --name
    if let Some(ref new_name) = args.name {
        track_ops::rename_track_name(
            &project.frame_dir,
            &mut doc,
            &mut config,
            &args.id,
            new_name,
        )?;
        println!("renamed \"{}\" → \"{}\"", args.id, new_name);
    }

    // Handle --id (track ID rename)
    let effective_id = if let Some(ref new_id) = args.new_id {
        track_ops::rename_track_id(&project.frame_dir, &mut doc, &mut config, &args.id, new_id)?;
        println!("id {} → {}", args.id, new_id);
        new_id.clone()
    } else {
        args.id.clone()
    };

    // Handle --prefix (bulk rewrite)
    if let Some(ref new_prefix) = args.prefix {
        let old_prefix = config
            .ids
            .prefixes
            .get(&effective_id)
            .cloned()
            .ok_or_else(|| format!("no prefix configured for track '{}'", effective_id))?;

        // Reload tracks for in-memory mutation
        let cwd = std::env::current_dir().map_err(|e| format!("could not get cwd: {}", e))?;
        let root = project_io::discover_project(&cwd)?;
        project = project_io::load_project(&root)?;
        // Re-read config to get latest state after potential --name/--id changes
        let (latest_config, _) = config_io::read_config(&project.frame_dir)?;
        project.config = latest_config;

        let result = track_ops::rename_track_prefix(
            &mut project.config,
            &mut project.tracks,
            &effective_id,
            &old_prefix,
            new_prefix,
        )?;

        // Check for archived tasks
        let archive_dir = project.frame_dir.join("archive");
        let archive_id_count = {
            let archive_path = archive_dir.join(format!("{}.md", effective_id));
            if archive_path.exists() {
                if let Ok(content) = std::fs::read_to_string(&archive_path) {
                    let archive_track = crate::parse::parse_track(&content);
                    track_ops::prefix_rename_impact(
                        &[(effective_id.clone(), archive_track)],
                        &effective_id,
                        &old_prefix,
                        None,
                    )
                    .task_id_count
                } else {
                    0
                }
            } else {
                0
            }
        };

        println!("Renaming prefix {} → {}:", old_prefix, new_prefix);
        println!("  {} tasks in {}", result.tasks_renamed, effective_id);
        if archive_id_count > 0 {
            println!("  {} archived task IDs", archive_id_count);
        }
        if result.deps_updated > 0 {
            println!(
                "  {} dep references across {} other tracks",
                result.deps_updated, result.tracks_affected
            );
        }

        if args.dry_run {
            println!("(dry run — no changes written)");
            return Ok(());
        }

        if !args.yes && result.tasks_renamed > 0 {
            // Interactive confirmation
            eprint!("Proceed? [y/n] ");
            let mut input = String::new();
            std::io::stdin().read_line(&mut input)?;
            if !input.trim().eq_ignore_ascii_case("y") {
                println!("cancelled");
                return Ok(());
            }
        }

        // Rename IDs in archive file
        let archive_count = track_ops::rename_archive_prefix(
            &project.frame_dir,
            &effective_id,
            &old_prefix,
            new_prefix,
        )?;
        if archive_count > 0 {
            println!("  {} archived task IDs renamed", archive_count);
        }

        // Save all affected tracks
        for (track_id, track) in &project.tracks {
            if let Some(file) = project
                .config
                .tracks
                .iter()
                .find(|tc| tc.id == *track_id)
                .map(|tc| tc.file.as_str())
            {
                project_io::save_track(&project.frame_dir, file, track)?;
            }
        }

        // Update prefix in config doc
        config_io::set_prefix(&mut doc, &effective_id, new_prefix);
    }

    config_io::write_config(&project.frame_dir, &doc)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Maintenance handlers
// ---------------------------------------------------------------------------

fn cmd_clean(args: CleanArgs) -> Result<(), Box<dyn std::error::Error>> {
    let mut project = load_project_cwd()?;

    if !args.dry_run {
        let _lock = FileLock::acquire_default(&project.frame_dir)?;
    }

    let result = clean::clean_project(&mut project);

    // Report results
    if !result.ids_assigned.is_empty() {
        println!("IDs assigned:");
        for a in &result.ids_assigned {
            println!("  [{}] {} → \"{}\"", a.track_id, a.assigned_id, a.title);
        }
    }
    if !result.dates_assigned.is_empty() {
        println!("Dates assigned:");
        for d in &result.dates_assigned {
            println!("  [{}] {} → {}", d.track_id, d.task_id, d.date);
        }
    }
    if !result.duplicates_resolved.is_empty() {
        println!("Duplicate IDs resolved:");
        for d in &result.duplicates_resolved {
            println!(
                "  [{}] {} → {} \"{}\"",
                d.track_id, d.original_id, d.new_id, d.title
            );
        }
    }
    if !result.tasks_archived.is_empty() {
        println!("Tasks archived:");
        for a in &result.tasks_archived {
            println!("  [{}] {} \"{}\"", a.track_id, a.task_id, a.title);
        }
    }
    if !result.dangling_deps.is_empty() {
        println!("Dangling dependencies:");
        for d in &result.dangling_deps {
            println!(
                "  [{}] {} → {} (not found)",
                d.track_id, d.task_id, d.dep_id
            );
        }
    }
    if !result.broken_refs.is_empty() {
        println!("Broken references:");
        for r in &result.broken_refs {
            println!("  [{}] {} → {} (not found)", r.track_id, r.task_id, r.path);
        }
    }
    if !result.suggestions.is_empty() {
        println!("Suggestions:");
        for s in &result.suggestions {
            let msg = match s.kind {
                clean::SuggestionKind::AllSubtasksDone => {
                    "all subtasks done — consider marking done"
                }
            };
            println!("  [{}] {} — {}", s.track_id, s.task_id, msg);
        }
    }

    if args.dry_run {
        println!("(dry run — no changes written)");
    } else {
        // Save all modified tracks
        for (track_id, track) in &project.tracks {
            if let Some(file) = track_file(&project, track_id) {
                project_io::save_track(&project.frame_dir, file, track)?;
            }
        }

        // Generate ACTIVE.md
        let active_md = clean::generate_active_md(&project);
        let active_path = project.frame_dir.join("ACTIVE.md");
        std::fs::write(&active_path, active_md)?;

        let total_changes = result.ids_assigned.len()
            + result.dates_assigned.len()
            + result.duplicates_resolved.len()
            + result.tasks_archived.len();
        if total_changes == 0
            && result.dangling_deps.is_empty()
            && result.broken_refs.is_empty()
            && result.suggestions.is_empty()
        {
            println!("✓ project is clean");
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Project registry handlers
// ---------------------------------------------------------------------------

fn cmd_projects(args: ProjectsCmd, json: bool) -> Result<(), Box<dyn std::error::Error>> {
    match args.action {
        None | Some(ProjectsAction::List) => cmd_projects_list(json),
        Some(ProjectsAction::Add(a)) => cmd_projects_add(a),
        Some(ProjectsAction::Remove(a)) => cmd_projects_remove(a),
    }
}

fn cmd_projects_list(json: bool) -> Result<(), Box<dyn std::error::Error>> {
    let reg = registry::read_registry();

    if json {
        #[derive(serde::Serialize)]
        struct ProjectJson {
            name: String,
            path: String,
            exists: bool,
            #[serde(skip_serializing_if = "Option::is_none")]
            last_accessed: Option<String>,
        }
        let items: Vec<ProjectJson> = reg
            .projects
            .iter()
            .map(|e| ProjectJson {
                name: e.name.clone(),
                path: e.path.clone(),
                exists: std::path::Path::new(&e.path).join("frame").exists(),
                last_accessed: e.last_accessed_cli.map(|dt| dt.to_rfc3339()),
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&items)?);
        return Ok(());
    }

    if reg.projects.is_empty() {
        println!("No projects registered.");
        println!();
        println!("Run `fr init` in a project directory to get started,");
        println!("or `fr projects add <path>` to register an existing project.");
        return Ok(());
    }

    // Sort by last_accessed_cli (most recent first)
    let mut sorted = reg.projects.clone();
    sorted.sort_by(|a, b| {
        let ta = a.last_accessed_cli.unwrap_or_default();
        let tb = b.last_accessed_cli.unwrap_or_default();
        tb.cmp(&ta)
    });

    // Compute column widths
    let max_name = sorted.iter().map(|e| e.name.len()).max().unwrap_or(0);
    let name_w = max_name.max(4);

    for entry in &sorted {
        let exists = std::path::Path::new(&entry.path).join("frame").exists();
        let path_display = if exists {
            registry::abbreviate_path(&entry.path)
        } else {
            "(not found)".to_string()
        };
        let time_str = match entry.last_accessed_cli {
            Some(dt) => registry::relative_time(&dt),
            None => String::new(),
        };
        println!(
            "  {:<width$}  {:<30}  {}",
            entry.name,
            path_display,
            time_str,
            width = name_w
        );
    }
    Ok(())
}

fn cmd_projects_add(args: ProjectsAddArgs) -> Result<(), Box<dyn std::error::Error>> {
    let abs_path = std::fs::canonicalize(&args.path)
        .map_err(|e| format!("cannot resolve path '{}': {}", args.path, e))?;

    // Verify it contains a frame project
    let frame_dir = abs_path.join("frame");
    let config_path = frame_dir.join("project.toml");
    if !config_path.exists() {
        return Err(format!("no project.toml found at {}", frame_dir.display()).into());
    }

    // Read the project name
    let config_text = std::fs::read_to_string(&config_path)?;
    let config: crate::model::config::ProjectConfig = toml::from_str(&config_text)?;
    let name = config.project.name;

    registry::register_project(&name, &abs_path);
    println!("Added: {} ({})", name, abs_path.display());
    Ok(())
}

fn cmd_projects_remove(args: ProjectsRemoveArgs) -> Result<(), Box<dyn std::error::Error>> {
    match registry::remove_project(&args.name_or_path) {
        Ok(Some(entry)) => {
            println!("Removed: {}", entry.name);
            Ok(())
        }
        Ok(None) => Err(format!("not found: {}", args.name_or_path).into()),
        Err(e) => Err(e.into()),
    }
}

fn cmd_import(args: ImportArgs) -> Result<(), Box<dyn std::error::Error>> {
    let mut project = load_project_cwd()?;
    let _lock = FileLock::acquire_default(&project.frame_dir)?;

    let prefix = track_prefix(&project, &args.track)
        .ok_or_else(|| format!("no ID prefix configured for track '{}'", args.track))?
        .to_string();

    let position = if args.top {
        task_ops::InsertPosition::Top
    } else if let Some(ref after_id) = args.after {
        task_ops::InsertPosition::After(after_id.clone())
    } else {
        task_ops::InsertPosition::Bottom
    };

    let markdown = std::fs::read_to_string(&args.file)
        .map_err(|e| format!("could not read {}: {}", args.file, e))?;

    let track = find_track_mut(&mut project, &args.track)
        .ok_or_else(|| format!("track not found: {}", args.track))?;

    let result = import::import_tasks(&markdown, track, position, &prefix)?;

    save_track(&project, &args.track)?;

    println!(
        "imported {} tasks ({} including subtasks)",
        result.assigned_ids.len(),
        result.total_count
    );
    for id in &result.assigned_ids {
        println!("  {}", id);
    }
    Ok(())
}
