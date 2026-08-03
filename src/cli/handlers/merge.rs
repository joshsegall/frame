//! `fr merge` — the version-control merge driver entry point.
//!
//! Deliberately outside the usual handler path, for two reasons that matter
//! while a rebase is running:
//!
//! - **It takes no lock.** The only file it writes is the temporary one the VCS
//!   handed it. Acquiring `frame/.lock` mid-merge would block on, or deadlock
//!   against, a concurrent `fr`.
//! - **It does not register the project.** Every normal command auto-registers
//!   what it touches into `~/.config/frame/projects.toml`; a driver firing once
//!   per conflicted file during a rebase must not. It uses
//!   [`crate::io::project_io::discover_project`] directly, which does not.
//!
//! Exit status is the interface with the VCS, so it is the return value here
//! rather than an error: `0` merged, `1` conflicted (stop and let a human look),
//! `2` declined or broken (fall back to the VCS's own merge).

use crate::cli::commands::MergeArgs;
use crate::ops::merge_files::{self, FileKind, MergeReport};

/// Merged cleanly.
const EXIT_MERGED: i32 = 0;
/// Merged, but something was left undecided.
const EXIT_CONFLICT: i32 = 1;
/// Not a file this driver handles, or it could not run.
const EXIT_DECLINED: i32 = 2;

pub fn cmd_merge(args: MergeArgs) -> i32 {
    // Clap guarantees all three are present whenever `--resolve` is absent, and
    // `--resolve` never reaches here (main.rs routes it through the normal
    // handler path, since it writes to the project and needs the lock).
    let (Some(base), Some(ours), Some(theirs)) =
        (args.base.as_ref(), args.ours.as_ref(), args.theirs.as_ref())
    else {
        eprintln!("fr merge: --base, --ours and --theirs are all required");
        return EXIT_DECLINED;
    };

    let Some(kind) = resolve_kind(&args, ours) else {
        // Declining is the whole point of a distinct status: the VCS still has
        // its own merge, and using it is better than guessing at a file shape.
        eprintln!(
            "fr merge: {} is not a frame track or inbox file — declining",
            args.path.as_deref().unwrap_or(ours)
        );
        return EXIT_DECLINED;
    };

    // One timestamp for the whole run, so the `conflict:` marker left in the
    // file and the recovery entry holding their version carry the same one and
    // can be matched up by eye.
    let now = chrono::Utc::now();
    let stamp = now.to_rfc3339_opts(chrono::SecondsFormat::Secs, true);

    let report = match merge_files::merge_files(
        kind,
        std::path::Path::new(base),
        std::path::Path::new(ours),
        std::path::Path::new(theirs),
        &stamp,
    ) {
        Ok(report) => report,
        Err(e) => {
            eprintln!("fr merge: {e}");
            return EXIT_DECLINED;
        }
    };

    let label = args.path.as_deref().unwrap_or(ours);

    if report.is_clean() {
        // Quiet unless something actually came across — a clean rebase should
        // not narrate itself once per file.
        if report.took_theirs > 0 || report.deleted > 0 {
            eprintln!(
                "fr merge: {label} — merged {} {} from the other side",
                report.took_theirs + report.deleted,
                if kind == FileKind::Track {
                    "task(s)"
                } else {
                    "item(s)"
                }
            );
        }
        return EXIT_MERGED;
    }

    report_conflicts(&report, label, now);
    EXIT_CONFLICT
}

/// Clear the `conflict:` marker on tasks whose conflict has been dealt with.
///
/// A normal write command, unlike the driver above: it takes the lock, loads the
/// project and saves it. That is why `main.rs` routes it here rather than
/// through the exit-status path — nothing is talking to a VCS at this point.
///
/// It does not check that anything was actually resolved, because it cannot: the
/// merge kept our version and recorded theirs, and whether the right thing came
/// out of that is a judgment only the person who read both can make. Clearing
/// the marker *is* that judgment being recorded.
pub fn cmd_merge_resolve(ids: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let mut project = super::load_project_cwd()?;
    let _lock = super::lock_and_recover(&mut project)?;

    let mut cleared = Vec::new();
    let mut not_conflicted = Vec::new();
    let mut missing = Vec::new();
    let mut touched_tracks = Vec::new();

    for id in ids {
        let Some(track_id) = super::find_task_track(&project, id).map(|t| t.to_string()) else {
            missing.push(id.clone());
            continue;
        };
        let Some(track) = super::find_track_mut(&mut project, &track_id) else {
            missing.push(id.clone());
            continue;
        };
        let Some(task) = crate::ops::task_ops::find_task_mut_in_track(track, id) else {
            missing.push(id.clone());
            continue;
        };

        let before = task.metadata.len();
        task.metadata
            .retain(|m| !matches!(m, crate::model::task::Metadata::Conflict(_)));
        if task.metadata.len() == before {
            not_conflicted.push(id.clone());
            continue;
        }
        task.dirty = true;
        cleared.push(id.clone());
        if !touched_tracks.contains(&track_id) {
            touched_tracks.push(track_id);
        }
    }

    for track_id in &touched_tracks {
        super::save_track(&project, track_id)?;
    }

    for id in &cleared {
        println!("{id} conflict resolved");
    }
    for id in &not_conflicted {
        println!("{id} had no conflict marker");
    }
    if !missing.is_empty() {
        return Err(format!("task not found: {}", missing.join(", ")).into());
    }
    Ok(())
}

