mod init;
pub use init::cmd_init;
mod merge;
pub use merge::{cmd_merge, cmd_merge_resolve};
mod git;
pub use git::cmd_git;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use regex::Regex;

/// Global override for project directory (set by -C flag)
static PROJECT_DIR_OVERRIDE: Mutex<Option<PathBuf>> = Mutex::new(None);

use crate::cli::commands::*;
use crate::cli::output::*;
use crate::io::actors;
use crate::io::config_io;
use crate::io::lock::FileLock;
use crate::io::project_io::{self, ProjectError};
use crate::io::registry;
use crate::model::inbox::Inbox;
use crate::model::project::Project;
use crate::model::task::{Metadata, Task, TaskState};
use crate::model::track::{Track, TrackNode};
use crate::ops::ids::Mint;
use crate::ops::{
    actor_merge, check, clean, deps, fix, import, inbox_ops, search, task_ops, track_ops,
};

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

            // Merge is handled in main.rs too — it owns its exit status, which
            // is how it reports a conflict to the version control system.
            // `--resolve` writes to the project, so it takes the normal path;
            // the driver form is handled in main.rs, which owns its exit status.
            Commands::Merge(args) => {
                if args.resolve.is_empty() {
                    cmd_merge(args);
                } else {
                    cmd_merge_resolve(&args.resolve)?;
                }
                Ok(())
            }

            // Repo configuration, not project content
            Commands::Git(args) => cmd_git(args, json),

            // Project registry (doesn't require a project context)
            Commands::Projects(args) => cmd_projects(args, json),

            // Actor token management
            Commands::Actor(args) => cmd_actor(args, json),

            // Read commands
            Commands::List(args) => cmd_list(args, json),
            Commands::Show(args) => cmd_show(args, json),
            Commands::Ready(args) => cmd_ready(args, json),
            Commands::Blocked => cmd_blocked(json),
            Commands::Search(args) => cmd_search(args, json),
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
            Commands::Deps(args) => cmd_deps(args, json),
            Commands::Check(args) => cmd_check(args, json),
            Commands::Info => cmd_info(json),

            // Write commands
            Commands::Add(args) => cmd_add(args),
            Commands::Push(args) => cmd_push(args),
            Commands::Sub(args) => cmd_sub(args),
            Commands::State(args) => cmd_state(args),
            Commands::Start(args) => cmd_start(args),
            Commands::Done(args) => cmd_done(args),
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
            Commands::Delete(args) => cmd_delete(args),

            // Recovery
            Commands::Recovery(args) => cmd_recovery(args, json),
        },
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn load_project_cwd() -> Result<Project, ProjectError> {
    load_project_at(&discover_project_root()?)
}

/// Load and register a project whose root is already known.
///
/// Split out so [`lock_and_load`] can discover the root, take the lock, and
/// only then read the files.
fn load_project_at(root: &Path) -> Result<Project, ProjectError> {
    let project = project_io::load_project(root)?;

    // Auto-register and touch CLI timestamp
    registry::register_project(&project.config.project.name, &project.root);
    registry::touch_cli(&project.root);

    Ok(project)
}

/// The project root, without loading or registering the project.
///
/// For commands that operate on a project's *surroundings* rather than its
/// contents — `fr git setup` configures a repo, and has to keep working when a
/// track file will not parse.
fn discover_project_root() -> Result<PathBuf, ProjectError> {
    let start = match PROJECT_DIR_OVERRIDE.lock().unwrap().as_ref() {
        Some(dir) => dir.clone(),
        None => std::env::current_dir().map_err(ProjectError::IoError)?,
    };
    project_io::discover_project(&start)
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

/// Return the configured state of a track ("active"/"shelved"/"archived"), if
/// the track exists in config.
fn track_state<'a>(project: &'a Project, track_id: &str) -> Option<&'a str> {
    project
        .config
        .tracks
        .iter()
        .find(|tc| tc.id == track_id)
        .map(|tc| tc.state.as_str())
}

/// Reject an operation that would add a task to a shelved track. A shelved
/// track is preserved for later, not receiving new work, so adding to it is
/// almost always a mistake (often a stale `--track` argument).
fn reject_add_to_shelved(
    project: &Project,
    track_id: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    match track_state(project, track_id) {
        Some(state) if !track_ops::accepts_new_tasks(state) => Err(format!(
            "track '{track_id}' is {state} and does not accept new tasks; \
             activate it first with `fr track activate {track_id}`"
        )
        .into()),
        _ => Ok(()),
    }
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

    // Both surfaces walk the same tracks, in the same order, with the same
    // tasks selected from each. Only the rendering below differs.
    let listed: Vec<(&String, &Track)> = project
        .tracks
        .iter()
        .filter(|(track_id, _)| track_is_listed(&project, track_id, &args))
        .map(|(track_id, track)| (track_id, track))
        .collect();

    if json {
        let results: Vec<TaskListJson> = listed
            .iter()
            .map(|(track_id, track)| TaskListJson {
                track: (*track_id).clone(),
                tasks: select_tasks(track, state_filter, tag_filter)
                    .all()
                    .map(task_to_json)
                    .collect(),
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&results)?);
    } else {
        for (i, (track_id, track)) in listed.iter().enumerate() {
            if i > 0 {
                println!();
            }
            let tasks = select_tasks(track, state_filter, tag_filter);
            for line in format_track_listing(track_id, track, &tasks) {
                println!("{}", line);
            }
        }
    }
    Ok(())
}

/// Whether `fr list` shows this track: the one named by the positional
/// argument, or — with no argument and without `--all` — every active track.
fn track_is_listed(project: &Project, track_id: &str, args: &ListArgs) -> bool {
    match args.track {
        Some(ref only) => track_id == only,
        None => {
            args.all
                || project
                    .config
                    .tracks
                    .iter()
                    .any(|tc| tc.id == track_id && tc.state == "active")
        }
    }
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
        // cc mode: all active tracks, focus track first
        let mut tracks: Vec<&str> = Vec::new();
        if let Some(ref focus) = project.config.agent.cc_focus {
            tracks.push(focus.as_str());
        }
        for tc in &project.config.tracks {
            if tc.state == "active" && project.config.agent.cc_focus.as_deref() != Some(&tc.id) {
                tracks.push(tc.id.as_str());
            }
        }
        tracks
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
        let cc_only = if args.cc {
            Some(project.config.agent.cc_only)
        } else {
            None
        };
        let output = ReadyJson {
            focus_track,
            cc_only,
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
            let deps = deps::task_deps(task);
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

/// One task a search matched, with every field that matched it.
///
/// Collapsing per-field hits into one entry per task is what both surfaces
/// want: the human output prints one line per task, and `--json` reports the
/// full `matched_fields` list rather than whichever field the scan reached
/// first.
struct SearchTaskHit<'a> {
    track_id: String,
    task_id: String,
    /// `None` when the hit does not resolve back to a task, which is reachable
    /// for a task with no id — hits carry `""` for those.
    task: Option<&'a Task>,
    fields: Vec<&'static str>,
}

struct SearchInboxHit<'a> {
    index: usize,
    item: &'a crate::model::inbox::InboxItem,
    fields: Vec<&'static str>,
}

/// Group hits by task, preserving first-seen order and accumulating fields.
fn group_task_hits<'a>(
    hits: &[search::SearchHit],
    resolve: impl Fn(&str, &str) -> Option<&'a Task>,
) -> Vec<SearchTaskHit<'a>> {
    let mut out: Vec<SearchTaskHit<'a>> = Vec::new();
    let mut seen: std::collections::HashMap<(String, String), usize> =
        std::collections::HashMap::new();

    for hit in hits {
        let key = (hit.track_id.clone(), hit.task_id.clone());
        let name = hit.field.name();
        match seen.get(&key) {
            Some(&i) => {
                if !out[i].fields.contains(&name) {
                    out[i].fields.push(name);
                }
            }
            None => {
                seen.insert(key, out.len());
                out.push(SearchTaskHit {
                    track_id: hit.track_id.clone(),
                    task_id: hit.task_id.clone(),
                    task: resolve(&hit.track_id, &hit.task_id),
                    fields: vec![name],
                });
            }
        }
    }
    out
}

fn hits_to_json(hits: &[SearchTaskHit]) -> Vec<SearchHitJson> {
    hits.iter()
        .map(|hit| SearchHitJson {
            track: hit.track_id.clone(),
            task: hit.task.map(task_to_json),
            matched_fields: hit.fields.iter().map(|f| f.to_string()).collect(),
        })
        .collect()
}

/// `[track] [ ] ID Title` when the task resolves, and the field that matched
/// when it does not — the only case where the human surface names a field.
fn format_search_hits(hits: &[SearchTaskHit], prefix: &str) -> Vec<String> {
    hits.iter()
        .map(|hit| match hit.task {
            Some(task) => format!("[{}{}] {}", prefix, hit.track_id, format_task_line(task)),
            None => format!(
                "[{}{}] {} (in {})",
                prefix, hit.track_id, hit.task_id, hit.fields[0]
            ),
        })
        .collect()
}

