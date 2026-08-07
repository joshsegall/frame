//! Reading a project off disk the way the code under test reads it.
//!
//! Shared by P7 (`conservation.rs`) and P8 (`concurrency.rs`) — two properties
//! that ask different questions of the same tree. P7 asks whether a sequence of
//! operations conserved what was there; P8 asks whether two writers between
//! them lost anything either one acknowledged. Both answers depend on reading
//! `frame/` correctly, and that is not a one-liner.
//!
//! **Three file shapes live under `frame/`, and each has its own pair.**
//! `tracks/<id>.md` and `archive/_tracks/<id>.md` are tracks, with `## Section`
//! headers, and parse with `parse_track`. `archive/<id>.md` is a flat task list
//! under a `# Archive — <id>` heading with no sections at all, and parses with
//! `parse_archive`. Reading either as the other is a defect this project has
//! shipped in both directions: walking sections in a done-task archive finds
//! nothing and reports success, and `archive/_tracks/` is skipped entirely by
//! `load_archives`. A test harness that got this wrong would under-count and
//! call it conservation.
//!
//! So [`all_tasks`] goes through `project_io::load_archives` for done-task
//! archives rather than inventing a second reading of the format, and handles
//! `_tracks/` separately because that function does not see it.

#![allow(dead_code)] // each consumer uses a different subset

use std::collections::BTreeSet;
use std::path::Path;

use frame::io::project_io;
use frame::model::task::Task;
use frame::model::track::{Track, TrackNode};
use frame::parse::{parse_archive, parse_track, serialize_archive, serialize_track};

/// Every `.md` under `frame/`, so content is judged across tracks and archives
/// together — a task moving into the archive is not a loss.
pub fn all_markdown(frame_dir: &Path) -> Vec<(std::path::PathBuf, String)> {
    let mut out = Vec::new();
    let mut stack = vec![frame_dir.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "md")
                && let Ok(text) = std::fs::read_to_string(&path)
            {
                out.push((path, text));
            }
        }
    }
    out.sort();
    out
}

pub fn walk<'a>(task: &'a Task, out: &mut Vec<&'a Task>) {
    out.push(task);
    for sub in &task.subtasks {
        walk(sub, out);
    }
}

pub fn tasks_of(track: &Track) -> Vec<&Task> {
    let mut out = Vec::new();
    for node in &track.nodes {
        if let TrackNode::Section { tasks, .. } = node {
            for task in tasks {
                walk(task, &mut out);
            }
        }
    }
    out
}

/// Every task anywhere under `frame/`, flattened, subtasks included.
///
/// Each of the three file shapes is read under its own pair — see the module
/// docs. Cloned rather than borrowed because the three sources have different
/// lifetimes and a test harness has no reason to care.
pub fn all_tasks(frame_dir: &Path) -> Vec<Task> {
    let mut out = Vec::new();

    for (path, text) in all_markdown(frame_dir) {
        if path.components().any(|c| c.as_os_str() == "archive") {
            continue;
        }
        let track = parse_track(&text);
        out.extend(tasks_of(&track).into_iter().cloned());
    }

    for (_, tasks) in project_io::load_archives(frame_dir).unwrap_or_default() {
        let mut flat = Vec::new();
        for task in &tasks {
            walk(task, &mut flat);
        }
        out.extend(flat.into_iter().cloned());
    }

    // `archive/_tracks/` holds whole archived track files, which `load_archives`
    // skips. They are still part of the project's content.
    let whole = frame_dir.join("archive").join("_tracks");
    if let Ok(entries) = std::fs::read_dir(&whole) {
        for entry in entries.flatten() {
            if entry.path().extension().is_some_and(|e| e == "md")
                && let Ok(text) = std::fs::read_to_string(entry.path())
            {
                let track = parse_track(&text);
                out.extend(tasks_of(&track).into_iter().cloned());
            }
        }
    }

    out
}

/// Titles and IDs present anywhere under `frame/`.
pub fn present(frame_dir: &Path) -> (BTreeSet<String>, BTreeSet<String>) {
    let mut titles = BTreeSet::new();
    let mut ids = BTreeSet::new();
    for task in all_tasks(frame_dir) {
        if !task.title.trim().is_empty() {
            titles.insert(task.title.clone());
        }
        if let Some(id) = &task.id {
            ids.insert(id.to_string());
        }
    }
    (titles, ids)
}

/// Every ID under `frame/`, **with repeats** — the tally a uniqueness check
/// needs, which [`present`]'s set has already thrown away.
pub fn id_tally(frame_dir: &Path) -> Vec<String> {
    all_tasks(frame_dir)
        .iter()
        .filter_map(|t| t.id.as_ref().map(|id| id.to_string()))
        .collect()
}

/// The whole of `frame/`'s markdown, for substring checks on unowned lines.
pub fn all_text(frame_dir: &Path) -> String {
    all_markdown(frame_dir)
        .into_iter()
        .map(|(_, t)| t)
        .collect::<Vec<_>>()
        .join("\n")
}

/// The first file under `frame/` that is not a fixpoint of its own
/// parse/serialize pair, described — or `None` if every one settles.
///
/// A file one rewrite away from its own fixpoint is churn: it produces a diff
/// nobody asked for on the next unrelated command. `f1a4ff5` and `0dfa9d1` were
/// both that, found only after the diff showed up in someone's `git status`.
pub fn unsettled(frame_dir: &Path) -> Option<String> {
    for (path, text) in all_markdown(frame_dir) {
        let is_archive = path.components().any(|c| c.as_os_str() == "archive");
        let is_whole_track = path.components().any(|c| c.as_os_str() == "_tracks");
        let rewritten = if is_archive && !is_whole_track {
            serialize_archive(&parse_archive(&text))
        } else {
            serialize_track(&parse_track(&text))
        };
        if rewritten != text {
            return Some(format!(
                "{} is unsettled\nwrote:       {text:?}\nrewrites to: {rewritten:?}",
                path.display()
            ));
        }
    }
    None
}
