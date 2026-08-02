use std::collections::HashSet;
use std::path::Path;

use chrono;
use serde::Serialize;

use crate::model::project::Project;
use crate::model::task::{Metadata, Task, TaskState};
use crate::model::track::{Track, TrackNode};

/// Structured result from `fr check`, suitable for --json output.
#[derive(Debug, Default, Serialize)]
pub struct CheckResult {
    pub valid: bool,
    pub errors: Vec<CheckError>,
    pub warnings: Vec<CheckWarning>,
    pub info: Vec<CheckInfo>,
}

/// A validation error (something that should be fixed).
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type")]
pub enum CheckError {
    /// A dep references a task ID that doesn't exist anywhere
    #[serde(rename = "dangling_dep")]
    DanglingDep {
        track_id: String,
        task_id: String,
        dep_id: String,
    },
    /// A `ref:` path doesn't exist on disk
    #[serde(rename = "broken_ref")]
    BrokenRef {
        track_id: String,
        task_id: String,
        path: String,
    },
    /// A `spec:` path doesn't exist on disk
    #[serde(rename = "broken_spec")]
    BrokenSpec {
        track_id: String,
        task_id: String,
        path: String,
    },
    /// Duplicate task ID found
    #[serde(rename = "duplicate_id")]
    DuplicateId {
        task_id: String,
        track_ids: Vec<String>,
    },
}

/// A validation warning (non-critical issue).
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type")]
pub enum CheckWarning {
    /// Task has no ID assigned
    #[serde(rename = "missing_id")]
    MissingId { track_id: String, title: String },
    /// Task has no `added:` date
    #[serde(rename = "missing_added_date")]
    MissingAddedDate { track_id: String, task_id: String },
    /// Done task has no `resolved:` date
    #[serde(rename = "missing_resolved_date")]
    MissingResolvedDate { track_id: String, task_id: String },
    /// A **top-level** task is not in the section its state calls for — a done
    /// task in `## Backlog`, a parked one in `## Done`, and so on.
    ///
    /// Generalised from a done-in-Backlog-only check, which covered one of the
    /// six possible misplacements. The other five were reachable: three separate
    /// defects in frame's own transition tables produced them (`12c0b57`,
    /// `5eb069f`, `62e253c`), and none of them was reported. Every misplacement
    /// found so far was made by frame rather than by hand, which is what this is
    /// really guarding.
    ///
    /// **Top-level only.** A subtask lives inside its parent and has no section
    /// of its own; a finished subtask under an unfinished parent in `## Backlog`
    /// is the normal shape. The old check inherited its parent's section and
    /// reported exactly that, a warning nothing could act on — `fr clean`
    /// reconciles top-level tasks only, so it never went away.
    #[serde(rename = "task_in_wrong_section")]
    TaskInWrongSection {
        track_id: String,
        task_id: String,
        /// Where the task's state says it belongs.
        expected: crate::model::track::SectionKind,
        /// Where it actually is.
        actual: crate::model::track::SectionKind,
    },
    /// Task has the #lost tag (created by recovery system)
    #[serde(rename = "lost_task")]
    LostTask { track_id: String, task_id: String },
    /// A subtask's ID does not extend its parent's — e.g. `BAC-207` nested under
    /// `BAC-153`. The ID no longer says where the task lives, and the parent's
    /// child-number scan cannot see it, so a later subtask can be handed a number
    /// this one already occupies in spirit.
    ///
    /// Reported only when both IDs match the grammar: a `Raw` ID is preserved
    /// verbatim by design and carries no parent/child relationship to break.
    ///
    /// Historically produced by `fr clean` itself, which resolved a duplicated
    /// *subtask* ID by minting a top-level number for it.
    #[serde(rename = "child_id_not_under_parent")]
    ChildIdNotUnderParent {
        track_id: String,
        task_id: String,
        parent_id: String,
    },
    /// This clone's `.actor` token has no row in `actors.toml` (registry drift —
    /// the committed registry lost our claim). The next mint re-registers it.
    #[serde(rename = "actor_token_unregistered")]
    ActorTokenUnregistered { token: String },
    /// This clone's `.actor` token is present but retired in `actors.toml`, yet
    /// this working copy still holds it. Claim a fresh token, or `fr actor set`
    /// it to reactivate.
    #[serde(rename = "actor_token_retired")]
    ActorTokenRetiredButHeld { token: String },
    /// Several active tokens share one provenance `name` (typically a hostname) —
    /// a sign a machine has accumulated tokens (e.g. a git-worktree-per-session
    /// workflow). Consider `fr actor merge` to collapse them.
    #[serde(rename = "actor_name_collision")]
    ActorNameCollision { name: String, tokens: Vec<String> },
    /// A working-copy-local frame file (see
    /// [`crate::io::project_io::LOCAL_ONLY_FRAME_FILES`]) is committed to git, or
    /// isn't covered by `.gitignore` and so will be. `path` is repo-relative.
    #[serde(rename = "local_file_committed")]
    LocalFileCommitted { path: String, tracked: bool },
    /// A task note leaves a code fence open. Frame parses the note correctly
    /// either way — note extent is bound by indentation, not fence state — but
    /// markdown renderers will swallow the rest of the file into a code block.
    #[serde(rename = "unclosed_note_fence")]
    UnclosedNoteFence {
        track_id: String,
        /// `None` for a task that has no ID yet; `title` identifies it instead.
        task_id: Option<String>,
        title: String,
        /// The unclosed opening fence, trimmed (e.g. ```` ```rust ````).
        fence: String,
    },
    /// A line frame could not attribute to any task: mis-indented prose, a
    /// metadata key that lost its colon, a subtask fragment left by a bad merge.
    /// It is preserved verbatim on every write and carried with the task it
    /// precedes, but frame does not understand it — it is not a note, and
    /// nothing but this warning will ever mention it.
    ///
    /// Worth reporting because the alternative was silence. These lines used to
    /// be dropped at parse time and deleted by the next write, which is how a
    /// `fr clean` run destroyed a note line in a track the user had not touched.
    /// Preserving them fixed the deletion; reporting them is what lets someone
    /// fix the indentation instead of carrying the line forever.
    ///
    /// Not repairable automatically: where the line was meant to go is a guess,
    /// and guessing wrong rewrites prose.
    #[serde(rename = "stranded_line")]
    StrandedLine {
        track_id: String,
        /// The task the line sits above — `None` if that task has no ID yet.
        before_task_id: Option<String>,
        /// The title of that task, which identifies it when the ID is absent.
        before_title: String,
        /// The line itself, trimmed.
        line: String,
    },
    /// A **live** task holding an ID an **archived** task already has: the number
    /// was reissued after the original left the live track.
    ///
    /// Neither the duplicate-ID error nor `fr clean`'s duplicate resolution sees
    /// this — both compare live tracks only — and minting used to as well, which
    /// is how a number could be reissued silently. The durable frontier in
    /// [`crate::io::ids`] closed that; this catches what happened before it, and
    /// anything hand-edited since. There is no automatic repair: renumbering a
    /// live task rewrites an ID other work may already reference.
    #[serde(rename = "id_reissued_after_archive")]
    IdReissuedAfterArchive {
        task_id: String,
        /// Live track(s) holding it.
        tracks: Vec<String>,
        /// Archive paths also holding it.
        archives: Vec<String>,
    },
    /// One task ID appearing more than once **inside the archives**, with no live
    /// task involved: the same task's history was written twice, not a number
    /// handed out twice.
    ///
    /// `fr clean` appends to an archive *before* removing the tasks from the
    /// track, so nothing is lost if the second write doesn't land — but until
    /// that append became idempotent, a lost track update meant the next clean
    /// appended the same batch again. Deduplicating is safe and manual: the
    /// copies are historical records, and nothing but this check reads them.
    #[serde(rename = "duplicate_archived_id")]
    DuplicateArchivedId {
        task_id: String,
        /// How many times the ID appears across the archives.
        total: usize,
        /// The distinct archive paths involved (one path when a single file holds
        /// every copy, which is the usual shape).
        archives: Vec<String>,
    },
    /// The durable ID frontier store exists but doesn't parse. The next mint
    /// moves it aside and falls back to scanning, which cannot see another
    /// worktree's uncommitted tasks — so IDs can collide until it refills.
    #[serde(rename = "id_frontier_unreadable")]
    IdFrontierUnreadable { path: String, detail: String },
    /// A `.bak` beside the frontier store: it was unreadable once and got reset.
    /// Numbers minted in that window may have been reissued. Informational —
    /// deleting the `.bak` clears it.
    #[serde(rename = "id_frontier_was_reset")]
    IdFrontierWasReset { path: String },
    /// An inbox item body leaves a code fence open. Same rendering hazard as
    /// [`CheckWarning::UnclosedNoteFence`].
    #[serde(rename = "unclosed_inbox_fence")]
    UnclosedInboxFence {
        /// 1-based index, matching `fr inbox` and `fr triage`.
        index: usize,
        title: String,
        /// The unclosed opening fence, trimmed.
        fence: String,
    },
    /// A multi-file operation started and did not finish — an
    /// [`crate::io::inflight`] marker is still in place.
    ///
    /// Normally the next write command completes it and clears the marker, so
    /// seeing this means either nothing has been written since, or recovery
    /// declined to act because a precondition no longer held (a hand edit, a
    /// `git checkout` in between). The recovery log has the detail.
    #[serde(rename = "interrupted_operation")]
    InterruptedOperation {
        /// The operation name, e.g. `mv --track`.
        operation: String,
        /// The command as it was run.
        command: String,
        /// RFC3339 timestamp of when it started.
        started: String,
    },
}