fn cmd_search(args: SearchArgs, json: bool) -> Result<(), Box<dyn std::error::Error>> {
    let project = load_project_cwd()?;
    let re = Regex::new(&args.pattern)?;

    let live = group_task_hits(
        &search::search_tasks(&project, &re, args.track.as_deref()),
        |track_id, task_id| {
            find_track(&project, track_id)
                .and_then(|track| task_ops::find_task_in_track(track, task_id))
        },
    );

    // Archives are searched by default -- finding a task you completed last
    // month is a common reason to reach for search at all -- with --no-archive
    // to opt out on a project whose archives have grown noisy.
    let archives = if args.no_archive {
        Vec::new()
    } else {
        project_io::load_archives(&project.frame_dir)?
    };
    let archived = group_task_hits(
        &search::search_archive_tasks(&archives, &re, args.track.as_deref()),
        |track_id, task_id| {
            archives
                .iter()
                .find(|(tid, _)| tid == track_id)
                .and_then(|(_, tasks)| find_task_by_id(tasks, task_id))
        },
    );

    // The inbox belongs to no track, so a track filter excludes it entirely.
    let mut inbox_hits: Vec<SearchInboxHit> = Vec::new();
    if args.track.is_none()
        && let Some(ref inbox) = project.inbox
    {
        let mut seen: std::collections::HashMap<usize, usize> = std::collections::HashMap::new();
        for hit in &search::search_inbox(inbox, &re) {
            let name = hit.field.name();
            match seen.get(&hit.item_index) {
                Some(&i) => {
                    if !inbox_hits[i].fields.contains(&name) {
                        inbox_hits[i].fields.push(name);
                    }
                }
                None => {
                    if let Some(item) = inbox.items.get(hit.item_index) {
                        seen.insert(hit.item_index, inbox_hits.len());
                        inbox_hits.push(SearchInboxHit {
                            index: hit.item_index,
                            item,
                            fields: vec![name],
                        });
                    }
                }
            }
        }
    }

    if json {
        let output = SearchJson {
            pattern: args.pattern.clone(),
            tasks: hits_to_json(&live),
            archived: hits_to_json(&archived),
            inbox: inbox_hits
                .iter()
                .map(|hit| InboxSearchHitJson {
                    index: hit.index + 1,
                    title: hit.item.title.clone(),
                    tags: hit.item.tags.clone(),
                    body: hit.item.body.clone(),
                    matched_fields: hit.fields.iter().map(|f| f.to_string()).collect(),
                })
                .collect(),
        };
        println!("{}", serde_json::to_string_pretty(&output)?);
    } else {
        for line in format_search_hits(&live, "") {
            println!("{}", line);
        }
        for line in format_search_hits(&archived, "archive:") {
            println!("{}", line);
        }
        for hit in &inbox_hits {
            let tags = if hit.item.tags.is_empty() {
                String::new()
            } else {
                format!(
                    " {}",
                    hit.item
                        .tags
                        .iter()
                        .map(|t| format!("#{}", t))
                        .collect::<Vec<_>>()
                        .join(" ")
                )
            };
            println!("[inbox:{}] {}{}", hit.index + 1, hit.item.title, tags);
        }
    }

    Ok(())
}

/// Recursively find a task by ID in a flat list of tasks (with subtasks).
fn find_task_by_id<'a>(tasks: &'a [Task], id: &str) -> Option<&'a Task> {
    for task in tasks {
        if task.id.as_deref() == Some(id) {
            return Some(task);
        }
        if let Some(found) = find_task_by_id(&task.subtasks, id) {
            return Some(found);
        }
    }
    None
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

/// Show project identity at a glance: version, name, frame dir, this clone's
/// actor token, and track counts. Read-only — never claims a token.
fn cmd_info(json: bool) -> Result<(), Box<dyn std::error::Error>> {
    let project = load_project_cwd()?;
    let frame_dir = &project.frame_dir;
    // Non-claiming read of this clone's token.
    let token = actors::read_actor_token(frame_dir);

    let name = &project.config.project.name;
    let frame_dir_str = frame_dir.display().to_string();

    // The ID frontier this clone mints against, in its own namespace: what the
    // durable store has recorded, per track prefix. Read-only.
    // `"null"` (the primary) and an unclaimed clone both map to the null
    // namespace; a letter token maps to its own.
    let namespace = token
        .as_deref()
        .and_then(crate::model::task_id::actor_namespace);
    let frontier_health = crate::io::ids::health(frame_dir);
    let frontier = crate::io::ids::recorded_by_prefix(frame_dir, namespace.as_ref());

    let active = project
        .config
        .tracks
        .iter()
        .filter(|t| t.state == "active")
        .count();
    let shelved = project
        .config
        .tracks
        .iter()
        .filter(|t| t.state == "shelved")
        .count();
    let archived = project.config.tracks.len() - active - shelved;

    if json {
        #[derive(serde::Serialize)]
        struct FrontierJson {
            /// The durable store this clone mints against.
            path: String,
            /// `ok`, `absent`, or `unparsable`.
            state: &'static str,
            /// `null` for the primary/unclaimed namespace, else the token.
            namespace: String,
            /// Prefix → highest number handed out in that namespace.
            recorded: std::collections::BTreeMap<String, u32>,
        }
        #[derive(serde::Serialize)]
        struct InfoJson {
            /// Bare crate version, so consumers can parse it as-is. The build's
            /// commit is a separate field.
            version: String,
            /// Short commit this binary was built from, or `null` when it wasn't
            /// built from a git checkout.
            commit: Option<String>,
            project: String,
            frame_dir: String,
            /// Literal token (`"a"`), `"null"` for primary, or JSON `null` when
            /// unclaimed — so consumers can distinguish all three states.
            actor: Option<String>,
            tracks: usize,
            shelved_tracks: usize,
            archived_tracks: usize,
            id_frontier: FrontierJson,
        }
        let info = InfoJson {
            version: crate::version::VERSION.to_string(),
            commit: crate::version::COMMIT.map(str::to_string),
            project: name.clone(),
            frame_dir: frame_dir_str,
            actor: token.clone(),
            tracks: active,
            shelved_tracks: shelved,
            archived_tracks: archived,
            id_frontier: FrontierJson {
                path: frontier_health.path.display().to_string(),
                state: match frontier_health.state {
                    crate::io::ids::StoreState::Ok => "ok",
                    crate::io::ids::StoreState::Absent => "absent",
                    crate::io::ids::StoreState::Unparsable(_) => "unparsable",
                },
                namespace: namespace
                    .as_ref()
                    .map_or_else(|| "null".to_string(), |t| t.as_str().to_string()),
                recorded: frontier,
            },
        };
        println!("{}", serde_json::to_string_pretty(&info)?);
        return Ok(());
    }

    println!("{:<10} {}", "version", crate::version::LONG);
    println!("{:<10} {}", "project", name);
    println!("{:<10} {}", "frame_dir", frame_dir_str);
    println!("{:<10} {}", "actor", actors::actor_label(token.as_deref()));
    if shelved > 0 || archived > 0 {
        println!(
            "{:<10} {} active, {} shelved, {} archived",
            "tracks", active, shelved, archived
        );
    } else {
        println!("{:<10} {}", "tracks", active);
    }

    // The durable ID frontier: the last number handed out per prefix in this
    // clone's namespace. Normally invisible, and the thing to look at when a
    // minted ID isn't the number you expected.
    let summary = match &frontier_health.state {
        crate::io::ids::StoreState::Unparsable(_) => "unreadable".to_string(),
        _ if frontier.is_empty() => "none recorded".to_string(),
        _ => frontier
            .iter()
            .map(|(prefix, n)| format!("{} {}", prefix, n))
            .collect::<Vec<_>>()
            .join(", "),
    };
    println!(
        "{:<10} {}  ({})",
        "frontier",
        summary,
        frontier_health.path.display()
    );
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

fn cmd_deps(args: DepsArgs, json: bool) -> Result<(), Box<dyn std::error::Error>> {
    let project = load_project_cwd()?;

    let tree = deps::dep_tree(&project, &args.id);
    if tree.status == deps::DepStatus::Missing {
        return Err(format!("task not found: {}", args.id).into());
    }

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&dep_tree_to_json(&tree))?
        );
    } else {
        for line in format_dep_tree(&tree) {
            println!("{}", line);
        }
    }
    Ok(())
}

