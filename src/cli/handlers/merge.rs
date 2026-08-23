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
//! `2` declined or broken.
//!
//! # What declining actually gets you
//!
//! **Not a fallback to the VCS's own merge.** Git treats *any* non-zero status
//! from a merge driver as a conflict: it leaves our side in the file, marks the
//! path unmerged, and stops — so `1` and `2` are indistinguishable to it, and
//! neither produces the `<<<<<<<` markers git's internal merge would. Measured,
//! not assumed; `scratch/archive-merge-repro/driver-exit-codes.sh` runs all four
//! cases.
//!
//! The fallback people expect from `2` happens one step earlier, and for a
//! different reason: `project.toml` and `actors.toml` merge line by line because
//! they are **never routed to the driver at all**, not because the driver turns
//! them down.
//!
//! So declining is still the right answer for a routed file whose shape we
//! cannot name — halting beats guessing, and guessing is what silently emptied
//! one side of every archive merge — but what it buys is a safe stop, not a
//! second opinion. The status stays distinct because it is the one the driver's
//! own stderr explains, and because a VCS that does distinguish them is entitled
//! to a straight answer.

use crate::cli::commands::MergeArgs;
use crate::ops::merge_files::{self, FileKind, MergeReport};
use crate::ops::reconcile;

/// Merged cleanly.
const EXIT_MERGED: i32 = 0;
/// Merged, but something was left undecided.
const EXIT_CONFLICT: i32 = 1;
/// Not a file this driver handles, or it could not run.
const EXIT_DECLINED: i32 = 2;