/// Informational messages (not errors or warnings).
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type")]
pub enum CheckInfo {
    /// Recovery log summary
    #[serde(rename = "recovery_log")]
    RecoveryLog { entry_count: usize, oldest: String },
}

// ---------------------------------------------------------------------------
// Main check entry point
// ---------------------------------------------------------------------------

/// Validate a project and return structured results.
///
/// This is a read-only operation — it does not modify the project.
///
/// Checks performed:
/// 1. All `dep:` references resolve to existing task IDs
/// 2. All `ref:` paths exist on disk
/// 3. All `spec:` paths exist on disk (section fragment stripped)
/// 4. No duplicate task IDs
/// 5. Warnings for missing IDs, dates, misplaced tasks
pub fn check_project(project: &Project) -> CheckResult {
    let mut result = CheckResult::default();

    // Collect all task IDs for dep validation and duplicate detection
    let all_ids = collect_all_task_ids(project);
    let duplicates = find_duplicate_ids(project);

    for (task_id, track_ids) in &duplicates {
        result.errors.push(CheckError::DuplicateId {
            task_id: task_id.clone(),
            track_ids: track_ids.clone(),
        });
    }

    for (track_id, track) in &project.tracks {
        check_track(track, track_id, &all_ids, &project.root, &mut result);
    }

    // Actor-registry drift: this clone's `.actor` token should have an active
    // row in the committed `actors.toml`.
    check_actor_registry(&project.frame_dir, &mut result);

    // Actor proliferation: several active tokens under one provenance name.
    check_actor_name_collisions(&project.frame_dir, &mut result);

    // Working-copy-local frame files leaking into git.
    check_local_files_ignored(&project.frame_dir, &mut result);

    // Numbers handed out twice, where one holder is archived (invisible to the
    // live-tracks-only duplicate check above).
    check_archived_id_collisions(project, &mut result);

    // The durable ID frontier store: unreadable, or reset at some point.
    check_id_frontier(&project.frame_dir, &mut result);
    check_inflight(&project.frame_dir, &mut result);

    // Inbox item bodies that leave a code fence open.
    if let Some(ref inbox) = project.inbox {
        check_inbox(inbox, &mut result);
    }

    // Recovery log summary
    if let Some(summary) = crate::io::recovery::recovery_summary(&project.frame_dir) {
        let oldest_str = summary
            .oldest
            .map(|ts| ts.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
            .unwrap_or_default();
        result.info.push(CheckInfo::RecoveryLog {
            entry_count: summary.entry_count,
            oldest: oldest_str,
        });
    }

    result.valid = result.errors.is_empty();
    result
}

// ---------------------------------------------------------------------------
// Per-track validation
// ---------------------------------------------------------------------------

fn check_track(
    track: &Track,
    track_id: &str,
    all_ids: &HashSet<String>,
    project_root: &Path,
    result: &mut CheckResult,
) {
    for node in &track.nodes {
        if let TrackNode::Section { kind, tasks, .. } = node {
            for task in tasks {
                check_task(task, None, track_id, *kind, all_ids, project_root, result);
            }
        }
    }
}

/// `parent` is the task this one is nested under, or `None` at top level — the
/// subtask-ID rule is the one check that a task cannot be judged against alone.
#[allow(clippy::too_many_arguments)]
fn check_task(
    task: &Task,
    parent: Option<&Task>,
    track_id: &str,
    section: crate::model::track::SectionKind,
    all_ids: &HashSet<String>,
    project_root: &Path,
    result: &mut CheckResult,
) {
    let task_id = task.id.as_deref().unwrap_or("");

    // Warning: missing ID
    if task.id.is_none() {
        result.warnings.push(CheckWarning::MissingId {
            track_id: track_id.to_string(),
            title: task.title.clone(),
        });
    }

    // Warning: missing added date
    let has_added = task
        .metadata
        .iter()
        .any(|m| matches!(m, Metadata::Added(_)));
    if !has_added && task.id.is_some() {
        result.warnings.push(CheckWarning::MissingAddedDate {
            track_id: track_id.to_string(),
            task_id: task_id.to_string(),
        });
    }

    // Warning: done task missing resolved date
    if task.state == TaskState::Done {
        let has_resolved = task
            .metadata
            .iter()
            .any(|m| matches!(m, Metadata::Resolved(_)));
        if !has_resolved {
            result.warnings.push(CheckWarning::MissingResolvedDate {
                track_id: track_id.to_string(),
                task_id: task_id.to_string(),
            });
        }
    }

    // Warning: a top-level task in a section its state does not call for.
    // `parent.is_none()` is the top-level test — a subtask has no section of its
    // own and simply inherits the one passed down this recursion.
    if parent.is_none() && task.id.is_some() {
        let expected = crate::ops::task_ops::canonical_section(task.state);
        if expected != section {
            result.warnings.push(CheckWarning::TaskInWrongSection {
                track_id: track_id.to_string(),
                task_id: task_id.to_string(),
                expected,
                actual: section,
            });
        }
    }

    // Warning: lines frame could not attribute to any task, held ahead of this
    // one. Blanks inside such a run are carried too — they are formatting, so
    // only the content lines are reported.
    for line in task.leading_lines.iter().filter(|l| !l.trim().is_empty()) {
        result.warnings.push(CheckWarning::StrandedLine {
            track_id: track_id.to_string(),
            before_task_id: task.id.as_ref().map(|id| id.to_string()),
            before_title: task.title.clone(),
            line: line.trim().to_string(),
        });
    }

    // Warning: a subtask whose ID does not extend its parent's.
    if let Some(id) = task.id.as_ref()
        && let Some(parent_id) = parent.and_then(|p| p.id.as_ref())
        && id.is_structured()
        && parent_id.is_structured()
        && !id.is_child_of(parent_id)
    {
        result.warnings.push(CheckWarning::ChildIdNotUnderParent {
            track_id: track_id.to_string(),
            task_id: id.to_string(),
            parent_id: parent_id.to_string(),
        });
    }

    // Warning: lost task (from recovery system)
    if task.tags.iter().any(|t| t == "lost") && task.id.is_some() {
        result.warnings.push(CheckWarning::LostTask {
            track_id: track_id.to_string(),
            task_id: task_id.to_string(),
        });
    }

    // Check metadata
    for meta in &task.metadata {
        match meta {
            Metadata::Dep(deps) => {
                for dep_id in deps {
                    if !all_ids.contains(dep_id) {
                        result.errors.push(CheckError::DanglingDep {
                            track_id: track_id.to_string(),
                            task_id: task_id.to_string(),
                            dep_id: dep_id.clone(),
                        });
                    }
                }
            }
            Metadata::Ref(refs) => {
                for r in refs {
                    if !project_root.join(r).exists() {
                        result.errors.push(CheckError::BrokenRef {
                            track_id: track_id.to_string(),
                            task_id: task_id.to_string(),
                            path: r.clone(),
                        });
                    }
                }
            }
            Metadata::Spec(spec) => {
                let file_path = spec.split('#').next().unwrap_or(spec);
                if !project_root.join(file_path).exists() {
                    result.errors.push(CheckError::BrokenSpec {
                        track_id: track_id.to_string(),
                        task_id: task_id.to_string(),
                        path: spec.clone(),
                    });
                }
            }
            Metadata::Note(note) => {
                if let Some(fence) = unclosed_fence(note) {
                    result.warnings.push(CheckWarning::UnclosedNoteFence {
                        track_id: track_id.to_string(),
                        task_id: task.id.as_ref().map(|id| id.to_string()),
                        title: task.title.clone(),
                        fence,
                    });
                }
            }
            _ => {}
        }
    }

    // Recurse into subtasks
    for sub in &task.subtasks {
        check_task(
            sub,
            Some(task),
            track_id,
            section,
            all_ids,
            project_root,
            result,
        );
    }
}

// ---------------------------------------------------------------------------
// Inbox validation
// ---------------------------------------------------------------------------

fn check_inbox(inbox: &crate::model::inbox::Inbox, result: &mut CheckResult) {
    for (i, item) in inbox.items.iter().enumerate() {
        if let Some(ref body) = item.body
            && let Some(fence) = unclosed_fence(body)
        {
            result.warnings.push(CheckWarning::UnclosedInboxFence {
                index: i + 1,
                title: item.title.clone(),
                fence,
            });
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Find a fenced code block left open at the end of `body`, returning its
/// opening fence line (trimmed), or `None` if every fence is closed.
///
/// Follows CommonMark: an opener is 3+ backticks optionally followed by an info
/// string (which may not itself contain a backtick); a closer is 3+ backticks —
/// at least as many as the opener — followed by nothing but whitespace. So
/// ```` ```rust ```` *cannot* close a block, it is content inside one.
///
/// Frame's own parsers are indifferent to fence balance: note and inbox-body
/// extents are bound by indentation alone. Markdown renderers are not — an
/// unclosed fence swallows the remainder of the document into a code block,
/// which is why this is worth a warning even though the file parses correctly.
///
/// Tilde fences (`~~~`) are not considered; frame has only ever special-cased
/// backticks.
pub(crate) fn unclosed_fence(body: &str) -> Option<String> {
    let mut open: Option<(usize, String)> = None;

    for line in body.lines() {
        let trimmed = line.trim();
        // Backticks are ASCII, so this char count doubles as a byte offset.
        let ticks = trimmed.chars().take_while(|c| *c == '`').count();
        if ticks < 3 {
            continue;
        }
        let rest = &trimmed[ticks..];

        match open {
            None => {
                // An info string containing a backtick disqualifies the opener.
                if !rest.contains('`') {
                    open = Some((ticks, trimmed.to_string()));
                }
            }
            Some((open_ticks, _)) => {
                if ticks >= open_ticks && rest.trim().is_empty() {
                    open = None;
                }
            }
        }
    }

    open.map(|(_, fence)| fence)
}

/// Compare this clone's `.actor` token against the committed registry. A held
/// token that is missing from, or retired in, `actors.toml` is drift worth
/// flagging. An unclaimed clone (no `.actor`) and an unreadable registry are
/// both no-ops here — the latter is a separate concern surfaced elsewhere.
fn check_actor_registry(frame_dir: &Path, result: &mut CheckResult) {
    let Some(token) = crate::io::actors::read_actor_token(frame_dir) else {
        return;
    };
    let Ok(reg) = crate::io::actors::read_actors(frame_dir) else {
        return;
    };
    match reg.actors.get(&token) {
        None => result
            .warnings
            .push(CheckWarning::ActorTokenUnregistered { token }),
        Some(entry) if entry.is_retired() => result
            .warnings
            .push(CheckWarning::ActorTokenRetiredButHeld { token }),
        Some(_) => {}
    }
}

/// Flag provenance names that back more than one *active* token. Same-name
/// active tokens usually mean one machine auto-claimed several (e.g. a fresh git
/// worktree per session) — proliferation worth collapsing with `fr actor merge`.
/// Retired tokens are ignored (they're already tombstoned). An unreadable
/// registry is a no-op.
fn check_actor_name_collisions(frame_dir: &Path, result: &mut CheckResult) {
    let Ok(reg) = crate::io::actors::read_actors(frame_dir) else {
        return;
    };
    // Group active tokens by provenance name, preserving registry order.
    let mut by_name: Vec<(String, Vec<String>)> = Vec::new();
    for (token, entry) in &reg.actors {
        if !entry.is_active() {
            continue;
        }
        match by_name.iter_mut().find(|(n, _)| n == &entry.name) {
            Some((_, tokens)) => tokens.push(token.clone()),
            None => by_name.push((entry.name.clone(), vec![token.clone()])),
        }
    }
    for (name, tokens) in by_name {
        if tokens.len() > 1 {
            result
                .warnings
                .push(CheckWarning::ActorNameCollision { name, tokens });
        }
    }
}

/// Flag working-copy-local frame files that git is carrying, or is about to.
///
/// `fr init` writes these to `.gitignore`, but only at init — a project created
/// before an entry existed never gets it, and nothing notices until the file is
/// committed (or conflicts on a merge, which the append-only recovery log does
/// reliably). Two states are worth flagging, strongest first:
///
/// - **tracked**: already in the index, so ignore rules no longer apply. Needs
///   `git rm --cached` as well as a `.gitignore` line.
/// - **not ignored** (and present on disk): the next `git add -A` commits it.
///
/// A project outside git, or one where `git` is unavailable, is a no-op.
fn check_local_files_ignored(frame_dir: &Path, result: &mut CheckResult) {
    let Some(paths) = crate::io::git::repo_paths(frame_dir) else {
        return;
    };
    // Everything git is asked about must be repo-relative, so the same strings
    // can be matched against its output and shown in the fix hints.
    let Ok(frame_rel) = frame_dir
        .canonicalize()
        .as_deref()
        .unwrap_or(frame_dir)
        .strip_prefix(&paths.toplevel)
        .map(|p| p.to_path_buf())
    else {
        return;
    };
    let rel_paths: Vec<String> = crate::io::project_io::LOCAL_ONLY_FRAME_FILES
        .iter()
        .map(|name| frame_rel.join(name).to_string_lossy().into_owned())
        .collect();

    let tracked = crate::io::git::tracked_paths(&paths.toplevel, &rel_paths).unwrap_or_default();
    let Some(ignored) = crate::io::git::ignored_paths(&paths.toplevel, &rel_paths) else {
        return;
    };

    for (rel, name) in rel_paths
        .iter()
        .zip(crate::io::project_io::LOCAL_ONLY_FRAME_FILES)
    {
        // A directory entry is tracked when anything *under* it is: git
        // ls-files reports `frame/.rescue/main.md`, never `frame/.rescue`, so an
        // equality test would call a committed rescue directory untracked and
        // hand out the wrong remedy.
        let dir_prefix = format!("{rel}/");
        if tracked
            .iter()
            .any(|t| t == rel || t.starts_with(&dir_prefix))
        {
            result.warnings.push(CheckWarning::LocalFileCommitted {
                path: rel.clone(),
                tracked: true,
            });
        } else if !ignored.iter().any(|i| i == rel)
            && (frame_dir.join(name).exists() || is_transient(name))
        {
            // Not ignored but not yet committed: normally only worth reporting
            // for a file that actually exists, since an absent one can't be
            // added — `.ids.toml` never appears inside git at all, and warning
            // about it here would be pure noise.
            //
            // A transient file is the exception. It exists only in a window
            // nobody is watching, so an existence check almost never catches it,
            // and `fr check --fix` never can: it recovers the interrupted
            // operation first, which removes the marker before the repair plan
            // is computed. Left as-is, a project created before the marker
            // existed could never acquire its `.gitignore` line.
            result.warnings.push(CheckWarning::LocalFileCommitted {
                path: rel.clone(),
                tracked: false,
            });
        }
    }
}

fn collect_all_task_ids(project: &Project) -> HashSet<String> {
    let mut ids = HashSet::new();
    for (_, track) in &project.tracks {
        for node in &track.nodes {
            if let TrackNode::Section { tasks, .. } = node {
                collect_ids_from_tasks(tasks, &mut ids);
            }
        }
    }
    ids
}

fn collect_ids_from_tasks(tasks: &[Task], ids: &mut HashSet<String>) {
    for task in tasks {
        if let Some(ref id) = task.id {
            ids.insert(id.to_string());
        }
        collect_ids_from_tasks(&task.subtasks, ids);
    }
}

/// Find task IDs that appear more than once (within or across tracks).
fn find_duplicate_ids(project: &Project) -> Vec<(String, Vec<String>)> {
    use std::collections::HashMap;
    // id → list of track_ids where it appears (including repeats for within-track dups)
    let mut id_to_tracks: HashMap<String, Vec<String>> = HashMap::new();

    for (track_id, track) in &project.tracks {
        for node in &track.nodes {
            if let TrackNode::Section { tasks, .. } = node {
                collect_id_locations(tasks, track_id, &mut id_to_tracks);
            }
        }
    }

    id_to_tracks
        .into_iter()
        .filter(|(_, tracks)| tracks.len() > 1)
        .collect()
}

// ---------------------------------------------------------------------------
// ID frontier and archive collisions
// ---------------------------------------------------------------------------

/// Flag task IDs involving an archive more than once, split by which of two very
/// different problems it is: a number **reissued** while an archived task still
/// holds it, or the same task's history **duplicated** inside the archives.
///
/// [`find_duplicate_ids`] compares live tracks against each other, and so does
/// `fr clean`'s duplicate resolution — anything an archive holds is invisible to
/// both.
fn check_archived_id_collisions(project: &Project, result: &mut CheckResult) {
    use std::collections::HashMap;

    let mut live: HashMap<String, Vec<String>> = HashMap::new();
    for (track_id, track) in &project.tracks {
        for node in &track.nodes {
            if let TrackNode::Section { tasks, .. } = node {
                collect_id_locations(tasks, track_id, &mut live);
            }
        }
    }

    let mut archived: HashMap<String, Vec<String>> = HashMap::new();
    for (label, tasks) in archived_task_lists(&project.frame_dir) {
        collect_id_locations(&tasks, &label, &mut archived);
    }

    // Only IDs an archive holds are reported here; a live-only duplicate is
    // already a `DuplicateId` error.
    let mut ids: Vec<&String> = archived.keys().collect();
    ids.sort();
    for id in ids {
        let in_archives = &archived[id];
        let in_tracks = live.get(id).cloned().unwrap_or_default();

        if !in_tracks.is_empty() {
            // A live task and an archived one share the number.
            result.warnings.push(CheckWarning::IdReissuedAfterArchive {
                task_id: id.clone(),
                tracks: in_tracks,
                archives: dedup_sorted(in_archives),
            });
        } else if in_archives.len() > 1 {
            // Archives only: the same task was archived more than once. The
            // repeated path is the point, so report the count and the distinct
            // files rather than the same name twice.
            result.warnings.push(CheckWarning::DuplicateArchivedId {
                task_id: id.clone(),
                total: in_archives.len(),
                archives: dedup_sorted(in_archives),
            });
        }
    }
}

/// The distinct entries of `labels`, sorted — several copies in one archive file
/// collapse to that one path.
fn dedup_sorted(labels: &[String]) -> Vec<String> {
    let mut out: Vec<String> = labels.to_vec();
    out.sort();
    out.dedup();
    out
}

/// Every archived task list, each labelled by the path it came from:
/// `archive/<track>.md` for done-task archives, `archive/_tracks/<track>.md` for
/// whole tracks that were archived. Unreadable files contribute nothing.
fn archived_task_lists(frame_dir: &Path) -> Vec<(String, Vec<Task>)> {
    let mut out = Vec::new();

    if let Ok(archives) = crate::io::project_io::load_archives(frame_dir) {
        for (track_id, tasks) in archives {
            out.push((format!("archive/{}.md", track_id), tasks));
        }
    }

    let whole_tracks = frame_dir.join("archive").join("_tracks");
    if let Ok(entries) = std::fs::read_dir(&whole_tracks) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("md") {
                continue;
            }
            let Ok(content) = std::fs::read_to_string(&path) else {
                continue;
            };
            let track = crate::parse::parse_track(&content);
            let mut tasks = Vec::new();
            for node in &track.nodes {
                if let TrackNode::Section { tasks: section, .. } = node {
                    tasks.extend(section.iter().cloned());
                }
            }
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            out.push((format!("archive/_tracks/{}", name), tasks));
        }
    }

    out
}

/// Report a frontier store that can't be read, or one that was reset earlier.
/// Read-only: unlike a mint, this leaves an unreadable store in place so the
/// warning is actionable while the file is still there.
/// Whether a working-copy-local file exists only briefly, rather than being
/// created once and kept.
///
/// Only the in-flight marker qualifies: it is written at the start of a
/// multi-file operation and removed at the end, so at any given moment it is
/// almost certainly absent. Everything else in
/// [`crate::io::project_io::LOCAL_ONLY_FRAME_FILES`] appears on first use and
/// stays, which is what makes an existence check the right gate for them.
fn is_transient(name: &str) -> bool {
    name == crate::io::inflight::MARKER_FILE
}

/// Report an operation that started and did not finish.
///
/// Read-only, like everything else here: the marker is left in place. Completing
/// it is the next write command's job (`crate::ops::recover`), which is also what
/// clears it.
fn check_inflight(frame_dir: &Path, result: &mut CheckResult) {
    if let Some(marker) = crate::io::inflight::read(frame_dir) {
        result.warnings.push(CheckWarning::InterruptedOperation {
            operation: marker.operation.name().to_string(),
            command: marker.command,
            started: marker.started,
        });
    }
}

fn check_id_frontier(frame_dir: &Path, result: &mut CheckResult) {
    let health = crate::io::ids::health(frame_dir);

    if let crate::io::ids::StoreState::Unparsable(detail) = &health.state {
        result.warnings.push(CheckWarning::IdFrontierUnreadable {
            path: health.path.display().to_string(),
            // TOML errors span several lines; the first carries the location.
            detail: detail.lines().next().unwrap_or_default().trim().to_string(),
        });
    }

    if let Some(backup) = &health.reset_backup {
        result.warnings.push(CheckWarning::IdFrontierWasReset {
            path: backup.display().to_string(),
        });
    }
}

fn collect_id_locations(
    tasks: &[Task],
    track_id: &str,
    id_to_tracks: &mut std::collections::HashMap<String, Vec<String>>,
) {
    for task in tasks {
        if let Some(ref id) = task.id {
            id_to_tracks
                .entry(id.to_string())
                .or_default()
                .push(track_id.to_string());
        }
        collect_id_locations(&task.subtasks, track_id, id_to_tracks);
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::config::{
        AgentConfig, CleanConfig, IdConfig, ProjectConfig, ProjectInfo, TrackConfig, UiConfig,
    };
    use crate::parse::parse_track;
    use indexmap::IndexMap;
    use tempfile::TempDir;

    fn make_config() -> ProjectConfig {
        ProjectConfig {
            project: ProjectInfo {
                name: "test".to_string(),
            },
            agent: AgentConfig::default(),
            tracks: vec![TrackConfig {
                id: "main".to_string(),
                name: "Main".to_string(),
                state: "active".to_string(),
                file: "tracks/main.md".to_string(),
            }],
            clean: CleanConfig::default(),
            ids: IdConfig {
                prefixes: IndexMap::new(),
            },
            ui: UiConfig::default(),
        }
    }

    fn make_project_at(root: &Path, track_src: &str) -> Project {
        let track = parse_track(track_src);
        Project {
            root: root.to_path_buf(),
            frame_dir: root.join("frame"),
            config: make_config(),
            tracks: vec![("main".to_string(), track)],
            inbox: None,
        }
    }

    // --- Archived-ID collisions and the ID frontier ---

    /// (task_id, tracks, archives) for each reissued-number warning.
    fn reissue_warnings(result: &CheckResult) -> Vec<(String, Vec<String>, Vec<String>)> {
        result
            .warnings
            .iter()
            .filter_map(|w| match w {
                CheckWarning::IdReissuedAfterArchive {
                    task_id,
                    tracks,
                    archives,
                } => Some((task_id.clone(), tracks.clone(), archives.clone())),
                _ => None,
            })
            .collect()
    }

    /// (task_id, total, archives) for each duplicated-archive-entry warning.
    fn duplicate_archive_warnings(result: &CheckResult) -> Vec<(String, usize, Vec<String>)> {
        result
            .warnings
            .iter()
            .filter_map(|w| match w {
                CheckWarning::DuplicateArchivedId {
                    task_id,
                    total,
                    archives,
                } => Some((task_id.clone(), *total, archives.clone())),
                _ => None,
            })
            .collect()
    }

    /// A live task holding a number an archived task already has: the number was
    /// reissued after the original was archived.
    #[test]
    fn test_check_live_id_colliding_with_an_archived_one() {
        let tmp = TempDir::new().unwrap();
        let frame_dir = tmp.path().join("frame");
        std::fs::create_dir_all(frame_dir.join("archive")).unwrap();
        std::fs::write(
            frame_dir.join("archive/main.md"),
            "# Archive \u{2014} main\n\n- [x] `M-005` archived work\n  - resolved: 2025-06-01\n",
        )
        .unwrap();

        let project = make_project_at(
            tmp.path(),
            "# Main\n\n## Backlog\n\n- [ ] `M-005` reissued\n  - added: 2025-07-01\n\n## Done\n",
        );
        let result = check_project(&project);
        assert_eq!(
            reissue_warnings(&result),
            vec![(
                "M-005".to_string(),
                vec!["main".to_string()],
                vec!["archive/main.md".to_string()]
            )]
        );
        // Not the duplicated-history problem.
        assert!(duplicate_archive_warnings(&result).is_empty());
    }

    /// The same task archived twice into one file — duplicated history, not a
    /// reissued number. The repeated path collapses to one entry with a count,
    /// rather than being listed twice as if two different files held it.
    #[test]
    fn test_check_duplicate_entry_within_one_archive() {
        let tmp = TempDir::new().unwrap();
        let frame_dir = tmp.path().join("frame");
        std::fs::create_dir_all(frame_dir.join("archive")).unwrap();
        std::fs::write(
            frame_dir.join("archive/main.md"),
            "# Archive \u{2014} main\n\n- [x] `M-007` done\n  - resolved: 2025-06-01\n- [x] `M-007` done\n  - resolved: 2025-06-01\n",
        )
        .unwrap();

        let project = make_project_at(tmp.path(), "# Main\n\n## Backlog\n\n## Done\n");
        let result = check_project(&project);
        assert_eq!(
            duplicate_archive_warnings(&result),
            vec![("M-007".to_string(), 2, vec!["archive/main.md".to_string()])]
        );
        // No live task is involved, so nothing was reissued.
        assert!(reissue_warnings(&result).is_empty());
    }

    /// One ID across two different archive files: still duplicated history, and
    /// both paths are named.
    #[test]
    fn test_check_duplicate_entry_across_two_archives() {
        let tmp = TempDir::new().unwrap();
        let frame_dir = tmp.path().join("frame");
        std::fs::create_dir_all(frame_dir.join("archive/_tracks")).unwrap();
        std::fs::write(
            frame_dir.join("archive/main.md"),
            "# Archive \u{2014} main\n\n- [x] `M-007` done once\n",
        )
        .unwrap();
        std::fs::write(
            frame_dir.join("archive/_tracks/old.md"),
            "# Old\n\n## Backlog\n\n- [ ] `M-007` same number\n\n## Done\n",
        )
        .unwrap();

        let project = make_project_at(tmp.path(), "# Main\n\n## Backlog\n\n## Done\n");
        let warnings = duplicate_archive_warnings(&check_project(&project));
        assert_eq!(
            warnings,
            vec![(
                "M-007".to_string(),
                2,
                vec![
                    "archive/_tracks/old.md".to_string(),
                    "archive/main.md".to_string()
                ]
            )]
        );
    }

    /// Distinct namespaces are distinct IDs: `M-a5` archived does not collide
    /// with a live `M-005`.
    #[test]
    fn test_check_archived_id_in_another_namespace_is_not_a_collision() {
        let tmp = TempDir::new().unwrap();
        let frame_dir = tmp.path().join("frame");
        std::fs::create_dir_all(frame_dir.join("archive")).unwrap();
        std::fs::write(
            frame_dir.join("archive/main.md"),
            "# Archive \u{2014} main\n\n- [x] `M-a5` another actor\n",
        )
        .unwrap();

        let project = make_project_at(
            tmp.path(),
            "# Main\n\n## Backlog\n\n- [ ] `M-005` mine\n\n## Done\n",
        );
        let result = check_project(&project);
        assert!(reissue_warnings(&result).is_empty());
        assert!(duplicate_archive_warnings(&result).is_empty());
    }

    /// An archive holding a number nothing else uses is just history.
    #[test]
    fn test_check_archive_without_collision_is_silent() {
        let tmp = TempDir::new().unwrap();
        let frame_dir = tmp.path().join("frame");
        std::fs::create_dir_all(frame_dir.join("archive")).unwrap();
        std::fs::write(
            frame_dir.join("archive/main.md"),
            "# Archive \u{2014} main\n\n- [x] `M-002` old work\n",
        )
        .unwrap();

        let project = make_project_at(
            tmp.path(),
            "# Main\n\n## Backlog\n\n- [ ] `M-009` current\n\n## Done\n",
        );
        let result = check_project(&project);
        assert!(reissue_warnings(&result).is_empty());
        assert!(duplicate_archive_warnings(&result).is_empty());
    }

    fn frontier_warnings(result: &CheckResult) -> Vec<String> {
        result
            .warnings
            .iter()
            .filter_map(|w| match w {
                CheckWarning::IdFrontierUnreadable { path, .. } => {
                    Some(format!("unreadable {path}"))
                }
                CheckWarning::IdFrontierWasReset { path } => Some(format!("reset {path}")),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn test_check_flags_an_unreadable_id_frontier() {
        let tmp = TempDir::new().unwrap();
        let frame_dir = tmp.path().join("frame");
        std::fs::create_dir_all(&frame_dir).unwrap();
        let store = crate::io::ids::locate(&frame_dir).data;
        std::fs::write(&store, "not toml {{{").unwrap();

        let project = make_project_at(tmp.path(), "# Main\n\n## Backlog\n\n## Done\n");
        let warnings = frontier_warnings(&check_project(&project));
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].starts_with("unreadable"));
        // Reporting must not reset it — the fix is to the file check named.
        assert!(store.is_file());
    }

    #[test]
    fn test_check_flags_a_reset_id_frontier() {
        let tmp = TempDir::new().unwrap();
        let frame_dir = tmp.path().join("frame");
        std::fs::create_dir_all(&frame_dir).unwrap();
        let store = crate::io::ids::locate(&frame_dir).data;
        std::fs::write(store.with_extension("toml.bak"), "old\n").unwrap();

        let project = make_project_at(tmp.path(), "# Main\n\n## Backlog\n\n## Done\n");
        let warnings = frontier_warnings(&check_project(&project));
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].starts_with("reset"));
    }

    #[test]
    fn test_check_healthy_id_frontier_is_silent() {
        let tmp = TempDir::new().unwrap();
        let frame_dir = tmp.path().join("frame");
        std::fs::create_dir_all(&frame_dir).unwrap();
        crate::io::ids::reserve(&frame_dir, "M", None, 0, 1);

        let project = make_project_at(tmp.path(), "# Main\n\n## Backlog\n\n## Done\n");
        assert!(frontier_warnings(&check_project(&project)).is_empty());
    }

    // --- Local-only files leaking into git ---

    /// Run git in `cwd`, reporting success. Tests skip themselves when git is
    /// unavailable rather than failing.
    fn git(cwd: &Path, args: &[&str]) -> bool {
        std::process::Command::new("git")
            .current_dir(cwd)
            .args(args)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    /// A git repo containing a frame project, with `gitignore` as its
    /// `.gitignore` and every local-only file present on disk. `None` when git
    /// is unavailable.
    fn repo_with_local_files(root: &Path, gitignore: &str) -> Option<Project> {
        if !git(root, &["init", "-q"]) {
            return None;
        }
        let frame_dir = root.join("frame");
        std::fs::create_dir_all(&frame_dir).unwrap();
        std::fs::write(root.join(".gitignore"), gitignore).unwrap();
        for name in crate::io::project_io::LOCAL_ONLY_FRAME_FILES {
            std::fs::write(frame_dir.join(name), "local\n").unwrap();
        }
        Some(make_project_at(root, "# Main\n\n## Backlog\n\n## Done\n"))
    }

    /// A `.gitignore` covering every local-only frame file except `omit` —
    /// derived from the one list, so adding an entry there can't silently make
    /// these tests weaker.
    fn gitignore_without(omit: &str) -> String {
        crate::io::project_io::LOCAL_ONLY_FRAME_FILES
            .iter()
            .filter(|name| **name != omit)
            .map(|name| format!("frame/{}\n", name))
            .collect()
    }

    fn local_file_warnings(result: &CheckResult) -> Vec<(String, bool)> {
        result
            .warnings
            .iter()
            .filter_map(|w| match w {
                CheckWarning::LocalFileCommitted { path, tracked } => {
                    Some((path.clone(), *tracked))
                }
                _ => None,
            })
            .collect()
    }

    /// The lace case: a `.gitignore` written before `.recovery.log` joined the
    /// list, so that one file is unignored and will be committed.
    #[test]
    fn test_check_local_file_not_ignored() {
        let tmp = TempDir::new().unwrap();
        let Some(project) = repo_with_local_files(tmp.path(), &gitignore_without(".recovery.log"))
        else {
            return; // git unavailable
        };

        let warnings = local_file_warnings(&check_project(&project));
        assert_eq!(
            warnings,
            vec![("frame/.recovery.log".to_string(), false)],
            "only the unignored file should be flagged"
        );
    }

    /// Once the file is committed, ignore rules no longer apply — it needs
    /// `git rm --cached`, so it's flagged as tracked even if .gitignore lists it.
    #[test]
    fn test_check_local_file_tracked() {
        let tmp = TempDir::new().unwrap();
        let Some(project) = repo_with_local_files(tmp.path(), &gitignore_without(".recovery.log"))
        else {
            return;
        };
        if !git(tmp.path(), &["add", "frame/.recovery.log"]) {
            return;
        }
        // Belatedly ignoring a tracked file does not untrack it.
        std::fs::write(tmp.path().join(".gitignore"), gitignore_without("")).unwrap();

        let warnings = local_file_warnings(&check_project(&project));
        assert_eq!(warnings, vec![("frame/.recovery.log".to_string(), true)]);
    }

    /// A project whose .gitignore covers every local-only file is silent.
    #[test]
    fn test_check_local_files_all_ignored() {
        let tmp = TempDir::new().unwrap();
        let ignore: String = crate::io::project_io::LOCAL_ONLY_FRAME_FILES
            .iter()
            .map(|n| format!("frame/{}\n", n))
            .collect();
        let Some(project) = repo_with_local_files(tmp.path(), &ignore) else {
            return;
        };
        assert!(local_file_warnings(&check_project(&project)).is_empty());
    }

    /// Outside a git repo there is nothing to leak into, so the check is a no-op.
    #[test]
    fn test_check_local_files_non_git_project_is_silent() {
        let tmp = TempDir::new().unwrap();
        let frame_dir = tmp.path().join("frame");
        std::fs::create_dir_all(&frame_dir).unwrap();
        for name in crate::io::project_io::LOCAL_ONLY_FRAME_FILES {
            std::fs::write(frame_dir.join(name), "local\n").unwrap();
        }
        let project = make_project_at(tmp.path(), "# Main\n\n## Backlog\n\n## Done\n");
        // Guard against the tempdir itself sitting inside a repo.
        if crate::io::git::repo_paths(&frame_dir).is_none() {
            assert!(local_file_warnings(&check_project(&project)).is_empty());
        }
    }

    // --- Actor name collision (proliferation) ---

    #[test]
    fn test_check_actor_name_collision() {
        use crate::io::actors::{ActorRegistry, write_actors};
        let tmp = TempDir::new().unwrap();
        let project = make_project_at(tmp.path(), "# Main\n\n## Backlog\n\n## Done\n");
        std::fs::create_dir_all(&project.frame_dir).unwrap();

        let mut reg = ActorRegistry::default();
        reg.claim("null", "Mac", None, "2026-06-01").unwrap();
        reg.claim("b", "ccdev", None, "2026-06-02").unwrap();
        reg.claim("d", "ccdev", None, "2026-06-03").unwrap();
        reg.claim("f", "ccdev", None, "2026-06-04").unwrap();
        reg.retire("f", "2026-06-05").unwrap(); // retired: excluded from collision
        write_actors(&project.frame_dir, &reg).unwrap();

        let collisions: Vec<_> = check_project(&project)
            .warnings
            .into_iter()
            .filter_map(|w| match w {
                CheckWarning::ActorNameCollision { name, tokens } => Some((name, tokens)),
                _ => None,
            })
            .collect();
        // Only 'ccdev' collides (b, d); the retired 'f' and the lone 'Mac' don't.
        assert_eq!(collisions.len(), 1);
        assert_eq!(collisions[0].0, "ccdev");
        assert_eq!(collisions[0].1, vec!["b".to_string(), "d".to_string()]);
    }

    // --- Dangling deps ---

    #[test]
    fn test_check_dangling_dep() {
        let tmp = TempDir::new().unwrap();
        let project = make_project_at(
            tmp.path(),
            "\
# Main

## Backlog

- [ ] `M-001` Task one
  - added: 2025-05-01
  - dep: NONEXIST-999

## Done
",
        );

        let result = check_project(&project);
        assert!(!result.valid);
        assert_eq!(result.errors.len(), 1);
        assert!(matches!(
            &result.errors[0],
            CheckError::DanglingDep { dep_id, .. } if dep_id == "NONEXIST-999"
        ));
    }

    #[test]
    fn test_check_valid_dep() {
        let tmp = TempDir::new().unwrap();
        let project = make_project_at(
            tmp.path(),
            "\
# Main

## Backlog

- [ ] `M-001` Task one
  - added: 2025-05-01
  - dep: M-002
- [ ] `M-002` Task two
  - added: 2025-05-01

## Done
",
        );

        let result = check_project(&project);
        assert!(result.valid);
        let dangling: Vec<_> = result
            .errors
            .iter()
            .filter(|e| matches!(e, CheckError::DanglingDep { .. }))
            .collect();
        assert!(dangling.is_empty());
    }

    #[test]
    fn test_check_cross_track_dep() {
        let tmp = TempDir::new().unwrap();
        let track_a = parse_track(
            "\
# A

## Backlog

- [ ] `A-001` Task A
  - added: 2025-05-01
  - dep: B-001

## Done
",
        );
        let track_b = parse_track(
            "\
# B

## Backlog

- [ ] `B-001` Task B
  - added: 2025-05-01

## Done
",
        );
        let mut config = make_config();
        config.tracks = vec![
            TrackConfig {
                id: "a".to_string(),
                name: "A".to_string(),
                state: "active".to_string(),
                file: "a.md".to_string(),
            },
            TrackConfig {
                id: "b".to_string(),
                name: "B".to_string(),
                state: "active".to_string(),
                file: "b.md".to_string(),
            },
        ];

        let project = Project {
            root: tmp.path().to_path_buf(),
            frame_dir: tmp.path().join("frame"),
            config,
            tracks: vec![("a".to_string(), track_a), ("b".to_string(), track_b)],
            inbox: None,
        };

        let result = check_project(&project);
        assert!(result.valid);
    }

    // --- Broken refs ---

    #[test]
    fn test_check_broken_ref() {
        let tmp = TempDir::new().unwrap();
        let project = make_project_at(
            tmp.path(),
            "\
# Main

## Backlog

- [ ] `M-001` Task one
  - added: 2025-05-01
  - ref: nonexistent/file.md

## Done
",
        );

        let result = check_project(&project);
        assert!(!result.valid);
        assert!(matches!(
            &result.errors[0],
            CheckError::BrokenRef { path, .. } if path == "nonexistent/file.md"
        ));
    }

    #[test]
    fn test_check_valid_ref() {
        let tmp = TempDir::new().unwrap();
        std::fs::write(tmp.path().join("doc.md"), "content").unwrap();

        let project = make_project_at(
            tmp.path(),
            "\
# Main

## Backlog

- [ ] `M-001` Task one
  - added: 2025-05-01
  - ref: doc.md

## Done
",
        );

        let result = check_project(&project);
        let broken: Vec<_> = result
            .errors
            .iter()
            .filter(|e| matches!(e, CheckError::BrokenRef { .. }))
            .collect();
        assert!(broken.is_empty());
    }

    // --- Broken spec ---

    #[test]
    fn test_check_broken_spec() {
        let tmp = TempDir::new().unwrap();
        let project = make_project_at(
            tmp.path(),
            "\
# Main

## Backlog

- [ ] `M-001` Task one
  - added: 2025-05-01
  - spec: missing/spec.md#section

## Done
",
        );

        let result = check_project(&project);
        assert!(!result.valid);
        assert!(matches!(
            &result.errors[0],
            CheckError::BrokenSpec { path, .. } if path == "missing/spec.md#section"
        ));
    }

    #[test]
    fn test_check_valid_spec_with_section() {
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join("doc")).unwrap();
        std::fs::write(tmp.path().join("doc/spec.md"), "# Section\ncontent").unwrap();

        let project = make_project_at(
            tmp.path(),
            "\
# Main

## Backlog

- [ ] `M-001` Task one
  - added: 2025-05-01
  - spec: doc/spec.md#section

## Done
",
        );

        let result = check_project(&project);
        let broken: Vec<_> = result
            .errors
            .iter()
            .filter(|e| matches!(e, CheckError::BrokenSpec { .. }))
            .collect();
        assert!(broken.is_empty());
    }

    // --- Duplicate IDs ---

    #[test]
    fn test_check_duplicate_ids() {
        let tmp = TempDir::new().unwrap();
        let track_a = parse_track(
            "\
# A

## Backlog

- [ ] `DUP-001` Task in A
  - added: 2025-05-01

## Done
",
        );
        let track_b = parse_track(
            "\
# B

## Backlog

- [ ] `DUP-001` Same ID in B
  - added: 2025-05-01

## Done
",
        );
        let mut config = make_config();
        config.tracks = vec![
            TrackConfig {
                id: "a".to_string(),
                name: "A".to_string(),
                state: "active".to_string(),
                file: "a.md".to_string(),
            },
            TrackConfig {
                id: "b".to_string(),
                name: "B".to_string(),
                state: "active".to_string(),
                file: "b.md".to_string(),
            },
        ];

        let project = Project {
            root: tmp.path().to_path_buf(),
            frame_dir: tmp.path().join("frame"),
            config,
            tracks: vec![("a".to_string(), track_a), ("b".to_string(), track_b)],
            inbox: None,
        };

        let result = check_project(&project);
        assert!(!result.valid);
        assert!(
            result.errors.iter().any(
                |e| matches!(e, CheckError::DuplicateId { task_id, .. } if task_id == "DUP-001")
            )
        );
    }

    #[test]
    fn test_check_duplicate_ids_within_track() {
        let tmp = TempDir::new().unwrap();
        let project = make_project_at(
            tmp.path(),
            "\
# Main

## Backlog

- [ ] `M-001` First occurrence
  - added: 2025-05-01
- [ ] `M-001` Same ID in same track
  - added: 2025-05-02

## Done
",
        );

        let result = check_project(&project);
        assert!(!result.valid);
        assert!(
            result.errors.iter().any(
                |e| matches!(e, CheckError::DuplicateId { task_id, track_ids } if task_id == "M-001" && track_ids.len() == 2)
            )
        );
    }

    // --- Subtask IDs that escape their parent ---

    /// (task_id, parent_id) for each misparented-subtask warning.
    fn misparented(result: &CheckResult) -> Vec<(String, String)> {
        result
            .warnings
            .iter()
            .filter_map(|w| match w {
                CheckWarning::ChildIdNotUnderParent {
                    task_id, parent_id, ..
                } => Some((task_id.clone(), parent_id.clone())),
                _ => None,
            })
            .collect()
    }

    /// The shape `fr clean` used to produce when it resolved a duplicated
    /// subtask ID by minting a top-level number for it.
    #[test]
    fn test_check_reports_a_subtask_holding_a_top_level_id() {
        let tmp = TempDir::new().unwrap();
        let project = make_project_at(
            tmp.path(),
            "\
# Main

## Backlog

- [ ] `M-001` Parent
  - added: 2025-05-01
  - [ ] `M-007` Escaped
    - added: 2025-05-01

## Done
",
        );

        assert_eq!(
            misparented(&check_project(&project)),
            vec![("M-007".to_string(), "M-001".to_string())]
        );
    }

    /// A subtask under the wrong parent entirely — one branch's ID nested in
    /// another's, which a bad three-way merge can produce.
    #[test]
    fn test_check_reports_a_subtask_under_the_wrong_parent() {
        let tmp = TempDir::new().unwrap();
        let project = make_project_at(
            tmp.path(),
            "\
# Main

## Backlog

- [ ] `M-001` One
  - added: 2025-05-01
- [ ] `M-002` Two
  - added: 2025-05-01
  - [ ] `M-001.3` Belongs to M-001
    - added: 2025-05-01

## Done
",
        );

        assert_eq!(
            misparented(&check_project(&project)),
            vec![("M-001.3".to_string(), "M-002".to_string())]
        );
    }

    /// Depth is not the rule — an ID two segments below its parent skips a level.
    #[test]
    fn test_check_reports_a_grandchild_id_on_a_child() {
        let tmp = TempDir::new().unwrap();
        let project = make_project_at(
            tmp.path(),
            "\
# Main

## Backlog

- [ ] `M-001` Parent
  - added: 2025-05-01
  - [ ] `M-001.1.1` Too deep
    - added: 2025-05-01

## Done
",
        );

        assert_eq!(
            misparented(&check_project(&project)),
            vec![("M-001.1.1".to_string(), "M-001".to_string())]
        );
    }

    /// A well-formed hierarchy, including a subtask minted by another clone —
    /// a different namespace on the last segment is still a child.
    #[test]
    fn test_check_accepts_well_formed_subtask_ids() {
        let tmp = TempDir::new().unwrap();
        let project = make_project_at(
            tmp.path(),
            "\
# Main

## Backlog

- [ ] `M-001` Parent
  - added: 2025-05-01
  - [ ] `M-001.1` Ours
    - added: 2025-05-01
  - [ ] `M-001.b2` Theirs
    - added: 2025-05-01
    - [ ] `M-001.b2.1` Deep
      - added: 2025-05-01

## Done
",
        );

        assert!(misparented(&check_project(&project)).is_empty());
    }

    /// IDs that don't match the grammar are preserved verbatim by design and
    /// carry no parent/child relationship, so they are never reported.
    #[test]
    fn test_check_leaves_raw_ids_alone() {
        let tmp = TempDir::new().unwrap();
        let project = make_project_at(
            tmp.path(),
            "\
# Main

## Backlog

- [ ] `legacy/thing` Parent
  - added: 2025-05-01
  - [ ] `M-001.1` Structured child of a raw parent
    - added: 2025-05-01
- [ ] `M-002` Structured parent
  - added: 2025-05-01
  - [ ] `whatever` Raw child
    - added: 2025-05-01

## Done
",
        );

        assert!(misparented(&check_project(&project)).is_empty());
    }

    // --- Warnings ---

    #[test]
    fn test_warn_missing_id() {
        let tmp = TempDir::new().unwrap();
        let project = make_project_at(
            tmp.path(),
            "\
# Main

## Backlog

- [ ] Task without ID

## Done
",
        );

        let result = check_project(&project);
        assert!(result.warnings.iter().any(|w| matches!(
            w,
            CheckWarning::MissingId { title, .. } if title == "Task without ID"
        )));
    }

    #[test]
    fn test_warn_missing_added_date() {
        let tmp = TempDir::new().unwrap();
        let project = make_project_at(
            tmp.path(),
            "\
# Main

## Backlog

- [ ] `M-001` Task without added date

## Done
",
        );

        let result = check_project(&project);
        assert!(result.warnings.iter().any(|w| matches!(
            w,
            CheckWarning::MissingAddedDate { task_id, .. } if task_id == "M-001"
        )));
    }

    #[test]
    fn test_warn_missing_resolved_date() {
        let tmp = TempDir::new().unwrap();
        let project = make_project_at(
            tmp.path(),
            "\
# Main

## Backlog

## Done

- [x] `M-001` Done task without resolved
  - added: 2025-05-01
",
        );

        let result = check_project(&project);
        assert!(result.warnings.iter().any(|w| matches!(
            w,
            CheckWarning::MissingResolvedDate { task_id, .. } if task_id == "M-001"
        )));
    }

    #[test]
    fn test_warn_done_in_backlog() {
        let tmp = TempDir::new().unwrap();
        let project = make_project_at(
            tmp.path(),
            "\
# Main

## Backlog

- [x] `M-001` Done task in backlog
  - added: 2025-05-01
  - resolved: 2025-05-10

## Done
",
        );

        let result = check_project(&project);
        assert!(result.warnings.iter().any(|w| matches!(
            w,
            CheckWarning::TaskInWrongSection { task_id, expected, actual, .. }
                if task_id == "M-001"
                    && *expected == crate::model::track::SectionKind::Done
                    && *actual == crate::model::track::SectionKind::Backlog
        )));
    }

    // --- Lost task warning ---

    #[test]
    fn test_warn_lost_task() {
        let tmp = TempDir::new().unwrap();
        let project = make_project_at(
            tmp.path(),
            "\
# Main

## Backlog

- [!] `M-001` Recovered task #lost
  - added: 2025-05-01

## Done
",
        );

        let result = check_project(&project);
        assert!(result.warnings.iter().any(|w| matches!(
            w,
            CheckWarning::LostTask { task_id, .. } if task_id == "M-001"
        )));
    }

    // --- Unclosed code fence warnings ---

    /// `unclosed_fence` follows CommonMark, so a fence with an info string is not
    /// a closer. These are the cases that decide false positives either way.
    #[test]
    fn test_unclosed_fence_detection() {
        // Balanced: opener with info string, bare closer.
        assert_eq!(unclosed_fence("```rust\nfn main() {}\n```"), None);
        // Three markers, but the middle one has an info string so it is content
        // inside the block, not a closer — the trailing bare fence closes it.
        // This is the shape from the original bug report; it renders fine.
        assert_eq!(unclosed_fence("```lace\n```rust\n```"), None);
        // Prose with no fences at all.
        assert_eq!(unclosed_fence("just some prose\nover two lines"), None);
        // Inline code spans are not fences.
        assert_eq!(unclosed_fence("call `foo()` then `bar()`"), None);
        // A longer closer is valid; a shorter one is not.
        assert_eq!(unclosed_fence("```\ncode\n`````"), None);
        assert_eq!(unclosed_fence("`````\ncode\n```").as_deref(), Some("`````"));

        // Unclosed: a single bare fence.
        assert_eq!(unclosed_fence("Example:\n```").as_deref(), Some("```"));
        // Unclosed: opener with info string, never closed.
        assert_eq!(
            unclosed_fence("```rust\nfn main() {}").as_deref(),
            Some("```rust")
        );
        // Reopened after a clean pair, then left open.
        assert_eq!(
            unclosed_fence("```\na\n```\nprose\n```py").as_deref(),
            Some("```py")
        );
    }

    #[test]
    fn test_warn_unclosed_note_fence() {
        let tmp = TempDir::new().unwrap();
        let project = make_project_at(
            tmp.path(),
            "\
# Main

## Backlog

- [ ] `M-001` Task with an unclosed fence
  - added: 2025-05-01
  - note:
    Example:
    ```rust
    fn main() {}

## Done
",
        );

        let result = check_project(&project);
        assert!(result.warnings.iter().any(|w| matches!(
            w,
            CheckWarning::UnclosedNoteFence { task_id, fence, .. }
                if task_id.as_deref() == Some("M-001") && fence == "```rust"
        )));
        // A rendering nit, not a structural problem.
        assert!(result.valid);
    }

    /// The shape from the bug report parses *and* renders correctly, so it must
    /// not warn — otherwise the fix trades corruption for a false alarm.
    #[test]
    fn test_no_fence_warning_for_balanced_note() {
        let tmp = TempDir::new().unwrap();
        let project = make_project_at(
            tmp.path(),
            "\
# Main

## Backlog

- [ ] `M-001` Task with balanced fences
  - added: 2025-05-01
  - note:
    §13.4 mentions three fence kinds:
      ```lace
      ```rust
      ```
    Check the spec.

## Done
",
        );

        let result = check_project(&project);
        assert!(
            !result
                .warnings
                .iter()
                .any(|w| matches!(w, CheckWarning::UnclosedNoteFence { .. }))
        );
    }

    /// A line the parser could not attribute to any task is now carried instead
    /// of deleted — which means someone has to be told it is there, or it stays
    /// misfiled forever and frame keeps not reading it.
    #[test]
    fn test_warn_stranded_line() {
        let tmp = TempDir::new().unwrap();
        let project = make_project_at(
            tmp.path(),
            "\
# Main

## Done

- [x] `M-001` Sharded map lowering
  - resolved: 2026-07-20
    **Shape.** A line that lost its indent.
- [x] `M-002` Next task
  - resolved: 2026-07-21
",
        );

        let result = check_project(&project);
        let stranded: Vec<_> = result
            .warnings
            .iter()
            .filter_map(|w| match w {
                CheckWarning::StrandedLine {
                    before_task_id,
                    line,
                    ..
                } => Some((before_task_id.clone(), line.clone())),
                _ => None,
            })
            .collect();

        // Reported against the task it sits above — M-002, not the M-001 block
        // it visually hangs off. Where it *belongs* is exactly what frame does
        // not know; where it *is* is what the user needs to find it.
        assert_eq!(
            stranded,
            vec![(
                Some("M-002".to_string()),
                "**Shape.** A line that lost its indent.".to_string()
            )]
        );
        // Nothing structural is broken — the file still parses and still writes
        // back unchanged.
        assert!(result.valid);
    }

    /// The ordinary case must stay quiet: every line here is attributable, so a
    /// well-formed track must produce no stranded-line warning at all.
    #[test]
    fn test_no_stranded_warning_for_a_clean_track() {
        let tmp = TempDir::new().unwrap();
        let project = make_project_at(
            tmp.path(),
            "\
# Main

## Backlog

- [ ] `M-001` Task
  - added: 2025-05-01
  - note:
    A note with
    two lines.
  - [ ] `M-001.1` Subtask

## Done
",
        );

        let result = check_project(&project);
        assert!(
            !result
                .warnings
                .iter()
                .any(|w| matches!(w, CheckWarning::StrandedLine { .. }))
        );
    }

    #[test]
    fn test_warn_unclosed_note_fence_on_task_without_id() {
        let tmp = TempDir::new().unwrap();
        let project = make_project_at(
            tmp.path(),
            "\
# Main

## Backlog

- [ ] Unnamed task
  - note:
    ```

## Done
",
        );

        let result = check_project(&project);
        assert!(result.warnings.iter().any(|w| matches!(
            w,
            CheckWarning::UnclosedNoteFence { task_id, title, .. }
                if task_id.is_none() && title == "Unnamed task"
        )));
    }

    #[test]
    fn test_warn_unclosed_inbox_fence() {
        let tmp = TempDir::new().unwrap();
        let mut project = make_project_at(tmp.path(), "# Main\n\n## Backlog\n\n## Done\n");
        let (inbox, _) = crate::parse::parse_inbox(
            "\
# Inbox

- Balanced item
  ```
  code
  ```

- Item with an unclosed fence
  Example:
  ```py

- Trailing item
",
        );
        project.inbox = Some(inbox);

        let result = check_project(&project);
        let fence_warnings: Vec<_> = result
            .warnings
            .iter()
            .filter_map(|w| match w {
                CheckWarning::UnclosedInboxFence { index, fence, .. } => Some((*index, fence)),
                _ => None,
            })
            .collect();
        // Only the middle item warns, and it is reported by its 1-based index.
        assert_eq!(fence_warnings.len(), 1);
        assert_eq!(fence_warnings[0].0, 2);
        assert_eq!(fence_warnings[0].1, "```py");
    }

    #[test]
    fn test_no_lost_warning_without_tag() {
        let tmp = TempDir::new().unwrap();
        let project = make_project_at(
            tmp.path(),
            "\
# Main

## Backlog

- [ ] `M-001` Normal task #core
  - added: 2025-05-01

## Done
",
        );

        let result = check_project(&project);
        assert!(
            !result
                .warnings
                .iter()
                .any(|w| matches!(w, CheckWarning::LostTask { .. }))
        );
    }

    #[test]
    fn test_lost_task_no_id_no_warning() {
        let tmp = TempDir::new().unwrap();
        let project = make_project_at(
            tmp.path(),
            "\
# Main

## Backlog

- [ ] Task without ID #lost

## Done
",
        );

        let result = check_project(&project);
        // Should have MissingId warning but NOT LostTask (no ID to report)
        assert!(
            result
                .warnings
                .iter()
                .any(|w| matches!(w, CheckWarning::MissingId { .. }))
        );
        assert!(
            !result
                .warnings
                .iter()
                .any(|w| matches!(w, CheckWarning::LostTask { .. }))
        );
    }

    // --- Recovery log info ---

    #[test]
    fn test_check_recovery_log_info() {
        let tmp = TempDir::new().unwrap();
        let frame_dir = tmp.path().join("frame");
        std::fs::create_dir_all(&frame_dir).unwrap();

        // Write a recovery log entry
        crate::io::recovery::log_recovery(
            &frame_dir,
            crate::io::recovery::RecoveryEntry {
                timestamp: chrono::Utc::now(),
                category: crate::io::recovery::RecoveryCategory::Write,
                description: "test write failure".to_string(),
                fields: vec![],
                body: "lost content".to_string(),
            },
        );

        let project = make_project_at(
            tmp.path(),
            "\
# Main

## Backlog

- [ ] `M-001` Task
  - added: 2025-05-01

## Done
",
        );

        let result = check_project(&project);
        assert!(result.info.iter().any(|i| matches!(
            i,
            CheckInfo::RecoveryLog { entry_count, .. } if *entry_count == 1
        )));
    }

    #[test]
    fn test_check_no_recovery_log_info() {
        let tmp = TempDir::new().unwrap();
        let project = make_project_at(
            tmp.path(),
            "\
# Main

## Backlog

- [ ] `M-001` Task
  - added: 2025-05-01

## Done
",
        );

        let result = check_project(&project);
        assert!(result.info.is_empty());
    }

    // --- Clean project ---

    #[test]
    fn test_check_clean_project() {
        let tmp = TempDir::new().unwrap();
        let project = make_project_at(
            tmp.path(),
            "\
# Main

## Backlog

- [ ] `M-001` Well-formed task
  - added: 2025-05-01
- [>] `M-002` Another good task
  - added: 2025-05-02

## Done

- [x] `M-000` Completed task
  - added: 2025-04-01
  - resolved: 2025-05-01
",
        );

        let result = check_project(&project);
        assert!(result.valid);
        assert!(result.errors.is_empty());
        assert!(result.warnings.is_empty());
    }

    // --- Subtask checks ---

    #[test]
    fn test_check_subtask_dangling_dep() {
        let tmp = TempDir::new().unwrap();
        let project = make_project_at(
            tmp.path(),
            "\
# Main

## Backlog

- [ ] `M-001` Parent
  - added: 2025-05-01
  - [ ] `M-001.1` Sub with bad dep
    - added: 2025-05-01
    - dep: GONE-999

## Done
",
        );

        let result = check_project(&project);
        assert!(!result.valid);
        assert!(matches!(
            &result.errors[0],
            CheckError::DanglingDep { task_id, dep_id, .. }
                if task_id == "M-001.1" && dep_id == "GONE-999"
        ));
    }

    // --- Actor-token namespaces ---
    //
    // Duplicate detection and dep resolution operate on `TaskId`'s canonical
    // text form (the `id.to_string()` keys in `collect_all_task_ids` /
    // `find_duplicate_ids`), so distinct token namespaces are distinct ids and a
    // genuine same-namespace collision still rides the existing duplicate report.

    /// Keystone safety-net: the three duplicate cases under tokened ids.
    /// `EFF-a14`/`EFF-a14` collide (same namespace), but `EFF-a14`/`EFF-14`
    /// (token vs null) and `EFF-a14`/`EFF-b14` (different tokens) do not.
    #[test]
    fn test_check_duplicate_token_namespaces() {
        let tmp = TempDir::new().unwrap();
        let project = make_project_at(
            tmp.path(),
            "\
# Main

## Backlog

- [ ] `EFF-a14` Same namespace, occurrence one
  - added: 2025-05-01
- [ ] `EFF-a14` Same namespace, occurrence two
  - added: 2025-05-02
- [ ] `EFF-14` Null namespace, not a dup of a14
  - added: 2025-05-03
- [ ] `EFF-b14` Different token, not a dup of a14
  - added: 2025-05-04

## Done
",
        );

        let result = check_project(&project);

        // Exactly the same-namespace pair is reported.
        let dups: Vec<&String> = result
            .errors
            .iter()
            .filter_map(|e| match e {
                CheckError::DuplicateId { task_id, .. } => Some(task_id),
                _ => None,
            })
            .collect();
        assert_eq!(dups, vec![&"EFF-a14".to_string()]);
        // The cross-namespace ids are NOT flagged as duplicates.
        assert!(!dups.iter().any(|id| *id == "EFF-14"));
        assert!(!dups.iter().any(|id| *id == "EFF-b14"));
    }

    /// A healthy project mixing null and tokened ids, with deps that cross
    /// namespaces, validates clean.
    #[test]
    fn test_check_clean_tokened_project() {
        let tmp = TempDir::new().unwrap();
        let project = make_project_at(
            tmp.path(),
            "\
# Main

## Backlog

- [ ] `M-001` Null-namespace task
  - added: 2025-05-01
- [ ] `M-a1` Tokened task depending on a null id
  - added: 2025-05-02
  - dep: M-001
- [ ] `M-b2` Another tokened task depending on a tokened id
  - added: 2025-05-03
  - dep: M-a1
  - [ ] `M-b2.c1` Tokened subtask depending on a null id
    - added: 2025-05-04
    - dep: M-001

## Done
",
        );

        let result = check_project(&project);
        assert!(result.valid, "expected valid, got {:?}", result.errors);
        assert!(result.errors.is_empty());
    }

    /// A dep on a non-existent tokened id is dangling; a same-prefix id in a
    /// different namespace does not satisfy it.
    #[test]
    fn test_check_dangling_dep_tokened() {
        let tmp = TempDir::new().unwrap();
        let project = make_project_at(
            tmp.path(),
            "\
# Main

## Backlog

- [ ] `M-014` A null-namespace task
  - added: 2025-05-01
- [ ] `M-a1` Depends on a tokened id that does not exist
  - added: 2025-05-02
  - dep: M-a14

## Done
",
        );

        let result = check_project(&project);
        assert!(!result.valid);
        // `M-a14` is dangling even though the null-namespace `M-014` exists.
        assert!(result.errors.iter().any(|e| matches!(
            e,
            CheckError::DanglingDep { dep_id, .. } if dep_id == "M-a14"
        )));
    }

    // --- Actor registry drift ---

    /// A clone holding a token absent from `actors.toml` is flagged (the bug
    /// where a concurrent clone clobbered the committed registry).
    #[test]
    fn test_check_actor_token_unregistered() {
        let tmp = TempDir::new().unwrap();
        let frame_dir = tmp.path().join("frame");
        std::fs::create_dir_all(&frame_dir).unwrap();
        crate::io::actors::write_actor_token(&frame_dir, "null").unwrap();
        // Registry exists but lists only another clone's token.
        std::fs::write(
            frame_dir.join("actors.toml"),
            "[actors.b]\nname = \"ccdev\"\nstate = \"active\"\n",
        )
        .unwrap();

        let project = make_project_at(tmp.path(), "# Main\n\n## Backlog\n\n## Done\n");
        let result = check_project(&project);
        assert!(result.warnings.iter().any(|w| matches!(
            w,
            CheckWarning::ActorTokenUnregistered { token } if token == "null"
        )));
    }

    /// A clone still holding a token the registry has retired is flagged.
    #[test]
    fn test_check_actor_token_retired_but_held() {
        let tmp = TempDir::new().unwrap();
        let frame_dir = tmp.path().join("frame");
        std::fs::create_dir_all(&frame_dir).unwrap();
        crate::io::actors::write_actor_token(&frame_dir, "a").unwrap();
        std::fs::write(
            frame_dir.join("actors.toml"),
            "[actors.a]\nname = \"host\"\nstate = \"retired\"\nretired = \"2026-06-27\"\n",
        )
        .unwrap();

        let project = make_project_at(tmp.path(), "# Main\n\n## Backlog\n\n## Done\n");
        let result = check_project(&project);
        assert!(result.warnings.iter().any(|w| matches!(
            w,
            CheckWarning::ActorTokenRetiredButHeld { token } if token == "a"
        )));
    }

    /// A clone whose active token is properly registered raises no actor warning,
    /// and an unclaimed clone (no `.actor`) is silent too.
    #[test]
    fn test_check_actor_token_healthy_and_unclaimed() {
        // Healthy: token present and active.
        let tmp = TempDir::new().unwrap();
        let frame_dir = tmp.path().join("frame");
        std::fs::create_dir_all(&frame_dir).unwrap();
        crate::io::actors::write_actor_token(&frame_dir, "a").unwrap();
        std::fs::write(
            frame_dir.join("actors.toml"),
            "[actors.a]\nname = \"host\"\nstate = \"active\"\nclaimed = \"2026-06-27\"\n",
        )
        .unwrap();
        let project = make_project_at(tmp.path(), "# Main\n\n## Backlog\n\n## Done\n");
        let result = check_project(&project);
        assert!(!result.warnings.iter().any(|w| matches!(
            w,
            CheckWarning::ActorTokenUnregistered { .. }
                | CheckWarning::ActorTokenRetiredButHeld { .. }
        )));

        // Unclaimed: no `.actor` at all — nothing to compare, no warning.
        let tmp2 = TempDir::new().unwrap();
        std::fs::create_dir_all(tmp2.path().join("frame")).unwrap();
        let project2 = make_project_at(tmp2.path(), "# Main\n\n## Backlog\n\n## Done\n");
        let result2 = check_project(&project2);
        assert!(!result2.warnings.iter().any(|w| matches!(
            w,
            CheckWarning::ActorTokenUnregistered { .. }
                | CheckWarning::ActorTokenRetiredButHeld { .. }
        )));
    }

    // --- JSON serialization ---

    #[test]
    fn test_check_result_serializes_to_json() {
        let tmp = TempDir::new().unwrap();
        let project = make_project_at(
            tmp.path(),
            "\
# Main

## Backlog

- [ ] `M-001` Task
  - added: 2025-05-01
  - dep: GONE-001

## Done
",
        );

        let result = check_project(&project);
        let json = serde_json::to_string_pretty(&result).unwrap();
        assert!(json.contains("dangling_dep"));
        assert!(json.contains("GONE-001"));
    }
}