fn cmd_check(args: CheckArgs, json: bool) -> Result<(), Box<dyn std::error::Error>> {
    // `fr check` with no flags stays read-only, on exactly the code it always
    // ran. The repair path is a separate function so that promise is visible
    // rather than asserted.
    if args.fix {
        return cmd_check_fix(args, json);
    }

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
                    check::CheckError::UnresolvedMergeConflict {
                        track_id,
                        task_id,
                        detail,
                    } => {
                        println!(
                            "  [{}] {} has an unresolved merge conflict ({}) — their version is in the recovery log (`fr recovery`); apply what is missing, then clear it with `fr merge --resolve {}`",
                            track_id, task_id, detail, task_id
                        );
                    }
                    check::CheckError::BrokenSpec {
                        track_id,
                        task_id,
                        path,
                    } => {
                        println!("  [{}] {} has broken spec: {}", track_id, task_id, path);
                    }
                    check::CheckError::TrackFileMissing {
                        track_id,
                        path,
                        state,
                    } => {
                        println!(
                            "  [{}] track file is missing: {} — the track and its tasks are absent from every view{}",
                            track_id,
                            path,
                            if state == "archived" {
                                " (archived track)"
                            } else {
                                ""
                            }
                        );
                    }
                    check::CheckError::TrackFileUnreferenced { path, title } => {
                        println!(
                            "  {} is not listed in project.toml — its tasks are invisible; add a [[tracks]] entry for it{}",
                            path,
                            title
                                .as_ref()
                                .map(|t| format!(" (titled \"{}\")", t))
                                .unwrap_or_default()
                        );
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
                    check::CheckWarning::TaskInWrongSection {
                        track_id,
                        task_id,
                        expected,
                        actual,
                    } => {
                        println!(
                            "  [{}] {} is in {} but its state belongs in {}",
                            track_id,
                            task_id,
                            fix::section_name(*actual),
                            fix::section_name(*expected)
                        );
                    }
                    check::CheckWarning::LostTask { track_id, task_id } => {
                        println!(
                            "  [{}] {} has #lost tag (recovery system)",
                            track_id, task_id
                        );
                    }
                    check::CheckWarning::ChildIdNotUnderParent {
                        track_id,
                        task_id,
                        parent_id,
                    } => {
                        println!(
                            "  [{}] {} is nested under {} but its id doesn't extend it — the id no longer says where the task lives, and {}'s child numbering can't see it. Repair with `fr check --fix`",
                            track_id, task_id, parent_id, parent_id
                        );
                    }
                    check::CheckWarning::ActorTokenUnregistered { token } => {
                        println!(
                            "  actor token '{}' (this clone's .actor) is missing from actors.toml — the next mint re-registers it, or run `fr actor set {}`",
                            token, token
                        );
                    }
                    check::CheckWarning::ActorTokenRetiredButHeld { token } => {
                        println!(
                            "  actor token '{}' (this clone's .actor) is retired in actors.toml — claim a fresh token (`fr actor claim`) or reactivate it (`fr actor set {}`)",
                            token, token
                        );
                    }
                    check::CheckWarning::ActorNameCollision { name, tokens } => {
                        println!(
                            "  {} active tokens share the name '{}': {} — likely one machine's worktrees. Collapse with `fr actor merge {} --into {}`",
                            tokens.len(),
                            name,
                            tokens.join(", "),
                            tokens[1..].join(" "),
                            tokens[0],
                        );
                    }
                    check::CheckWarning::MergeDriverUnregistered => {
                        println!(
                            "  frame's merge driver is not registered in this clone, so git will merge track files line by line — run `fr git setup`"
                        );
                    }
                    check::CheckWarning::LocalFileCommitted { path, tracked } => {
                        if *tracked {
                            // `git rm --cached` refuses a directory without
                            // `-r`, and some local-only entries are directories.
                            let flags = if project
                                .frame_dir
                                .join(std::path::Path::new(path).file_name().unwrap_or_default())
                                .is_dir()
                            {
                                "-r --cached"
                            } else {
                                "--cached"
                            };
                            println!(
                                "  {} is tracked by git, but it is local to this working copy — untrack it with `git rm {} {}`, then run `fr git setup`",
                                path, flags, path
                            );
                        } else {
                            println!(
                                "  {} is not covered by .gitignore, but it is local to this working copy — run `fr git setup` before it gets committed",
                                path
                            );
                        }
                    }
                    check::CheckWarning::UnclosedNoteFence {
                        track_id,
                        task_id,
                        title,
                        fence,
                    } => {
                        let who = match task_id {
                            Some(id) => id.clone(),
                            None => format!("\"{}\"", title),
                        };
                        println!(
                            "  [{}] {} note leaves a code fence open ({}) — frame parses it fine, but markdown renderers will treat the rest of the file as code",
                            track_id, who, fence
                        );
                    }
                    check::CheckWarning::StrandedLine {
                        track_id,
                        before_task_id,
                        before_title,
                        line,
                    } => {
                        let who = match before_task_id {
                            Some(id) => id.clone(),
                            None => format!("\"{}\"", before_title),
                        };
                        println!(
                            "  [{}] a line above {} belongs to no task: \"{}\" — kept as-is on every write; re-indent it under a task to make frame read it",
                            track_id, who, line
                        );
                    }
                    check::CheckWarning::UnclosedInboxFence {
                        index,
                        title,
                        fence,
                    } => {
                        println!(
                            "  inbox item {} (\"{}\") leaves a code fence open ({}) — frame parses it fine, but markdown renderers will treat the rest of the file as code",
                            index, title, fence
                        );
                    }
                    check::CheckWarning::IdReissuedAfterArchive {
                        task_id,
                        tracks,
                        archives,
                    } => {
                        println!(
                            "  {} is live in {} but is also archived in {} — the number was reissued after the original was archived. Renumber the live task by hand; `fr clean` only dedups live tracks",
                            task_id,
                            tracks.join(", "),
                            archives.join(", ")
                        );
                    }
                    check::CheckWarning::DuplicateArchivedId {
                        task_id,
                        total,
                        archives,
                    } => {
                        println!(
                            "  {} appears {} times in {} and in no live track — the same task was archived more than once, so its history is duplicated (a clean whose archive write landed while its track update was lost). Delete the extra copies; no number was reissued",
                            task_id,
                            total,
                            archives.join(", ")
                        );
                    }
                    check::CheckWarning::IdFrontierUnreadable { path, detail } => {
                        println!(
                            "  the ID frontier at {} is unreadable ({}) — the next mint resets it and falls back to scanning, which can't see another worktree's uncommitted tasks. Fix or delete the file",
                            path, detail
                        );
                    }
                    check::CheckWarning::IdFrontierWasReset { path } => {
                        println!(
                            "  {} shows the ID frontier was reset after becoming unreadable — numbers minted in that window may have been reissued. Delete the file to clear this warning",
                            path
                        );
                    }
                    check::CheckWarning::InterruptedOperation {
                        operation,
                        command,
                        started,
                    } => {
                        println!(
                            "  `{}` started {} did not finish — the next write command completes it. If it keeps appearing, recovery could not act; see `fr recovery`",
                            command, started
                        );
                        let _ = operation;
                    }
                }
            }
        }
        if !result.info.is_empty() {
            if !result.errors.is_empty() || !result.warnings.is_empty() {
                println!();
            }
            for info in &result.info {
                match info {
                    check::CheckInfo::RecoveryLog {
                        entry_count,
                        oldest,
                    } => {
                        println!(
                            "Recovery log: {} {} (oldest: {})",
                            entry_count,
                            if *entry_count == 1 {
                                "entry"
                            } else {
                                "entries"
                            },
                            oldest,
                        );
                        println!("  view with: fr recovery");
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

    if !result.valid {
        check_failed();
    }
    Ok(())
}

/// Exit non-zero because the project has errors.
///
/// `exit` rather than an `Err`, for the same reason `fr merge` owns its status:
/// the report *is* the output, and returning an error would print an `error:`
/// line on top of it saying the command failed — which it did not. It ran, and
/// what it found is on stdout.
///
/// Warnings do not reach here. Only `result.errors` clears `valid`, so the
/// status answers "is this project sound", which is the question a pre-commit
/// hook or a CI step is asking. Skipping destructors is safe on this path:
/// check is read-only and holds no lock.
fn check_failed() -> ! {
    std::process::exit(1)
}

/// `fr check --fix`: apply the repairs check would otherwise only describe.
///
/// All-or-nothing. If any repair in the plan deletes bytes and `--yes` was not
/// given, the run asks once and cancels entirely on anything but `y` — the
/// additive repairs included. That is what `cancelled` already means in
/// `fr delete` and `fr track rename --prefix`, and it keeps a half-applied plan
/// from being a state anyone has to reason about.
fn cmd_check_fix(args: CheckArgs, json: bool) -> Result<(), Box<dyn std::error::Error>> {
    // A dry run only previews, so it takes no lock and runs no recovery — the
    // same split `fr clean` makes. A real run locks first, like every other
    // write path, so the plan is computed against a project that is neither
    // mid-operation nor a stale read of one another `fr` was writing.
    let (mut project, _lock) = if args.dry_run {
        (load_project_cwd()?, None)
    } else {
        let (project, lock) = lock_and_load()?;
        (project, Some(lock))
    };

    let before = check::check_project(&project);
    let plan = fix::plan(&before);

    if plan.is_empty() {
        if json {
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "planned": [],
                    "applied": [],
                    "skipped": [],
                    "dry_run": args.dry_run,
                }))?
            );
        } else {
            println!("nothing to repair");
        }
        // "Nothing to repair" is not "nothing wrong". Most errors have no
        // repair by design — a dangling dep, an unresolved merge conflict, a
        // track file nobody can find — so this is the *common* way a broken
        // project leaves `--fix`, and it has to report the same status bare
        // `check` would.
        if !before.valid {
            check_failed();
        }
        return Ok(());
    }

    if !json {
        println!("Repairs:");
        for repair in &plan {
            println!("  {}", repair.describe());
        }
    }

    if args.dry_run {
        if json {
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "planned": &plan,
                    "applied": [],
                    "skipped": [],
                    "dry_run": true,
                }))?
            );
        } else {
            println!("(dry run — no changes written)");
        }
        return Ok(());
    }

    let deleting = fix::destructive_count(&plan);
    if deleting > 0 && !args.yes {
        eprint!("{deleting} of these delete data. Proceed? [y/n] ");
        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
        if !input.trim().eq_ignore_ascii_case("y") {
            eprintln!("cancelled");
            return Ok(());
        }
    }

    let result = fix::apply(&mut project, &plan);

    for track_id in fix::tracks_touched(&result) {
        let Some(file) = track_file(&project, &track_id).map(|f| f.to_string()) else {
            continue;
        };
        let Some((_, track)) = project.tracks.iter().find(|(id, _)| *id == track_id) else {
            continue;
        };
        project_io::save_track(&project.frame_dir, &file, track)?;
    }
    if fix::inbox_touched(&result)
        && let Some(inbox) = &project.inbox
    {
        project_io::save_inbox(&project.frame_dir, inbox)?;
    }

    // Re-read from disk and re-check, so the closing report describes what is
    // actually on disk rather than what we believe we wrote.
    let after = check::check_project(&load_project_cwd()?);

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "planned": &plan,
                "applied": &result.applied,
                "skipped": &result.skipped,
                "dry_run": false,
                "remaining": &after,
            }))?
        );
    } else {
        println!();
        println!("Applied {} repair(s).", result.applied.len());
        for skipped in &result.skipped {
            println!(
                "  skipped: {} — {}",
                skipped.repair.describe(),
                skipped.reason
            );
        }
        println!(
            "{} error(s), {} warning(s) remain.",
            after.errors.len(),
            after.warnings.len()
        );
    }

    // Same rule as bare `fr check`, on the same re-read: `--fix` repairs what it
    // can, and errors it could not repair are still errors. A run that fixed
    // something and left an unresolved merge conflict behind has not made the
    // project sound, and a caller keying off the status should not be told it
    // has.
    if !after.valid {
        check_failed();
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Write command handlers
// ---------------------------------------------------------------------------

/// Take the project lock, then read the project under it.
///
/// Every write command goes through this, and the order is the point. Waiting
/// for the lock is the ordinary case, not an exotic one: another `fr` holds it
/// for as long as its own write takes, and we block up to five seconds for it.
/// A project read *before* that wait is a pre-write copy of files the other
/// process is about to change, so saving it back erases whatever landed —
/// silently, with no recovery entry, and precisely when contention is highest.
///
/// That is `ed273b2` from the CLI side. The TUI answered it with a baseline and
/// `ops::reconcile` because it holds state across many writes and cannot simply
/// re-read. A CLI command has no such constraint: it reads once and writes
/// once, so reading after the lock closes the window outright.
///
/// Returning the lock alongside the project is what keeps it closed. There is
/// no ordering left for a caller to get wrong.
///
/// A previous `fr` that died partway through a multi-file operation left a
/// marker (see [`crate::io::inflight`]); this is where the remaining steps get
/// finished, before the new command touches anything. The common case is one
/// `stat` that finds no marker. Recovery rewrites files, so the project is
/// re-read when it does anything — otherwise the command would undo the repair.
fn lock_and_load() -> Result<(Project, FileLock), Box<dyn std::error::Error>> {
    let root = discover_project_root()?;
    let lock = FileLock::acquire_default(&root.join("frame"))?;
    let mut project = load_project_at(&root)?;
    recover_under_lock(&mut project)?;
    Ok((project, lock))
}

/// Complete any interrupted operation. The project lock must already be held.
fn recover_under_lock(project: &mut Project) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(outcome) = crate::ops::recover::recover_pending(project) {
        match &outcome {
            crate::ops::recover::Outcome::AlreadyComplete { .. } => {}
            crate::ops::recover::Outcome::Completed { operation, steps } => {
                eprintln!("recovered an interrupted `fr {operation}`:");
                for step in steps {
                    eprintln!("  {step}");
                }
            }
            crate::ops::recover::Outcome::Indeterminate { operation, reason } => {
                eprintln!(
                    "warning: an interrupted `fr {operation}` could not be completed \
                     automatically:\n  {reason}\n  see `fr check` and `fr recovery`"
                );
            }
        }
        *project = project_io::load_project(&project.root)?;
    }

    Ok(())
}

