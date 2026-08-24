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

/// `json` and `project_dir` are the two global flags, passed in rather than left
/// behind. `main.rs` dispatches this arm before project discovery, so nothing
/// else hands them over — which is why `--json` was accepted and ignored here
/// for the driver's whole life, and why `-C`, the one way to *name* the project
/// a hand-run driver belongs to, could not reach [`owning_project`].
pub fn cmd_merge(args: MergeArgs, json: bool, project_dir: Option<&str>) -> i32 {
    crate::io::dryrun::arm(args.dry_run);
    // Clap guarantees all three are present whenever `--resolve` is absent, and
    // `--resolve` never reaches here (main.rs routes it through the normal
    // handler path, since it writes to the project and needs the lock).
    let (Some(base), Some(ours), Some(theirs)) =
        (args.base.as_ref(), args.ours.as_ref(), args.theirs.as_ref())
    else {
        // No document, even under `--json`: this is the driver failing to run,
        // not a verdict about a file. See [`crate::cli::output::MergeJson`].
        eprintln!("fr merge: --base, --ours and --theirs are all required");
        return EXIT_DECLINED;
    };

    let label = args.path.as_deref().unwrap_or(ours).to_string();

    let Some(kind) = resolve_kind(&args, ours) else {
        // Declining halts the merge with our side intact and the path unmerged
        // — it does *not* hand the file back to the VCS's own merge; see the
        // module docs. That is still the right answer, because the alternative
        // is guessing at a file shape, and guessing is precisely what silently
        // emptied one side of every archive merge.
        //
        // Unlike the two failures around it, this *is* a verdict — the driver
        // ran and refused a named file — so a consumer gets a document for it.
        if json {
            emit_json(declined_json(&label, args.dry_run));
        } else {
            eprintln!("fr merge: {label} is not a frame track, archive or inbox file — declining");
        }
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

    // Resolved once and passed around: both the report and the recovery log want
    // it, and discovering it twice is how the two could disagree.
    let owner = owning_project(ours, args.path.as_deref(), project_dir);
    let operation = match &owner {
        Owner::Found(root) => crate::io::git::operation_kind(&root.join("frame")),
        Owner::None { .. } => crate::io::git::VcsOperation::Unknown,
    };

    if report.is_clean() {
        if json {
            emit_json(clean_json(&report, &label, &owner, operation, args.dry_run));
        } else if report.took_anything() {
            // Quiet unless something actually came across — a clean rebase
            // should not narrate itself once per file.
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

    // Logged before either report is produced, because both have to state the
    // outcome and neither may state a different one.
    let logged = log_conflicts(&report, &owner, &label, now);
    if json {
        emit_json(conflict_json(
            &report,
            &label,
            &owner,
            operation,
            &logged,
            args.dry_run,
        ));
    } else {
        report_conflicts(&report, &label, operation, &logged);
    }
    EXIT_CONFLICT
}

fn emit_json<T: serde::Serialize>(doc: T) {
    match serde_json::to_string_pretty(&doc) {
        Ok(text) => println!("{text}"),
        // Unreachable for these shapes, and silence would be the one failure a
        // consumer cannot distinguish from "nothing happened".
        Err(e) => eprintln!("fr merge: could not render the JSON report: {e}"),
    }
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
///
/// `--json` reports it as the write it is, in [`TaskWriteJson`]'s shape, like
/// every other task-writing command. It was left out of the sweep alongside the
/// driver — but the reasoning that exempted the driver ("its interface is an
/// exit status") never applied here: this one takes the lock, writes the
/// project, and is the step a program automating a conflict has to finish with.
///
/// [`TaskWriteJson`]: crate::cli::output::TaskWriteJson
pub fn cmd_merge_resolve(
    ids: &[String],
    json: bool,
    dry_run: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    crate::io::dryrun::arm(dry_run);
    let (mut project, _lock) = super::lock_and_load()?;

    let mut cleared = Vec::new();
    let mut resolved_tasks = Vec::new();
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
        // Snapshotted rather than looked up again afterwards: the report wants
        // the task as it now stands, and a second lookup is a second chance for
        // the two to disagree.
        resolved_tasks.push(task.clone());
        cleared.push(id.clone());
        if !touched_tracks.contains(&track_id) {
            touched_tracks.push(track_id);
        }
    }

    for track_id in &touched_tracks {
        super::save_track(&project, track_id)?;
    }

    let human = || {
        for id in &cleared {
            println!("{id} conflict resolved");
        }
        for id in &not_conflicted {
            println!("{id} had no conflict marker");
        }
    };

    if !missing.is_empty() {
        // A failed run emits no document — it would describe changes beside an
        // error, which is the one shape the `--json` sweep rules out. The human
        // form still says what *did* land first, because a person needs to know
        // which of the ids they passed were cleared before the run gave up.
        if !json {
            human();
        }
        return Err(format!("task not found: {}", missing.join(", ")).into());
    }

    super::report_task_write(
        json,
        "merge --resolve",
        !cleared.is_empty(),
        None,
        resolved_tasks.iter().collect(),
        human,
    )
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
    label: &str,
    operation: crate::io::git::VcsOperation,
    logged: &Logged,
) {
    // A conflict whose key carries no `#`/`~` sigil names no task, so nothing in
    // the file can carry a marker for it and `fr merge --resolve` cannot clear
    // one. The two kinds have to be reported in different words.
    let task_conflicts = report.conflicts.iter().filter(|c| is_task_key(&c.key));
    let unaccounted = report
        .conflicts
        .iter()
        .any(|c| c.reason == reconcile::ConflictReason::UnaccountedLoss);
    // Under `--dry-run` nothing was written: not the merged file, so no
    // `conflict:` marker; not the recovery log, so no set-aside version. Every
    // sentence below that says where something *is* has to say "would" instead,
    // and this is the one path where a reader's question is precisely "was my
    // work recorded?".
    let preview = matches!(logged, Logged::WouldBeAt(_));

    let n = report.conflicts.len();
    eprintln!(
        "fr merge: CONFLICT in {label} — {n} version{} {}, NOT merged",
        if n == 1 { "" } else { "s" },
        if preview {
            "would be set aside"
        } else {
            "set aside"
        }
    );

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
        let key = bare_key(&conflict.key);
        eprintln!("  {key} — {}", conflict.reason.describe());
        if conflict.key.starts_with('#') {
            ids.push(key.to_string());
        }
    }

    // Name the log by absolute path. The marker left in the file is committed
    // and travels; the log does not, and the reader may well be in a different
    // working copy by the time they follow this.
    match logged {
        Logged::At(path) => {
            let lookup = ids
                .first()
                .map(|id| format!(" (`fr recovery --for {id}`)"))
                .unwrap_or_default();
            eprintln!("the version set aside is in the recovery log{lookup}:");
            eprintln!("  {}", path.display());
        }
        Logged::WouldBeAt(path) => {
            eprintln!("--dry-run: nothing was written. On a real run the version set aside would");
            eprintln!("go to the recovery log at:");
            eprintln!("  {}", path.display());
        }
        Logged::NoProject { searched } => {
            eprintln!(
                "WARNING: the version set aside was NOT recorded — no frame project found for"
            );
            eprintln!("  {label}");
            eprintln!("  (searched from {})", searched.display());
            eprintln!("name the project with `fr -C <dir> merge …`, or pass `--path` as git does,");
            eprintln!("and recover this side from version control before you stage the file");
        }
    }

    // Everything below describes the merged file and what to do with it, and
    // under `--dry-run` there is no merged file: no marker was written, nothing
    // is staged, and `fr merge --resolve` has nothing to clear. Saying any of it
    // anyway is how a preview came to end on "each task above carries a
    // `conflict:` line" for tasks that carry nothing.
    if preview {
        report_unaccounted(unaccounted);
        eprintln!("--dry-run: nothing was written, so no task carries a `conflict:` marker and");
        eprintln!("the file on disk is untouched. Re-run without --dry-run to perform the merge.");
        return;
    }

    // The file parses, and saying so is still worth a line — but never as the
    // last word, and never phrased as an all-clear.
    eprintln!("this file has no <<<<<<< markers, by design, so frame's own tools still read it.");
    eprintln!("that is NOT a sign the merge resolved anything: staging it as it stands commits");
    eprintln!("one side and discards the other.");

    report_unaccounted(unaccounted);

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

/// The one conflict reason that is a bug report rather than a decision, said in
/// the same words wherever it is said.
///
/// It is about the *merge*, not about the file, so it survives `--dry-run`: a
/// preview that would have dropped their lines is exactly as much of a defect as
/// a real run that did.
fn report_unaccounted(unaccounted: bool) {
    if !unaccounted {
        return;
    }
    eprintln!("one conflict above is a DEFECT IN fr merge rather than a decision it made:");
    eprintln!("lines only the other side added did not reach the merged file. Please report");
    eprintln!("it. The merge was halted rather than completed, and nothing was overwritten.");
}

/// Whether a conflict key names a task, and so whether a `conflict:` marker can
/// exist for it. The sigil is the whole test — see
/// [`crate::ops::reconcile::SURROUNDING_TEXT_KEY`].
fn is_task_key(key: &str) -> bool {
    key.starts_with('#') || key.starts_with('~')
}

/// The conflict key with the merge's internal `#`/`~` sigil removed.
fn bare_key(key: &str) -> &str {
    key.strip_prefix('#')
        .or_else(|| key.strip_prefix('~'))
        .unwrap_or(key)
}

/// Whether a `conflict:` line for this conflict actually reached the merged
/// file.
///
/// Three ways it does not, and a consumer has to be able to tell them apart from
/// a conflict that *is* marked: the key names no task, so there is nothing to
/// hang a marker on; the file is an archive, which `fr check` and
/// `fr merge --resolve` do not walk, so a marker there could be neither reported
/// nor cleared; or the run was a preview and no file was written at all.
fn marker_written(kind: FileKind, key: &str, preview: bool) -> bool {
    !preview && kind != FileKind::Archive && is_task_key(key)
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
    /// `--dry-run`: nothing was written, and this is where it would have gone.
    /// A distinct variant rather than a flag beside `At` so the match that
    /// reports it is exhaustive — the preview claiming a version *is* in the log
    /// is the failure this exists to make unwriteable.
    WouldBeAt(std::path::PathBuf),
    /// No project could be found holding the file being merged.
    NoProject { searched: std::path::PathBuf },
}

/// Find the project the merged file belongs to.
///
/// # The owner is named, not guessed
///
/// This decides where a discarded version goes, so it is the one lookup in the
/// driver whose wrong answer destroys data — either by recording nothing, or, in
/// the branch that is easier to miss, by recording it in a project that has
/// nothing to do with the merge. Evidence is taken strongest first:
///
/// 1. **`-C <dir>`.** Explicit, so it is trusted outright. Newly reachable:
///    `main.rs` dispatches the driver before project discovery, so until the
///    global flag was passed in, the one way to *name* the project could not
///    reach the code that needed it.
/// 2. **`--path`.** Git's `%P` is the repo-relative path the result belongs at,
///    so the project holding it is the owner by definition — wherever the VCS
///    chose to put its temp files. Searched from the working directory first,
///    then from `--ours`. **This is the branch that answers a hand-run or
///    wrapper-run driver** whose three temp files live outside the project
///    entirely; it used to fall all the way through to "no frame project found"
///    while `--path` sat there naming it.
/// 3. **`--ours`, then the working directory**, each required to contain the
///    file being merged. Today's rule, now applied to both rather than only to
///    the second.
///
/// # What it deliberately still does not do
///
/// With neither `-C` nor `--path`, the answer is a guess, and a temp file inside
/// an unrelated project's tree will still resolve to that project. Requiring
/// containment in `root/frame` instead of `root` would look tighter and would
/// break the one caller that matters: git runs the driver from the worktree root
/// with `%A` a temp file *there*, outside `frame/`. Two named locators and an
/// honest warning is the trade; a stricter containment test is not.
fn owning_project(ours: &str, path: Option<&str>, project_dir: Option<&str>) -> Owner {
    let ours_abs = absolute(std::path::Path::new(ours));
    let cwd = std::env::current_dir().ok();

    // 1. `-C`. An explicit directory that is not a project is a caller error
    //    worth surfacing, so it does not silently fall through to a guess.
    if let Some(dir) = project_dir {
        let start = absolute(std::path::Path::new(dir));
        return match crate::io::project_io::discover_project(&start) {
            Ok(root) => Owner::Found(root),
            Err(_) => Owner::None { searched: start },
        };
    }

    // 2. `--path` names the destination, so a project that could hold it is the
    //    owner however far away the temp files sit.
    if let Some(path) = path {
        for start in [cwd.as_deref(), ours_abs.parent()].into_iter().flatten() {
            if let Ok(root) = crate::io::project_io::discover_project(start)
                && project_could_hold(&root, path)
            {
                return Owner::Found(root);
            }
        }
    }

    // 3. The file being merged, then where we are standing — each only when it
    //    actually holds `--ours`.
    //
    //    `discover_project`, not `load_project_cwd`: no registry write, and no
    //    need to parse a project we are not otherwise touching.
    let searched = ours_abs.parent().unwrap_or(&ours_abs).to_path_buf();
    for start in [Some(searched.as_path()), cwd.as_deref()]
        .into_iter()
        .flatten()
    {
        if let Ok(root) = crate::io::project_io::discover_project(start)
            && ours_abs.starts_with(&root)
        {
            return Owner::Found(root);
        }
    }

    Owner::None { searched }
}

/// Whether `root` is a project that could hold the destination `path`.
///
/// `path` is the VCS's, so it is relative to the repository rather than to the
/// project — and a project in a subdirectory makes those differ. Matching from
/// the *end*, at the last `frame` component, is the same rule
/// [`merge_files::kind_for_path`] uses and for the same reason: nothing may
/// assume where the project sits.
///
/// The **directory** is what is tested, not the file. An add/add — two branches
/// that each created the same track — has a destination that does not exist in
/// the working tree yet, and refusing to locate the project for the one merge
/// case with no ancestor would be exactly backwards.
fn project_could_hold(root: &std::path::Path, path: &str) -> bool {
    let normalized = path.replace('\\', "/");
    let components: Vec<&str> = normalized.split('/').filter(|c| !c.is_empty()).collect();
    let Some(start) = components.iter().rposition(|c| *c == "frame") else {
        return false;
    };
    let mut candidate = root.to_path_buf();
    for component in &components[start..] {
        candidate.push(component);
    }
    candidate.parent().is_some_and(|dir| dir.is_dir())
}

/// An absolute path for a file that may not exist, since `canonicalize` fails on
/// one that does not — and a merge destination legitimately may not.
fn absolute(path: &std::path::Path) -> std::path::PathBuf {
    path.canonicalize().unwrap_or_else(|_| {
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            std::env::current_dir()
                .map(|cwd| cwd.join(path))
                .unwrap_or_else(|_| path.to_path_buf())
        }
    })
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
        // Called even under `--dry-run`. The write is a no-op — `io::dryrun`
        // blocks it — but the path is recorded on the way past, so a preview
        // still reports the file it would have touched.
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

    let path = crate::io::recovery::recovery_log_path(&frame_dir);
    if crate::io::dryrun::is_active() {
        Logged::WouldBeAt(path)
    } else {
        Logged::At(path)
    }
}

// ---------------------------------------------------------------------------
// The `--json` report
// ---------------------------------------------------------------------------

/// The driver declined: it could not name the file's shape, so nothing was
/// parsed and there is nothing but the verdict to report.
fn declined_json(label: &str, dry_run: bool) -> crate::cli::output::MergeJson {
    crate::cli::output::MergeJson {
        command: "merge",
        outcome: "declined",
        exit: EXIT_DECLINED,
        dry_run,
        path: label.to_string(),
        kind: None,
        operation: crate::io::git::VcsOperation::Unknown.slug(),
        took_theirs: 0,
        deleted: 0,
        took_their_shell: false,
        conflicts: Vec::new(),
        // Nothing was set aside, so nothing needed recording. `true` rather than
        // `false`, which is reserved for "there was something and it went
        // nowhere" — the one alarming case, which must not be diluted.
        recorded: true,
        recovery_log: None,
        project: None,
    }
}

fn clean_json(
    report: &MergeReport,
    label: &str,
    owner: &Owner,
    operation: crate::io::git::VcsOperation,
    dry_run: bool,
) -> crate::cli::output::MergeJson {
    crate::cli::output::MergeJson {
        command: "merge",
        outcome: "merged",
        exit: EXIT_MERGED,
        dry_run,
        path: label.to_string(),
        kind: Some(report.kind.label()),
        operation: operation.slug(),
        took_theirs: report.took_theirs,
        deleted: report.deleted,
        took_their_shell: report.took_their_shell,
        conflicts: Vec::new(),
        recorded: true,
        recovery_log: None,
        project: owner_root(owner),
    }
}

fn conflict_json(
    report: &MergeReport,
    label: &str,
    owner: &Owner,
    operation: crate::io::git::VcsOperation,
    logged: &Logged,
    dry_run: bool,
) -> crate::cli::output::MergeJson {
    let preview = matches!(logged, Logged::WouldBeAt(_));
    let conflicts = report
        .conflicts
        .iter()
        .map(|c| crate::cli::output::MergeConflictJson {
            key: bare_key(&c.key).to_string(),
            // Only a `#` key is an ID. A `~` key is a title, and a sigil-less
            // one names no task at all — a consumer that fed either to
            // `fr merge --resolve` would be told there was nothing to clear.
            task: c.key.strip_prefix('#').map(|id| id.to_string()),
            reason: c.reason.slug(),
            description: c.reason.describe(),
            marker_written: marker_written(report.kind, &c.key, preview),
            theirs: c.theirs.clone(),
        })
        .collect();

    crate::cli::output::MergeJson {
        command: "merge",
        outcome: "conflict",
        exit: EXIT_CONFLICT,
        dry_run,
        path: label.to_string(),
        kind: Some(report.kind.label()),
        operation: operation.slug(),
        took_theirs: report.took_theirs,
        deleted: report.deleted,
        took_their_shell: report.took_their_shell,
        conflicts,
        // A preview recorded nothing, and says so — but the losing side is not
        // *lost*, because nothing was written over either. `NoProject` is the
        // case that means the version in this document is the only copy.
        recorded: matches!(logged, Logged::At(_)),
        recovery_log: match logged {
            Logged::At(path) | Logged::WouldBeAt(path) => Some(path.display().to_string()),
            Logged::NoProject { .. } => None,
        },
        project: owner_root(owner),
    }
}

fn owner_root(owner: &Owner) -> Option<String> {
    match owner {
        Owner::Found(root) => Some(root.display().to_string()),
        Owner::None { .. } => None,
    }
}