/// Settle which merge algorithm applies before anything is parsed.
///
/// `--kind` wins when given (tests and manual runs); otherwise the VCS-supplied
/// `--path` decides; failing that, our own file path, so a hand-run
/// `fr merge --ours frame/tracks/main.md ...` works without extra flags.
fn resolve_kind(args: &MergeArgs, ours: &str) -> Option<FileKind> {
    if let Some(kind) = args.kind.as_deref() {
        return FileKind::parse(kind);
    }
    if let Some(path) = args.path.as_deref() {
        return merge_files::kind_for_path(path);
    }
    merge_files::kind_for_path(ours)
}

/// Tell the user what was left undecided, where the other version went, and what
/// to do next.
///
/// Written to stderr because that is what a VCS surfaces while a merge is
/// running, and repeated into the recovery log because stderr scrolls away.
fn report_conflicts(report: &MergeReport, label: &str, now: chrono::DateTime<chrono::Utc>) {
    eprintln!("fr merge: conflict in {label}");
    let mut ids = Vec::new();
    for conflict in &report.conflicts {
        // Keys are `#ID` or `~title`; the sigil is internal to the merge.
        let key = conflict
            .key
            .strip_prefix('#')
            .or_else(|| conflict.key.strip_prefix('~'))
            .unwrap_or(&conflict.key);
        eprintln!("  {key} — {}", conflict.reason.describe());
        if conflict.key.starts_with('#') {
            ids.push(key.to_string());
        }
    }

    let logged = log_conflicts(report, label, now);

    // No conflict markers were written, so say plainly that the file is intact
    // and readable — otherwise the obvious next move is to open it looking for
    // `<<<<<<<` and conclude the merge did nothing.
    eprintln!("the merged file is valid frame markdown — no conflict markers were written");
    if logged {
        eprintln!("their version of each task above is in the recovery log — see `fr recovery`");
    }
    eprintln!("each task above carries a `conflict:` line, which `fr check` reports as an error");
    if ids.is_empty() {
        eprintln!("resolve with `fr note` / `fr state`, then clear it with `fr merge --resolve`");
    } else {
        eprintln!(
            "resolve with `fr note` / `fr state`, then: fr merge --resolve {}",
            ids.join(" ")
        );
    }
}

/// Record each conflict in the project's recovery log. Returns whether anything
/// was written.
///
/// Best-effort by design: the driver may be running somewhere the project cannot
/// be discovered — a bare repo, a temp checkout — and a merge that already
/// succeeded must not be failed over a log write. When it cannot log, the stderr
/// report above simply omits the pointer.
fn log_conflicts(report: &MergeReport, label: &str, now: chrono::DateTime<chrono::Utc>) -> bool {
    let Ok(cwd) = std::env::current_dir() else {
        return false;
    };
    // `discover_project`, not `load_project_cwd`: no registry write, and no need
    // to parse a project we are not otherwise touching.
    let Ok(root) = crate::io::project_io::discover_project(&cwd) else {
        return false;
    };
    let frame_dir = root.join("frame");

    for conflict in &report.conflicts {
        crate::io::recovery::log_recovery(
            &frame_dir,
            crate::io::recovery::RecoveryEntry {
                timestamp: now,
                category: crate::io::recovery::RecoveryCategory::Conflict,
                description: format!(
                    "merge conflict on {} in {label} — kept our version",
                    conflict.key
                ),
                fields: vec![("Reason".to_string(), conflict.reason.describe().to_string())],
                body: conflict.theirs.join("\n"),
            },
        );
    }
    true
}