/// Resolve this clone's minting namespace for a CLI mint command, auto-claiming
/// a token on first use and announcing it once to stderr (so stdout stays clean
/// for the minted ID). A frontier-empty error aborts the mint, creating nothing.
/// The project lock must already be held.
fn resolve_mint_namespace(
    frame_dir: &std::path::Path,
) -> Result<Option<crate::model::task_id::Token>, Box<dyn std::error::Error>> {
    let resolved = actors::resolve_actor_token(frame_dir)?;
    if let Some(msg) = resolved.announcement {
        eprintln!("{}", msg);
    }
    Ok(crate::model::task_id::actor_namespace(&resolved.token))
}

fn cmd_add(args: AddArgs) -> Result<(), Box<dyn std::error::Error>> {
    let (mut project, _lock) = lock_and_load()?;

    reject_add_to_shelved(&project, &args.track)?;

    let prefix = track_prefix(&project, &args.track)
        .ok_or_else(|| format!("no ID prefix configured for track '{}'", args.track))?
        .to_string();
    let token = resolve_mint_namespace(&project.frame_dir)?;

    let position = if let Some(ref after_id) = args.after {
        task_ops::InsertPosition::After(after_id.clone())
    } else {
        task_ops::InsertPosition::Bottom
    };

    let frame_dir = project.frame_dir.clone();
    let track = find_track_mut(&mut project, &args.track)
        .ok_or_else(|| format!("track not found: {}", args.track))?;

    let mint = Mint::new(&frame_dir, &args.track, &prefix, token.as_ref());
    let id = task_ops::add_task(track, args.title.clone(), position, mint)?;

    // If --found-from, add a note
    if let Some(ref from_id) = args.found_from {
        task_ops::set_note(track, &id, format!("Found while working on {}", from_id))?;
    }

    save_track(&project, &args.track)?;
    println!("{}", id);
    Ok(())
}

fn cmd_push(args: PushArgs) -> Result<(), Box<dyn std::error::Error>> {
    let (mut project, _lock) = lock_and_load()?;

    reject_add_to_shelved(&project, &args.track)?;

    let prefix = track_prefix(&project, &args.track)
        .ok_or_else(|| format!("no ID prefix configured for track '{}'", args.track))?
        .to_string();
    let token = resolve_mint_namespace(&project.frame_dir)?;

    let frame_dir = project.frame_dir.clone();
    let track = find_track_mut(&mut project, &args.track)
        .ok_or_else(|| format!("track not found: {}", args.track))?;

    let id = task_ops::add_task(
        track,
        args.title.clone(),
        task_ops::InsertPosition::Top,
        Mint::new(&frame_dir, &args.track, &prefix, token.as_ref()),
    )?;

    save_track(&project, &args.track)?;
    println!("{}", id);
    Ok(())
}

fn cmd_sub(args: SubArgs) -> Result<(), Box<dyn std::error::Error>> {
    let (mut project, _lock) = lock_and_load()?;

    // Find which track the parent task is in
    let track_id = find_task_track(&project, &args.id)
        .ok_or_else(|| format!("task not found: {}", args.id))?
        .to_string();
    reject_add_to_shelved(&project, &track_id)?;
    let token = resolve_mint_namespace(&project.frame_dir)?;

    let track = find_track_mut(&mut project, &track_id)
        .ok_or_else(|| format!("track not found: {}", track_id))?;

    let sub_id = task_ops::add_subtask(track, &args.id, args.title, token.as_ref())?;

    save_track(&project, &track_id)?;
    println!("{}", sub_id);
    Ok(())
}

fn cmd_inbox_add(args: InboxCmd) -> Result<(), Box<dyn std::error::Error>> {
    let (mut project, _lock) = lock_and_load()?;

    let text = args.text.unwrap(); // We know it's Some from dispatch
    let inbox = project.inbox.get_or_insert_with(|| Inbox {
        header_lines: vec!["# Inbox".to_string(), String::new()],
        items: Vec::new(),
        source_lines: vec!["# Inbox".to_string(), String::new()],
        // A file frame is creating, so frame picks: LF, like everything else
        // it writes from scratch.
        eol: crate::parse::LineEnding::default(),
    });

    inbox_ops::add_inbox_item(inbox, text.clone(), args.tag, args.note);

    project_io::save_inbox(&project.frame_dir, inbox)?;
    println!("added to inbox");
    Ok(())
}

fn cmd_start(args: StartArgs) -> Result<(), Box<dyn std::error::Error>> {
    cmd_state(StateArgs {
        id: args.id,
        state: "active".to_string(),
    })
}

fn cmd_done(args: DoneArgs) -> Result<(), Box<dyn std::error::Error>> {
    cmd_state(StateArgs {
        id: args.id,
        state: "done".to_string(),
    })
}

fn cmd_state(args: StateArgs) -> Result<(), Box<dyn std::error::Error>> {
    let (mut project, _lock) = lock_and_load()?;

    let new_state = parse_task_state(&args.state).map_err(Box::<dyn std::error::Error>::from)?;

    let track_id = find_task_track(&project, &args.id)
        .ok_or_else(|| format!("task not found: {}", args.id))?
        .to_string();

    // A shelved track is paused work — nothing in it should be marked active.
    if new_state == TaskState::Active && track_state(&project, &track_id) == Some("shelved") {
        return Err(format!(
            "cannot mark '{}' active: its track '{track_id}' is shelved; \
             activate it first with `fr track activate {track_id}`",
            args.id
        )
        .into());
    }

    let track = find_track_mut(&mut project, &track_id)
        .ok_or_else(|| format!("track not found: {}", track_id))?;

    let task = task_ops::find_task_mut_in_track(track, &args.id)
        .ok_or_else(|| format!("task not found: {}", args.id))?;

    task_ops::set_state(task, new_state);

    // Put the task in the section its new state calls for. Asking where the
    // state belongs rather than listing `from → to` pairs is what makes this
    // total: the enumerated form here had no case for Done → Parked, so a
    // `[~]` task stayed sitting in `## Done`.
    {
        let track = find_track_mut(&mut project, &track_id)
            .ok_or_else(|| format!("track not found: {}", track_id))?;
        task_ops::reconcile_task_section(track, &args.id, new_state);
    }

    save_track(&project, &track_id)?;
    println!("{} → {}", args.id, args.state);
    Ok(())
}

fn cmd_tag(args: TagArgs) -> Result<(), Box<dyn std::error::Error>> {
    let (mut project, _lock) = lock_and_load()?;

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
    let (mut project, _lock) = lock_and_load()?;

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
    let (mut project, _lock) = lock_and_load()?;

    let track_id = find_task_track(&project, &args.id)
        .ok_or_else(|| format!("task not found: {}", args.id))?
        .to_string();

    let track = find_track_mut(&mut project, &track_id)
        .ok_or_else(|| format!("track not found: {}", track_id))?;

    if args.replace {
        task_ops::set_note(track, &args.id, args.text)?;
    } else {
        task_ops::append_note(track, &args.id, args.text)?;
    }

    save_track(&project, &track_id)?;
    println!("{} note updated", args.id);
    Ok(())
}