pub fn cmd_merge(args: MergeArgs) -> i32 {
    crate::io::dryrun::arm(args.dry_run);
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
        // Declining halts the merge with our side intact and the path unmerged
        // — it does *not* hand the file back to the VCS's own merge; see the
        // module docs. That is still the right answer, because the alternative
        // is guessing at a file shape, and guessing is precisely what silently
        // emptied one side of every archive merge.
        eprintln!(
            "fr merge: {} is not a frame track, archive or inbox file — declining",
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
        if report.took_anything() {
            let units = report.took_theirs + report.deleted;
            // The text around the tasks is said separately, and said at all.
            // A merge whose only change was their heading, title or note used
            // to print nothing whatsoever, which is the shape of output a
            // silent loss hides in.
            let what = match (units, report.took_their_shell) {
                (0, _) => kind.surroundings().to_string(),
                (n, false) => format!("{n} {}", kind.unit()),
                (n, true) => format!("{n} {} and the text around them", kind.unit()),
            };
            eprintln!("fr merge: {label} — merged {what} from the other side");
        }
        return EXIT_MERGED;
    }

    report_conflicts(&report, ours, label, now);
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
pub fn cmd_merge_resolve(ids: &[String], dry_run: bool) -> Result<(), Box<dyn std::error::Error>> {
    crate::io::dryrun::arm(dry_run);
    let (mut project, _lock) = super::lock_and_load()?;

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

/// Tell the user what was set aside, which side that was, where it went, and
/// what to do next — in that order.
///
/// Written to stderr because that is what a VCS surfaces while a merge is
/// running, and repeated into the recovery log because stderr scrolls away.
///
/// # Why it leads with the loss and names the sides
///
/// It used to lead with the list and end with *"the merged file is valid frame
/// markdown — no conflict markers were written"*. Both halves of that sentence
/// are true and the second half is the most quotable line in the output, sitting
/// directly under the news that a version had been discarded. Twice now a reader
/// has come within one step of trusting it and pushing a merge that had eaten
/// one side; both times what caught it was checking the file rather than reading
/// this.
///
/// The other half of the problem is that **"kept ours" names different sides in
/// different operations**. In a rebase HEAD is upstream, so "ours" is the branch
/// being rebased onto and your own replayed commit is "theirs" — the exact
/// inverse of a merge. A driver is never told which it is in, so it asks:
/// [`crate::io::git::operation_kind`]. Without that line the report is not
/// merely unhelpful, it reads as the opposite of the truth to half its readers.
fn report_conflicts(
    report: &MergeReport,
    ours: &str,
    label: &str,
    now: chrono::DateTime<chrono::Utc>,
) {
    let owner = owning_project(ours);

    // A conflict whose key carries no `#`/`~` sigil names no task, so nothing in
    // the file can carry a marker for it and `fr merge --resolve` cannot clear
    // one. The two kinds have to be reported in different words.
    let task_conflicts = report.conflicts.iter().filter(|c| is_task_key(&c.key));
    let unaccounted = report
        .conflicts
        .iter()
        .any(|c| c.reason == reconcile::ConflictReason::UnaccountedLoss);

    let n = report.conflicts.len();
    eprintln!(
        "fr merge: CONFLICT in {label} — {n} version{} set aside, NOT merged",
        if n == 1 { "" } else { "s" }
    );

    let operation = match &owner {
        Owner::Found(root) => crate::io::git::operation_kind(&root.join("frame")),
        Owner::None { .. } => crate::io::git::VcsOperation::Unknown,
    };
    match operation.sides() {
        Some(sides) => eprintln!("  {sides}"),
        None => {
            // Not "frame cannot tell" and nothing else: an unnamed operation is
            // still an operation somebody is standing in, and the rule they need
            // is the same one either way. Say the rule.
            eprintln!(
                "  \"ours\" and \"theirs\" below are the VCS's labels, not yours — and a rebase or"
            );
            eprintln!(
                "  cherry-pick inverts them, so there \"ours\" is the branch being replayed onto"
            );
            eprintln!("  and \"theirs\" is your own commit");
        }
    }

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

    // Name the log by absolute path. The marker left in the file is committed
    // and travels; the log does not, and the reader may well be in a different
    // working copy by the time they follow this.
    match log_conflicts(report, &owner, label, now) {
        Logged::At(path) => {
            let lookup = ids
                .first()
                .map(|id| format!(" (`fr recovery --for {id}`)"))
                .unwrap_or_default();
            eprintln!("the version set aside is in the recovery log{lookup}:");
            eprintln!("  {}", path.display());
        }
        Logged::NoProject { searched } => {
            eprintln!("WARNING: the version set aside was NOT recorded — no frame project found");
            eprintln!("  from {}", searched.display());
            eprintln!("recover it from version control before you stage this file");
        }
    }

    // The file parses, and saying so is still worth a line — but never as the
    // last word, and never phrased as an all-clear.
    eprintln!("this file has no <<<<<<< markers, by design, so frame's own tools still read it.");
    eprintln!("that is NOT a sign the merge resolved anything: staging it as it stands commits");
    eprintln!("one side and discards the other.");

    if unaccounted {
        eprintln!("one conflict above is a DEFECT IN fr merge rather than a decision it made:");
        eprintln!("lines only the other side added did not reach the merged file. Please report");
        eprintln!("it. The merge was halted rather than completed, and nothing was overwritten.");
    }

    // An archive carries no marker, so it must not be told it does. `fr check`'s
    // conflict detector and `fr merge --resolve` both walk `project.tracks`,
    // which archives are not in — a marker written here could be neither
    // reported nor cleared. The halt is what carries the decision instead: the
    // one person who can make it is standing in front of it right now.
    if report.kind == FileKind::Archive {
        eprintln!("this archive keeps one version of each task above; edit it by hand if the");
        eprintln!("other is the one you want, then stage the file to finish the merge");
        return;
    }

    let marked = task_conflicts.count();
    if marked == 0 {
        // Nothing in the file records this, because there is no task to record
        // it on. Say so, rather than pointing at a `conflict:` line that is not
        // there and a `--resolve` that would report nothing to clear.
        eprintln!(
            "nothing in the file marks this — it names no task, so there is nothing to mark."
        );
        eprintln!("this message and the recovery log are the only record: settle the text by hand");
        eprintln!("before you stage the file");
        return;
    }
    if marked < n {
        eprintln!(
            "{marked} of those carry a `conflict:` line, which `fr check` reports as an error;"
        );
        eprintln!("the rest name no task and leave nothing in the file — settle those by hand");
    } else {
        eprintln!(
            "each task above carries a `conflict:` line, which `fr check` reports as an error"
        );
    }
    if ids.is_empty() {
        eprintln!("resolve with `fr note` / `fr state`, then clear it with `fr merge --resolve`");
    } else {
        eprintln!(
            "resolve with `fr note` / `fr state`, then: fr merge --resolve {}",
            ids.join(" ")
        );
    }
}

/// Whether a conflict key names a task, and so whether a `conflict:` marker can
/// exist for it. The sigil is the whole test — see
/// [`crate::ops::reconcile::SURROUNDING_TEXT_KEY`].
fn is_task_key(key: &str) -> bool {
    key.starts_with('#') || key.starts_with('~')
}

/// The project that owns the file being merged, or where we looked for one.
///
/// Wanted twice over — once to name which VCS operation is running, once to put
/// the discarded side somewhere — so it is found once and passed around.
enum Owner {
    Found(std::path::PathBuf),
    /// No project could be found holding the file being merged.
    None {
        searched: std::path::PathBuf,
    },
}

/// Where the discarded side went, or why it went nowhere.
enum Logged {
    /// Written to this log. Absolute — the reader may be standing anywhere.
    At(std::path::PathBuf),
    /// No project could be found holding the file being merged.
    NoProject { searched: std::path::PathBuf },
}

/// Find the project holding the file being merged.
///
/// **Located from `--ours`, not just from the working directory.** A VCS runs
/// the driver from the worktree root with `%A` as a temp file there, so both
/// answer the same; a driver run by hand or by an agent from somewhere else does
/// not. The discovered project must contain the file being merged, or this
/// declines — otherwise a merge of files in a scratch directory writes the
/// discarded side into whatever unrelated project happens to sit above the
/// current directory, which is a real way to lose it.
fn owning_project(ours: &str) -> Owner {
    let ours_path = std::path::Path::new(ours);
    let ours_abs = ours_path
        .canonicalize()
        .unwrap_or_else(|_| ours_path.to_path_buf());

    let mut searched = ours_abs.parent().unwrap_or(&ours_abs).to_path_buf();

    // `discover_project`, not `load_project_cwd`: no registry write, and no need
    // to parse a project we are not otherwise touching.
    let mut found = crate::io::project_io::discover_project(&searched).ok();

    if found.is_none()
        && let Ok(cwd) = std::env::current_dir()
    {
        searched = cwd.clone();
        // Only accept the working directory's project when it actually holds
        // the file being merged.
        found = crate::io::project_io::discover_project(&cwd)
            .ok()
            .filter(|root| ours_abs.starts_with(root));
    }

    match found {
        Some(root) => Owner::Found(root),
        None => Owner::None { searched },
    }
}

/// Record each conflict in the recovery log of the project that owns the file
/// being merged.
///
/// Best-effort by design: a merge that already succeeded must not be failed over
/// a log write. But the caller is told which case it was, because "nothing was
/// recorded" and "their version is in the log" are the two things a reader must
/// never have confused.
fn log_conflicts(
    report: &MergeReport,
    owner: &Owner,
    label: &str,
    now: chrono::DateTime<chrono::Utc>,
) -> Logged {
    let root = match owner {
        Owner::Found(root) => root,
        Owner::None { searched } => {
            return Logged::NoProject {
                searched: searched.clone(),
            };
        }
    };
    let frame_dir = root.join("frame");

    for conflict in &report.conflicts {
        crate::io::recovery::log_recovery(
            &frame_dir,
            crate::io::recovery::RecoveryEntry {
                timestamp: now,
                category: crate::io::recovery::RecoveryCategory::Conflict,
                description: format!(
                    "merge conflict on {} in {label} — kept the `ours` side",
                    conflict.key
                ),
                fields: vec![("Reason".to_string(), conflict.reason.describe().to_string())],
                body: conflict.theirs.join("\n"),
            },
        );
    }
    Logged::At(crate::io::recovery::recovery_log_path(&frame_dir))
}