fn cmd_ref(args: RefArgs) -> Result<(), Box<dyn std::error::Error>> {
    let (mut project, _lock) = lock_and_load()?;

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
    let (mut project, _lock) = lock_and_load()?;

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
    let (mut project, _lock) = lock_and_load()?;

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
    let (mut project, _lock) = lock_and_load()?;
    // Taken before the tracks are borrowed mutably below.
    let frame_dir = project.frame_dir.clone();

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
        // Promote re-mints the new top-level id in the mover's namespace.
        let token = resolve_mint_namespace(&project.frame_dir)?;

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
            Mint::new(&frame_dir, &source_track_id, &prefix, token.as_ref()),
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
        // Reparent re-mints the new child id in the mover's namespace.
        let token = resolve_mint_namespace(&project.frame_dir)?;

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
            Mint::new(&frame_dir, &source_track_id, &prefix, token.as_ref()),
            &mut other_tracks,
        )?;

        save_track(&project, &source_track_id)?;
        println!("{} → {} (under {})", args.id, result.new_root_id, parent_id);
        return Ok(());
    }

    if let Some(ref target_track_id) = args.track {
        // Cross-track move
        reject_add_to_shelved(&project, target_track_id)?;
        let target_prefix = track_prefix(&project, target_track_id)
            .ok_or_else(|| format!("no ID prefix configured for track '{}'", target_track_id))?
            .to_string();
        // Re-mint the moved id (and its subtree) in the mover's namespace.
        // Resolved before any mutation so a frontier-empty abort changes nothing.
        let token = resolve_mint_namespace(&project.frame_dir)?;

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
            Mint::new(&frame_dir, target_track_id, &target_prefix, token.as_ref()),
            &mut [], // dep references are rewritten across all tracks below
        )?;

        // Rewrite dep references to the moved task across ALL tracks (the same
        // routine the TUI path uses), now that the source/target borrows have
        // released. This also covers dependents in the source and target tracks.
        task_ops::update_dep_references(&mut project.tracks, &args.id, &new_id);

        // **Target first, then source.** A cross-track move is two writes, and
        // whichever runs first decides what an interruption between them leaves
        // behind. Writing the source first removes the task before it exists
        // anywhere else, so a crash in the window loses it outright — and
        // nothing can detect that, because a task in neither track is
        // indistinguishable from a task that never existed.
        //
        // This is the ordering `fr clean` already documents for archival
        // ("append to the archive first, remove from the track second, so a
        // failure between the two writes can never lose a task") and that
        // `fr triage` uses for the inbox. `fr mv --track` was the outlier.
        //
        // The cost is that an interruption now leaves the task in *both*
        // tracks. Because the move re-mints into the mover's namespace, the two
        // copies carry different ids, so this is not a duplicate-id that
        // `fr check` reports and `fr clean` resolves — it is the same work
        // appearing twice, for a human to reconcile. Visible and recoverable
        // beats silent and not.
        // Record the intent before the first write. If this command dies between
        // the two saves, the next one finishes the move rather than leaving the
        // task in both tracks under different ids — a state nothing else can
        // detect.
        let marker = crate::io::inflight::InFlight::begin(
            &project.frame_dir,
            crate::io::inflight::Operation::CrossTrackMove {
                moves: vec![crate::io::inflight::MovedTask {
                    old_id: args.id.clone(),
                    new_id: new_id.clone(),
                }],
                source_track: source_track_id.clone(),
                target_track: target_track_id.clone(),
            },
            &format!("fr mv {} --track {}", args.id, target_track_id),
        )?;

        save_track(&project, target_track_id)?;
        if let Err(e) = save_track(&project, &source_track_id) {
            // Target holds the task under its new id; source still holds it
            // under the old one. Nothing is lost, so this is a warning about a
            // duplicate rather than a rescue of dropped data.
            let source_file = project
                .config
                .tracks
                .iter()
                .find(|tc| tc.id == source_track_id)
                .map(|tc| tc.file.as_str())
                .unwrap_or("unknown");
            crate::io::recovery::log_recovery(
                &project.frame_dir,
                crate::io::recovery::RecoveryEntry {
                    timestamp: chrono::Utc::now(),
                    category: crate::io::recovery::RecoveryCategory::Write,
                    description: format!(
                        "cross-track move: source write failed after {new_id} was written to \
                         the target — the task now exists in both tracks"
                    ),
                    fields: vec![
                        ("Source".to_string(), source_file.to_string()),
                        ("Target".to_string(), target_track_id.to_string()),
                        ("OldID".to_string(), args.id.clone()),
                        ("NewID".to_string(), new_id.clone()),
                        ("Error".to_string(), e.to_string()),
                    ],
                    body: format!(
                        "{} is in {} and {} is in {}. The in-flight marker is still in \
                         place, so the next write command completes the move by dropping \
                         {} from {}.",
                        args.id, source_file, new_id, target_track_id, args.id, source_file
                    ),
                },
            );
            // Deliberately not committed: the marker stays, and recovery finishes
            // the move on the next command.
            return Err(e.into());
        }
        // Persist any other track whose dep references were rewritten. The move
        // itself is already durable (source + target written), so a dangling dep
        // here is recoverable; saving the touched tracks makes the rewrite stick.
        for (other_id, other_track) in &project.tracks {
            if other_id == &source_track_id || other_id == target_track_id {
                continue;
            }
            if task_ops::track_has_dirty_task(other_track) {
                save_track(&project, other_id)?;
            }
        }
        marker.commit();
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
                        task_ops::InsertPosition::After(after_task_id.to_string())
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
    let (mut project, _lock) = lock_and_load()?;

    reject_add_to_shelved(&project, &args.track)?;

    let prefix = track_prefix(&project, &args.track)
        .ok_or_else(|| format!("no ID prefix configured for track '{}'", args.track))?
        .to_string();
    let token = resolve_mint_namespace(&project.frame_dir)?;

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

    // Captured before the triage removes it, so recovery can confirm it is the
    // same item before dropping it from the inbox.
    let item_title = project
        .inbox
        .as_ref()
        .and_then(|i| i.items.get(index))
        .map(|i| i.title.clone())
        .ok_or_else(|| format!("inbox item {} not found", args.index))?;

    let frame_dir = project.frame_dir.clone();
    let inbox = project.inbox.as_mut().ok_or("no inbox.md found")?;
    let track = &mut project.tracks[track_idx].1;

    let mint = Mint::new(&frame_dir, &args.track, &prefix, token.as_ref());
    let task_id = inbox_ops::triage(inbox, index, track, position, mint)?;

    // Two writes: the task lands, then the inbox item goes. Interrupted between
    // them the item exists in both places, which nothing else detects.
    let marker = crate::io::inflight::InFlight::begin(
        &project.frame_dir,
        crate::io::inflight::Operation::Triage {
            index: args.index,
            title: item_title,
            track_id: args.track.clone(),
        },
        &format!("fr triage {} --track {}", args.index, args.track),
    )?;

    // Save track first (new data), then inbox (deletion)
    save_track(&project, &args.track)?;
    if let Some(ref inbox) = project.inbox {
        project_io::save_inbox(&project.frame_dir, inbox)?;
    }
    marker.commit();
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
    let (mut project, _lock) = lock_and_load()?;

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
    let (project, _lock) = lock_and_load()?;

    let (mut config, mut doc) = config_io::read_config(&project.frame_dir)?;

    // Capture the track's file path before state change (needed for archive file move)
    let track_file = config
        .tracks
        .iter()
        .find(|t| t.id == track_id)
        .map(|t| t.file.clone());

    // And the state it is coming *from*, because `activate` means two different
    // operations. From shelved it is a config edit and nothing else; from
    // archived it also has to bring the file back out of `archive/_tracks/`.
    let was_archived = config
        .tracks
        .iter()
        .find(|t| t.id == track_id)
        .is_some_and(|t| t.state == "archived");

    match action {
        "shelve" => track_ops::shelve_track(&mut doc, &mut config, &track_id)?,
        "activate" => track_ops::activate_track(&mut doc, &mut config, &track_id)?,
        "archive" => track_ops::archive_track(&mut doc, &mut config, &track_id)?,
        _ => unreachable!(),
    }

    // Archiving is two writes — config, then the file move — and interrupted in
    // between it leaves config claiming the track is archived while the file is
    // still in tracks/, which `fr check` reports as a perfectly valid project.
    // Record the intent so the next command finishes the move.
    let marker = if action == "archive" {
        match &track_file {
            Some(file) => Some(crate::io::inflight::InFlight::begin(
                &project.frame_dir,
                crate::io::inflight::Operation::TrackArchive {
                    track_id: track_id.clone(),
                    file: file.clone(),
                },
                &format!("fr track archive {track_id}"),
            )?),
            None => None,
        }
    } else {
        None
    };

    config_io::write_config(&project.frame_dir, &doc)?;

    // Move the track file to archive/_tracks/ after archiving
    if action == "archive"
        && let Some(file) = &track_file
    {
        track_ops::archive_track_file(&project.frame_dir, &track_id, file)?;
    }

    // And back out of it when un-archiving, which is the same two writes in the
    // same order. Without this the config says active while the file is still in
    // `archive/_tracks/` — and `load_project` skips a configured track whose
    // file is missing, so the track and every task in it leave the project while
    // the command reports success. The TUI's unarchive has always done this;
    // the CLI's `activate` did not.
    if action == "activate"
        && was_archived
        && let Some(file) = &track_file
    {
        track_ops::restore_track_file(&project.frame_dir, &track_id, file)?;
    }

    if let Some(marker) = marker {
        marker.commit();
    }

    println!("{} → {}d", track_id, action);
    Ok(())
}

fn cmd_track_mv(args: TrackMvArgs) -> Result<(), Box<dyn std::error::Error>> {
    let (mut project, _lock) = lock_and_load()?;

    track_ops::reorder_tracks(&mut project.config, &args.id, args.position)?;

    // Rewrite the config with the new order
    // We need to regenerate the TOML since reorder_tracks only modifies in-memory config
    config_io::write_config_from_struct(&project.frame_dir, &project.config)?;

    println!("{} moved to position {}", args.id, args.position);
    Ok(())
}

fn cmd_track_cc_focus(args: CcFocusArgs) -> Result<(), Box<dyn std::error::Error>> {
    if args.clear {
        let (project, _lock) = lock_and_load()?;
        let (mut config, mut doc) = config_io::read_config(&project.frame_dir)?;
        track_ops::clear_cc_focus(&mut doc, &mut config);
        config_io::write_config(&project.frame_dir, &doc)?;
        println!("cc-focus cleared");
        Ok(())
    } else if let Some(id) = args.id {
        let (project, _lock) = lock_and_load()?;
        let (mut config, mut doc) = config_io::read_config(&project.frame_dir)?;
        track_ops::set_cc_focus(&mut doc, &mut config, &id)?;
        config_io::write_config(&project.frame_dir, &doc)?;
        println!("cc-focus → {}", id);
        Ok(())
    } else {
        Err("provide a track ID or use --clear".into())
    }
}

fn cmd_track_delete(track_id: String) -> Result<(), Box<dyn std::error::Error>> {
    let (project, _lock) = lock_and_load()?;

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
    let (mut project, _lock) = lock_and_load()?;

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
    //
    // The files move here and the config lands at the end of this function,
    // with the whole `--prefix` block in between — a wide window, and an
    // interruption inside it leaves the config naming a file that is gone.
    // `load_project` skips such a track, so it and its tasks drop out of every
    // view. Record the intent so the next command finishes the rename.
    let rename_marker = if let Some(ref new_id) = args.new_id {
        let old_file = config
            .tracks
            .iter()
            .find(|t| t.id == args.id)
            .map(|t| t.file.clone())
            .ok_or_else(|| format!("track not found: {}", args.id))?;
        Some(crate::io::inflight::InFlight::begin(
            &project.frame_dir,
            crate::io::inflight::Operation::TrackRename {
                old_id: args.id.clone(),
                new_id: new_id.clone(),
                old_file,
                new_file: format!("tracks/{}.md", new_id),
            },
            &format!("fr track rename {} --new-id {}", args.id, new_id),
        )?)
    } else {
        None
    };

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
    // The config is what makes the renamed track findable again, so the
    // operation is complete only now.
    if let Some(marker) = rename_marker {
        marker.commit();
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Maintenance handlers
// ---------------------------------------------------------------------------

fn cmd_clean(args: CleanArgs) -> Result<(), Box<dyn std::error::Error>> {
    let mut project = load_project_cwd()?;

    // A real clean holds the lock and mints in this clone's namespace (auto-
    // claiming a token on first use); a dry run only previews, so it neither
    // locks nor claims — and on an unclaimed clone it mints nothing (strict
    // null policy).
    let (_lock, scope) = if args.dry_run {
        (None, actors::id_scope(&project.frame_dir))
    } else {
        let lock = FileLock::acquire_default(&project.frame_dir)?;
        let token = resolve_mint_namespace(&project.frame_dir)?;
        (Some(lock), actors::IdScope::Mint(token))
    };

    let result = clean::clean_project(&mut project, scope);

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
            println!(
                "  [{}] {} {}: {}",
                d.track_id,
                d.task_id,
                d.kind.key(),
                d.date
            );
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
    if !result.sections_reconciled.is_empty() {
        println!("Sections reconciled:");
        for s in &result.sections_reconciled {
            println!(
                "  [{}] {} moved {} → {}",
                s.track_id, s.task_id, s.from, s.to
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
        Some(ProjectsAction::Prune(a)) => cmd_projects_prune(a, json),
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

fn cmd_projects_prune(
    args: ProjectsPruneArgs,
    json: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    // For a dry run, report the not-found entries without mutating the file.
    // Otherwise prune them and report what was removed.
    let removed = if args.dry_run {
        registry::read_registry()
            .projects
            .into_iter()
            .filter(|e| !registry::entry_exists(e))
            .collect::<Vec<_>>()
    } else {
        registry::prune_missing()
    };

    if json {
        #[derive(serde::Serialize)]
        struct PrunedJson {
            name: String,
            path: String,
        }
        let items: Vec<PrunedJson> = removed
            .iter()
            .map(|e| PrunedJson {
                name: e.name.clone(),
                path: e.path.clone(),
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&items)?);
        return Ok(());
    }

    if removed.is_empty() {
        println!("No not-found projects to prune.");
        return Ok(());
    }

    let verb = if args.dry_run {
        "Would remove"
    } else {
        "Removed"
    };
    println!(
        "{} {} not-found project{}:",
        verb,
        removed.len(),
        if removed.len() == 1 { "" } else { "s" }
    );
    for entry in &removed {
        println!("  {}  {}", entry.name, entry.path);
    }
    if args.dry_run {
        println!();
        println!("Run `fr projects prune` to remove them.");
    }
    Ok(())
}

fn cmd_import(args: ImportArgs) -> Result<(), Box<dyn std::error::Error>> {
    let (mut project, _lock) = lock_and_load()?;

    reject_add_to_shelved(&project, &args.track)?;

    let prefix = track_prefix(&project, &args.track)
        .ok_or_else(|| format!("no ID prefix configured for track '{}'", args.track))?
        .to_string();
    let token = resolve_mint_namespace(&project.frame_dir)?;

    let position = if args.top {
        task_ops::InsertPosition::Top
    } else if let Some(ref after_id) = args.after {
        task_ops::InsertPosition::After(after_id.clone())
    } else {
        task_ops::InsertPosition::Bottom
    };

    let markdown = std::fs::read_to_string(&args.file)
        .map_err(|e| format!("could not read {}: {}", args.file, e))?;

    let frame_dir = project.frame_dir.clone();
    let track = find_track_mut(&mut project, &args.track)
        .ok_or_else(|| format!("track not found: {}", args.track))?;

    let mint = Mint::new(&frame_dir, &args.track, &prefix, token.as_ref());
    let result = import::import_tasks(&markdown, track, position, mint)?;

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

fn cmd_delete(args: DeleteArgs) -> Result<(), Box<dyn std::error::Error>> {
    use crate::io::recovery;

    let (mut project, _lock) = lock_and_load()?;

    // Resolve each ID to its track
    let mut to_delete: Vec<(String, String)> = Vec::new(); // (track_id, task_id)
    for task_id in &args.ids {
        let track_id = find_task_track(&project, task_id)
            .ok_or_else(|| format!("task not found: {}", task_id))?
            .to_string();
        to_delete.push((track_id, task_id.clone()));
    }

    // Show what will be deleted
    if !args.yes {
        for (track_id, task_id) in &to_delete {
            let track = find_track(&project, track_id)
                .ok_or_else(|| format!("track not found: {}", track_id))?;
            let task = task_ops::find_task_in_track(track, task_id)
                .ok_or_else(|| format!("task not found: {}", task_id))?;
            let subtree_size = task_ops::count_subtree_size(task);
            if subtree_size > 1 {
                eprintln!(
                    "  [{}] {} {} ({} subtasks)",
                    track_id,
                    task_id,
                    task.title,
                    subtree_size - 1
                );
            } else {
                eprintln!("  [{}] {} {}", track_id, task_id, task.title);
            }
        }
        eprint!("Delete {} task(s)? [y/n] ", to_delete.len());
        let mut input = String::new();
        std::io::stdin().read_line(&mut input)?;
        if !input.trim().eq_ignore_ascii_case("y") {
            eprintln!("cancelled");
            return Ok(());
        }
    }

    // Delete each task
    let mut tracks_to_save = std::collections::HashSet::new();
    for (track_id, task_id) in &to_delete {
        // Capture source text for recovery before deletion
        let track = find_track(&project, track_id)
            .ok_or_else(|| format!("track not found: {}", track_id))?;
        let task = task_ops::find_task_in_track(track, task_id)
            .ok_or_else(|| format!("task not found: {}", task_id))?;
        let source_text = crate::parse::serialize_tasks(std::slice::from_ref(task), 0).join("\n");

        let track = find_track_mut(&mut project, track_id)
            .ok_or_else(|| format!("track not found: {}", track_id))?;
        task_ops::hard_delete_task(track, task_id, track_id)?;

        recovery::log_task_deletion(&project.frame_dir, task_id, track_id, &source_text);
        tracks_to_save.insert(track_id.clone());
    }

    // Save affected tracks
    for track_id in &tracks_to_save {
        save_track(&project, track_id)?;
    }

    for (_, task_id) in &to_delete {
        println!("deleted {}", task_id);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Actor token handlers
// ---------------------------------------------------------------------------

fn cmd_actor(args: ActorCmd, json: bool) -> Result<(), Box<dyn std::error::Error>> {
    match args.action {
        None => cmd_actor_status(json),
        Some(ActorAction::Claim(a)) => cmd_actor_claim(a, json),
        Some(ActorAction::Set(a)) => cmd_actor_set(a, json),
        Some(ActorAction::Retire(a)) => cmd_actor_retire(a, json),
        Some(ActorAction::Merge(a)) => cmd_actor_merge(a, json),
        Some(ActorAction::List) => cmd_actor_list(json),
    }
}

/// Read registry + this clone's token, claim `token`, and persist both files.
///
/// A clone-wide (shared) claim also clears this working copy's local
/// `frame/.actor`, which would otherwise keep winning resolution and make the
/// claim look like a no-op. Returns the outcome and the cleared override's
/// token, if any.
fn finalize_claim(
    frame_dir: &std::path::Path,
    reg: &mut actors::ActorRegistry,
    token: &str,
    name: &str,
    scope: actors::TokenScope,
) -> Result<(actors::ClaimOutcome, Option<String>), Box<dyn std::error::Error>> {
    let current = actors::read_actor_token(frame_dir);
    let outcome = reg.claim(token, name, current.as_deref(), &actors::today())?;
    actors::write_actors(frame_dir, reg)?;
    actors::write_actor_token_scoped(frame_dir, token, scope)?;
    // Only when the shared file is a distinct path: a non-git project writes the
    // "shared" token *into* the local file, so clearing it would undo the claim.
    let cleared =
        if scope == actors::TokenScope::Shared && actors::shared_actor_path(frame_dir).is_some() {
            actors::clear_local_actor_token(frame_dir)?
        } else {
            None
        };
    Ok((outcome, cleared))
}

/// Warn, on stderr, about anything that keeps a just-written token from taking
/// effect where the user expects: a local override this claim removed, or a
/// local override in the *main* working tree that still shadows a clone-wide
/// claim made from a linked worktree.
fn report_claim_scope(
    frame_dir: &std::path::Path,
    token: &str,
    scope: actors::TokenScope,
    cleared_local: Option<String>,
) {
    if let Some(held) = cleared_local.filter(|h| h != token) {
        eprintln!(
            "note: removed this working copy's local frame/.actor (held '{}') so the \
             clone-wide token applies here.",
            held
        );
    }
    if scope != actors::TokenScope::Shared {
        return;
    }
    if let Some(main_frame) = crate::io::git::main_worktree_frame_dir(frame_dir)
        && let Some(main_token) = actors::read_local_actor_token(&main_frame)
        && main_token != token
    {
        let main_root = main_frame.parent().unwrap_or(&main_frame).display();
        eprintln!(
            "note: the main working tree ({}) has a local frame/.actor holding '{}', which \
             overrides the shared token there. Run `fr actor set {}` in it (or delete that \
             file) to put the whole clone on '{}'.",
            main_root, main_token, token, token
        );
    }
}

fn thin_frontier_notice(reg: &actors::ActorRegistry) {
    if reg.is_thin_frontier() {
        let remaining = reg.never_used_frontier().len();
        eprintln!(
            "note: only {} unused token(s) remain — explicit `fr actor set <token>` is recommended.",
            remaining
        );
    }
}

/// Every id-bearing markdown file under `frame/archive/` (per-track archives and
/// archived whole-track files under `_tracks/`).
fn archive_md_files(frame_dir: &std::path::Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let archive_dir = frame_dir.join("archive");
    let push_md_dir = |dir: &std::path::Path, out: &mut Vec<PathBuf>| {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file() && path.extension().and_then(|e| e.to_str()) == Some("md") {
                    out.push(path);
                }
            }
        }
    };
    push_md_dir(&archive_dir, &mut out);
    push_md_dir(&archive_dir.join("_tracks"), &mut out);
    out.sort();
    out
}

fn cmd_actor_merge(args: ActorMergeArgs, json: bool) -> Result<(), Box<dyn std::error::Error>> {
    use crate::io::recovery::atomic_write;
    use crate::model::task_id::{TaskId, actor_namespace};
    use crate::ops::actor_merge::ProseHit;
    use crate::parse::{parse_tasks, serialize_tasks};

    let mut project = load_project_cwd()?;
    let frame_dir = project.frame_dir.clone();
    let _lock = FileLock::acquire_default(&frame_dir)?;

    // Syntactic validation: token grammar, non-empty sources, no self-merge.
    actor_merge::validate_merge_request(&args.from, &args.into)?;

    let mut reg = actors::read_actors(&frame_dir)?;

    // The target must be an existing, active token.
    match reg.actors.get(&args.into) {
        Some(e) if e.is_retired() => {
            return Err(format!(
                "target token '{}' is retired — reactivate it with `fr actor set {}` before merging into it",
                args.into, args.into
            )
            .into());
        }
        Some(_) => {}
        None => {
            return Err(format!(
                "target token '{}' is not in the registry — claim it with `fr actor set {}` first",
                args.into, args.into
            )
            .into());
        }
    }

    // Source tokens that have a registry row get retired on apply; sources that
    // are pure id-drift (no row) are still renumbered.
    let retire: Vec<String> = args
        .from
        .iter()
        .filter(|t| matches!(reg.actors.get(*t), Some(e) if !e.is_retired()))
        .cloned()
        .collect();

    // Collect every id across tracks and archive files, then plan the remap.
    let mut all_ids: Vec<TaskId> = Vec::new();
    for (_, track) in &project.tracks {
        actor_merge::collect_ids_in_track(track, &mut all_ids);
    }
    // Archive files are a `# Archive — <track>` header followed by a bare task
    // list (no `## Section` headers), so they parse/serialize as task lists, not
    // tracks. Keep the header verbatim and round-trip only the tasks.
    let archive_paths = archive_md_files(&frame_dir);
    let mut archives: Vec<(PathBuf, Vec<String>, Vec<Task>)> = Vec::new();
    for path in &archive_paths {
        let content = std::fs::read_to_string(path)?;
        let lines: Vec<String> = content.lines().map(|l| l.to_string()).collect();
        let start = lines
            .iter()
            .position(|l| l.starts_with("- ["))
            .unwrap_or(lines.len());
        let header = lines[..start].to_vec();
        let (tasks, _) = parse_tasks(&lines, start, 0, 0);
        actor_merge::collect_ids(&tasks, &mut all_ids);
        archives.push((path.clone(), header, tasks));
    }

    let into_tok = actor_namespace(&args.into);
    let from_set: HashSet<String> = args.from.iter().cloned().collect();
    let pairs = actor_merge::build_merge_map(&all_ids, &from_set, into_tok.as_ref());

    if pairs.is_empty() && retire.is_empty() {
        return Err(format!(
            "nothing to merge: no ids in namespace(s) {} and no matching active registry row",
            args.from.join(", ")
        )
        .into());
    }

    let map: HashMap<String, TaskId> = pairs
        .iter()
        .map(|(o, n)| (o.as_str().to_string(), n.clone()))
        .collect();

    // Apply in memory (for both the preview and the real run), gathering the
    // prose occurrences and which files changed.
    let mut hits: Vec<(String, ProseHit)> = Vec::new();
    let mut changed_tracks: Vec<String> = Vec::new();
    for (id, track) in project.tracks.iter_mut() {
        let mut local: Vec<ProseHit> = Vec::new();
        if actor_merge::apply_map_to_track(track, &map, args.rewrite_notes, &mut local) {
            changed_tracks.push(id.clone());
        }
        for h in local {
            hits.push((format!("track:{}", id), h));
        }
    }
    let mut changed_archives: Vec<usize> = Vec::new();
    for (i, (path, _header, tasks)) in archives.iter_mut().enumerate() {
        let mut local: Vec<ProseHit> = Vec::new();
        let changed = actor_merge::apply_map_to_tasks(tasks, &map, args.rewrite_notes, &mut local);
        let label = format!(
            "archive:{}",
            path.file_name().and_then(|s| s.to_str()).unwrap_or("")
        );
        if changed {
            changed_archives.push(i);
        }
        for h in local {
            hits.push((label.clone(), h));
        }
    }

    // Persist, unless this is a dry run.
    if !args.dry_run {
        // Tracks and archives are renumbered before the registry is written, so
        // an interruption leaves ids already remapped while the source tokens are
        // still active — silent, and a naive retry finds no source-namespace ids
        // to work from. The marker records which tokens still need retiring.
        let marker = crate::io::inflight::InFlight::begin(
            &project.frame_dir,
            crate::io::inflight::Operation::ActorMerge {
                sources: args.from.clone(),
                target: args.into.clone(),
            },
            &format!(
                "fr actor merge {} --into {}",
                args.from.join(" "),
                args.into
            ),
        )?;

        for tid in &changed_tracks {
            save_track(&project, tid)?;
        }
        for &i in &changed_archives {
            let (path, header, tasks) = &archives[i];
            let body = serialize_tasks(tasks, 0).join("\n");
            let content = if header.is_empty() {
                format!("{}\n", body)
            } else {
                format!("{}\n{}\n", header.join("\n"), body)
            };
            atomic_write(path, content.as_bytes())?;
        }
        for tok in &retire {
            reg.retire(tok, &actors::today())?;
        }
        actors::write_actors(&frame_dir, &reg)?;
        marker.commit();

        // The remap is collision-free by construction; verify against a reload.
        if let Ok(reloaded) = project_io::load_project(&project.root) {
            let dups = check::check_project(&reloaded)
                .errors
                .iter()
                .filter(|e| matches!(e, check::CheckError::DuplicateId { .. }))
                .count();
            if dups > 0 {
                eprintln!(
                    "warning: {} duplicate id(s) present after merge — run `fr check`",
                    dups
                );
            }
        }
    }

    if json {
        let renamed: Vec<_> = pairs
            .iter()
            .map(|(o, n)| serde_json::json!({ "old": o.as_str(), "new": n.as_str() }))
            .collect();
        let prose: Vec<_> = hits
            .iter()
            .map(|(src, h)| {
                serde_json::json!({
                    "source": src,
                    "old": h.old,
                    "new": h.new,
                    "is_citation": h.is_citation,
                    "rewritten": args.rewrite_notes && !h.is_citation,
                    "context": h.context,
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "into": args.into,
                "from": args.from,
                "dry_run": args.dry_run,
                "applied": !args.dry_run,
                "renamed": renamed,
                "retired": retire,
                "prose_hits": prose,
            }))?
        );
        return Ok(());
    }

    if args.dry_run {
        println!("DRY RUN — no files written\n");
    }
    println!("Merge {} → {}", args.from.join(" + "), args.into);

    if pairs.is_empty() {
        println!("\n(no ids to renumber)");
    } else {
        println!("\n{} id(s) renumbered:", pairs.len());
        for (o, n) in &pairs {
            println!("  {:<18} → {}", o.as_str(), n.as_str());
        }
    }

    if !hits.is_empty() {
        println!("\nprose references ({}):", hits.len());
        for (src, h) in &hits {
            let tag = if h.is_citation {
                "  [git citation — NOT rewritten]"
            } else if args.rewrite_notes {
                "  [rewritten]"
            } else {
                "  [report only — pass --rewrite-notes to rewrite]"
            };
            println!("  [{}] {} → {}{}", src, h.old, h.new, tag);
            println!("      …{}…", h.context);
        }
    }

    let retire_verb = if args.dry_run {
        "would retire"
    } else {
        "retired"
    };
    if !retire.is_empty() {
        println!("\ntokens {}: {}", retire_verb, retire.join(", "));
    }

    if args.dry_run {
        println!("\nRe-run without --dry-run to apply.");
    } else {
        println!(
            "\nMerged into '{}'. Any working copy still holding {} must be re-pointed \
             (`fr actor set {}` there, or delete its `frame/.actor` to inherit the shared token).",
            args.into,
            args.from
                .iter()
                .map(|t| format!("'{}'", t))
                .collect::<Vec<_>>()
                .join("/"),
            args.into
        );
    }
    Ok(())
}

fn cmd_actor_status(json: bool) -> Result<(), Box<dyn std::error::Error>> {
    let project = load_project_cwd()?;
    let frame_dir = &project.frame_dir;
    let reg = actors::read_actors(frame_dir)?;
    let local = actors::read_local_actor_token(frame_dir);
    let shared = actors::read_shared_actor_token(frame_dir);
    let (token, token_source) = actors::actor_token_with_source(frame_dir);
    let entry = token.as_ref().and_then(|t| reg.actors.get(t));
    let frontier_remaining = reg.never_used_frontier().len();
    // How this working copy resolved its token: a local override, the clone-wide
    // shared token, the main working tree's token (linked worktree), or nothing.
    let source = match token_source {
        actors::TokenSource::Local => "local",
        actors::TokenSource::Shared => "shared",
        actors::TokenSource::MainWorktree => "inherited",
        actors::TokenSource::None => "none",
    };

    if json {
        #[derive(serde::Serialize)]
        struct StatusJson {
            token: Option<String>,
            claimed: bool,
            primary: bool,
            in_registry: bool,
            state: Option<String>,
            name: Option<String>,
            source: String,
            frontier_remaining: usize,
            thin_frontier: bool,
        }
        let status = StatusJson {
            primary: token.as_deref() == Some("null"),
            in_registry: entry.is_some(),
            state: entry.map(|e| e.state.clone()),
            name: entry.map(|e| e.name.clone()),
            claimed: token.is_some(),
            token: token.clone(),
            source: source.to_string(),
            frontier_remaining,
            thin_frontier: reg.is_thin_frontier(),
        };
        println!("{}", serde_json::to_string_pretty(&status)?);
        return Ok(());
    }

    // A short note on where a tokened identity came from.
    let inherited_note = |main: Option<PathBuf>| match main.as_ref().and_then(|f| f.parent()) {
        Some(root) => format!(
            "  (inherited from the main working tree {})",
            root.display()
        ),
        None => "  (inherited from the main working tree)".to_string(),
    };
    let scope_note = match token_source {
        actors::TokenSource::Shared => "  (shared clone token, inherited by worktrees)".to_string(),
        actors::TokenSource::MainWorktree => {
            inherited_note(crate::io::git::main_worktree_frame_dir(frame_dir))
        }
        actors::TokenSource::Local if shared.is_some() && shared != local => {
            "  (local override; shared token differs)".to_string()
        }
        actors::TokenSource::Local => "  (local to this worktree)".to_string(),
        actors::TokenSource::None => String::new(),
    };

    match &token {
        None => {
            println!("This working copy is unclaimed (operating as primary / legacy, untokened).");
            if let crate::io::git::WorktreeKind::Linked { main_root } =
                crate::io::git::worktree_kind(frame_dir)
            {
                // A worktree can't auto-claim on first mint, so say so here
                // rather than letting the next `fr add` be the messenger.
                println!(
                    "This is a linked git worktree, so a mint here will not auto-claim a token."
                );
                println!(
                    "Run `fr actor claim` in the main working tree{} to claim one for the whole \
                     clone, or `fr actor claim --local` here to run this worktree as its own actor.",
                    match main_root {
                        Some(root) => format!(" ({})", root.display()),
                        None => String::new(),
                    }
                );
            } else {
                println!(
                    "Run `fr actor claim` to claim a token, or `fr actor set null` to record this clone as primary."
                );
            }
        }
        Some(tok) if tok == "null" => {
            println!("Token: null — primary (untokened){}", scope_note);
            if entry.is_none() {
                println!(
                    "warning: token 'null' is not in the registry (run `fr actor set null` to record it)"
                );
            }
        }
        Some(tok) => match entry {
            Some(e) => println!("Token: {} ({}) — {}{}", tok, e.state, e.name, scope_note),
            None => {
                println!("Token: {}{}", tok, scope_note);
                println!(
                    "warning: token '{}' is not in the registry (run `fr actor set {}` to record it)",
                    tok, tok
                );
            }
        },
    }

    thin_frontier_notice(&reg);
    Ok(())
}

fn cmd_actor_claim(args: ActorClaimArgs, json: bool) -> Result<(), Box<dyn std::error::Error>> {
    let project = load_project_cwd()?;
    let frame_dir = project.frame_dir.clone();
    let _lock = FileLock::acquire_default(&frame_dir)?;

    let mut reg = actors::read_actors(&frame_dir)?;
    let token = match reg.auto_pick() {
        Some(t) => t,
        None => {
            let hint = if reg.has_retired() {
                "no unused tokens remain. Reclaim a retired token with `fr actor set <retired-token>` (see `fr actor list`), or claim a custom multi-char token with `fr actor set <aa|foo|…>`."
            } else {
                "no unused tokens remain. Claim a custom multi-char token with `fr actor set <aa|foo|…>`."
            };
            return Err(hint.into());
        }
    };

    let name = args.name.unwrap_or_else(actors::default_name);
    let scope = if args.local {
        actors::TokenScope::Local
    } else {
        actors::TokenScope::Shared
    };
    let (_, cleared) = finalize_claim(&frame_dir, &mut reg, &token, &name, scope)?;
    report_claim_scope(&frame_dir, &token, scope, cleared);

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "token": token,
                "outcome": "created",
            }))?
        );
    } else {
        println!("claimed token '{}'", token);
        thin_frontier_notice(&reg);
    }
    Ok(())
}

fn cmd_actor_set(args: ActorSetArgs, json: bool) -> Result<(), Box<dyn std::error::Error>> {
    let project = load_project_cwd()?;
    let frame_dir = project.frame_dir.clone();
    let _lock = FileLock::acquire_default(&frame_dir)?;

    let warnings = actors::validate_token(&args.token)?;
    for w in &warnings {
        eprintln!("warning: {}", w);
    }

    let mut reg = actors::read_actors(&frame_dir)?;
    let name = args.name.unwrap_or_else(actors::default_name);
    // The null (primary) token is always local — the shared token is a real
    // letter that worktrees inherit, never null.
    let scope = if args.local || args.token == "null" {
        actors::TokenScope::Local
    } else {
        actors::TokenScope::Shared
    };
    let (outcome, cleared) = finalize_claim(&frame_dir, &mut reg, &args.token, &name, scope)?;
    report_claim_scope(&frame_dir, &args.token, scope, cleared);

    let outcome_str = match outcome {
        actors::ClaimOutcome::Created => "created",
        actors::ClaimOutcome::Reclaimed => "reclaimed",
        actors::ClaimOutcome::AlreadyOwned => "already_owned",
    };

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "token": args.token,
                "outcome": outcome_str,
            }))?
        );
        return Ok(());
    }

    match outcome {
        actors::ClaimOutcome::Created if args.token == "null" => {
            println!("claimed token 'null' (primary / untokened)");
        }
        actors::ClaimOutcome::Created => println!("claimed token '{}'", args.token),
        actors::ClaimOutcome::Reclaimed => {
            println!("reclaimed retired token '{}'", args.token)
        }
        actors::ClaimOutcome::AlreadyOwned => {
            println!(
                "token '{}' is already claimed by this working copy (no change)",
                args.token
            )
        }
    }
    thin_frontier_notice(&reg);
    Ok(())
}

fn cmd_actor_retire(args: ActorRetireArgs, json: bool) -> Result<(), Box<dyn std::error::Error>> {
    let project = load_project_cwd()?;
    let frame_dir = project.frame_dir.clone();
    let _lock = FileLock::acquire_default(&frame_dir)?;

    let mut reg = actors::read_actors(&frame_dir)?;
    reg.retire(&args.token, &actors::today())?;
    actors::write_actors(&frame_dir, &reg)?;

    let is_own = actors::read_actor_token(&frame_dir).as_deref() == Some(args.token.as_str());

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "token": args.token,
                "outcome": "retired",
                "was_own": is_own,
            }))?
        );
        return Ok(());
    }

    println!("retired token '{}'", args.token);
    if is_own {
        eprintln!(
            "warning: this working copy's token ('{}') is now retired — run `fr actor claim` or `fr actor set <token>` to claim a new one.",
            args.token
        );
    }
    Ok(())
}

fn cmd_actor_list(json: bool) -> Result<(), Box<dyn std::error::Error>> {
    let project = load_project_cwd()?;
    let reg = actors::read_actors(&project.frame_dir)?;
    let current = actors::read_actor_token(&project.frame_dir);

    if json {
        #[derive(serde::Serialize)]
        struct RowJson {
            token: String,
            name: String,
            state: String,
            #[serde(skip_serializing_if = "Option::is_none")]
            claimed: Option<String>,
            #[serde(skip_serializing_if = "Option::is_none")]
            retired: Option<String>,
            is_current: bool,
        }
        let rows: Vec<RowJson> = reg
            .actors
            .iter()
            .map(|(t, e)| RowJson {
                is_current: current.as_deref() == Some(t.as_str()),
                token: t.clone(),
                name: e.name.clone(),
                state: e.state.clone(),
                claimed: e.claimed.clone(),
                retired: e.retired.clone(),
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&rows)?);
        return Ok(());
    }

    if reg.actors.is_empty() {
        println!(
            "No actors registered (this project predates actor tokens; it operates as primary/legacy)."
        );
        return Ok(());
    }

    let token_w = reg.actors.keys().map(|t| t.len()).max().unwrap_or(5).max(5);
    for (tok, e) in &reg.actors {
        let marker = if current.as_deref() == Some(tok.as_str()) {
            "*"
        } else {
            " "
        };
        let date = match e.state.as_str() {
            "retired" => e.retired.as_deref().map(|d| format!("retired {}", d)),
            _ => e.claimed.as_deref().map(|d| format!("claimed {}", d)),
        }
        .unwrap_or_default();
        println!(
            "{} {:<token_w$}  {:<8}  {:<20}  {}",
            marker, tok, e.state, e.name, date
        );
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Recovery handlers
// ---------------------------------------------------------------------------

fn cmd_recovery(args: RecoveryCmd, global_json: bool) -> Result<(), Box<dyn std::error::Error>> {
    use crate::io::recovery;

    match args.action {
        Some(RecoveryAction::Prune(prune_args)) => {
            let project = load_project_cwd()?;
            let before = if let Some(ref s) = prune_args.before {
                Some(
                    chrono::DateTime::parse_from_rfc3339(s)
                        .map_err(|e| format!("invalid timestamp '{}': {}", s, e))?
                        .with_timezone(&chrono::Utc),
                )
            } else {
                None
            };
            let count = recovery::prune_recovery(&project.frame_dir, before, prune_args.all)?;
            println!("pruned {} entries", count);
            Ok(())
        }
        Some(RecoveryAction::Path) => {
            let project = load_project_cwd()?;
            let path = recovery::recovery_log_path(&project.frame_dir);
            println!("{}", path.display());
            Ok(())
        }
        None => {
            let project = load_project_cwd()?;
            let json = args.json || global_json;
            let limit = args.limit.unwrap_or(10);
            let since = if let Some(ref s) = args.since {
                Some(
                    chrono::DateTime::parse_from_rfc3339(s)
                        .map_err(|e| format!("invalid timestamp '{}': {}", s, e))?
                        .with_timezone(&chrono::Utc),
                )
            } else {
                None
            };

            let entries = recovery::read_recovery_entries(&project.frame_dir, Some(limit), since);

            if entries.is_empty() {
                if json {
                    println!("[]");
                } else {
                    println!("No recovery log entries.");
                }
                return Ok(());
            }

            if json {
                let json_entries: Vec<serde_json::Value> =
                    entries.iter().map(|e| e.to_json()).collect();
                println!("{}", serde_json::to_string_pretty(&json_entries)?);
            } else {
                for entry in &entries {
                    print!("{}", entry.to_display_markdown());
                }
            }
            Ok(())
        }
    }
}
