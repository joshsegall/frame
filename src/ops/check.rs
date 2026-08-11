use std::collections::HashSet;
use std::path::Path;

use chrono;
use serde::Serialize;

use crate::model::project::Project;
use crate::model::task::{Metadata, Task, TaskState};
use crate::model::track::{Track, TrackNode};
use crate::ops::refs as refs_ops;

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
    /// A track has two or more sections of one kind — two `## Done`, say.
    ///
    /// **How it happens: a line-by-line git merge of a track file.** Found in a
    /// real project, introduced by a merge commit six weeks before frame's
    /// merge driver existed; both parents had one `## Done` and the merge
    /// produced two. Re-running that merge reproduces it exactly.
    ///
    /// **Why it is an error rather than cosmetic.** `Track::section_tasks`
    /// returns the *first* section of a kind, and roughly a hundred call sites
    /// are built on it — archiving, section reconciliation, the byte
    /// accounting, the TUI's section rendering. Everything in the second
    /// section is invisible to all of them while remaining findable by ID, so
    /// the file looks fine and 150 done tasks quietly stop being archivable.
    /// It also round-trips byte-identically, so it never heals on its own and
    /// never gets worse — it simply stays.
    ///
    /// Repaired by the next write, not by `--fix`: see
    /// [`crate::io::project_io::save_track`].
    #[serde(rename = "duplicate_section")]
    DuplicateSection {
        track_id: String,
        section: crate::model::track::SectionKind,
        count: usize,
        /// Tasks in the second and later sections.
        hidden_tasks: usize,
    },
    /// A `##` heading frame does not recognise, in a file frame owns.
    ///
    /// Reported even when nothing is behind it yet. In a track file an unknown
    /// heading does not merely get ignored: the parser sends it to literal
    /// text, and every task line after it goes the same way, until the next
    /// heading frame does know. So the heading is a trapdoor — anything written
    /// under it stops being a task. In an archive or the inbox, which have no
    /// sections at all, a heading below the title ends the task list and turns
    /// the remainder into trailing text.
    ///
    /// No automatic repair. Where the content was meant to go is a guess, and
    /// the heading may be deliberate; deleting it or promoting it are both
    /// decisions about someone else's writing.
    #[serde(rename = "unknown_section_heading")]
    UnknownSectionHeading {
        /// The track this is in, or the file name for an archive or the inbox.
        track_id: String,
        heading: String,
        stranded_tasks: usize,
    },
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
    /// A task still carries the `conflict:` marker `fr merge` left on it, so a
    /// merge kept one side and set the other aside without anyone deciding that
    /// was right.
    ///
    /// An **error**, not a warning, and the reason is what happens if it is
    /// ignored: the merge writes no conflict markers — deliberately, so the file
    /// stays valid frame markdown — which means staging the path commits our
    /// version and discards theirs with nothing left to say so. This is the only
    /// durable record that a decision is outstanding.
    ///
    /// **No `--fix`.** Which side should win is exactly the judgment a machine
    /// cannot make; the same reasoning as `IdReissuedAfterArchive`. Their
    /// version goes to the recovery log — apply what is missing with `fr note` /
    /// `fr state`, then clear the marker with `fr merge --resolve <ID>`.
    ///
    /// **Whether the log actually holds it is checked, not assumed.** The marker
    /// is written into the track file and is committed, so it travels to every
    /// clone; the recovery log is working-copy-local and does not. A marker
    /// pulled from someone else's merge therefore points at an entry this
    /// working copy has never seen, and telling the reader to go read it sends
    /// them looking for something that is not there. `evidence` records what a
    /// lookup found, so the message can say which case this is.
    #[serde(rename = "unresolved_merge_conflict")]
    UnresolvedMergeConflict {
        track_id: String,
        task_id: String,
        /// The marker's payload: a reason slug and the timestamp of the recovery
        /// entry holding the other version.
        detail: String,
        /// Whether a recovery entry matching this marker was found here.
        evidence: bool,
    },
    /// `project.toml` lists a track whose file is not where it says it is.
    ///
    /// `load_project` skips a configured track whose file is missing, so the
    /// track and every task in it silently leave the project: absent from `fr
    /// list`, from the TUI, and from every other check here, which can only see
    /// tracks that loaded. Nothing else reports it — that is what makes this an
    /// error rather than a warning.
    ///
    /// An archived track is expected to live at `archive/_tracks/<id>.md`
    /// instead, and is only reported when it is missing from *there*.
    ///
    /// **No `--fix`.** Both repairs guess: dropping the entry discards a track
    /// that may be one `git checkout` from returning, and recreating the file
    /// fabricates content.
    #[serde(rename = "track_file_missing")]
    TrackFileMissing {
        track_id: String,
        /// Where the file was expected, relative to `frame/`.
        path: String,
        /// The track's configured state, since it decides where to look.
        state: String,
    },
    /// A `.md` file in `tracks/` that no `[[tracks]]` entry references.
    ///
    /// The other direction of the same drift, and the same consequence: the
    /// file is real, its tasks are real, and nothing shows them. IDs inside it
    /// are also invisible to the duplicate-ID check, so a collision with a live
    /// track goes unreported until the file is wired back in.
    ///
    /// **No `--fix`.** Adopting the file means inventing an id, a name and an
    /// ID prefix, and when this is the far half of an interrupted rename the
    /// right answer is to restore the *original* entry rather than mint a
    /// second track beside the dangling one. Which of those it is, only the
    /// person who knows what they renamed can say — the same reasoning as
    /// `stranded_line` and `UnresolvedMergeConflict`.
    #[serde(rename = "track_file_unreferenced")]
    TrackFileUnreferenced {
        /// Path relative to `frame/`.
        path: String,
        /// The `# Title` the file carries, when it has one — the name to give
        /// the track if it is adopted.
        title: Option<String>,
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
    /// This clone does not have frame's merge driver registered, so git will
    /// merge track and inbox files line by line.
    ///
    /// Worth a warning because of *where* the registration lives. `.gitattributes`
    /// is committed and arrives with a clone; the driver itself is in
    /// `.git/config`, which is per-clone and cannot be committed. So a teammate
    /// who clones a correctly-configured project silently gets text merges — and
    /// a text merge duplicates any task a `fr done` relocated between sections,
    /// which is not obvious until `fr show` disagrees with the file.
    ///
    /// Nothing in the project is wrong, which is why this is a warning and has no
    /// `--fix`: the repair is `fr git setup`, and it writes machine state rather
    /// than project content.
    #[serde(rename = "merge_driver_unregistered")]
    MergeDriverUnregistered,
    /// `.gitattributes` does not actually route frame's files to the merge
    /// driver, so git will merge them line by line however the file reads.
    ///
    /// **Asked of git, not read off the file.** A pattern containing a slash is
    /// relative to the directory of the `.gitattributes` holding it, so a
    /// plausible-looking line can match nothing at all: `fr git setup` used to
    /// write `sub/frame/archive/*.md` into `sub/.gitattributes`, where it means
    /// `sub/sub/frame/...`. Checking that the patterns are *present* said the
    /// project was fine; `git check-attr` says whether they *work*, and that is
    /// the only question worth asking.
    ///
    /// `paths` are the representative files that did not route, one per shape
    /// (see [`crate::ops::git_setup::routed_paths`]). They need not exist — the
    /// attribute is matched against the path, not the file.
    ///
    /// A warning rather than an error, and no `--fix`: the repair is
    /// `fr git setup`, which rewrites git configuration rather than project
    /// content.
    #[serde(rename = "merge_routing_broken")]
    MergeRoutingBroken { paths: Vec<String> },
    /// A track holds more open work than `limits.track_warn_bytes` allows for.
    ///
    /// **One finding per track, and it names no tasks.** The per-task version of
    /// this was tried on paper and discarded: a project that has been running a
    /// while has dozens of long notes, and a list of them is a wall nobody reads
    /// twice. There is also nothing per-task to *do* here — no individual note
    /// is the problem, the aggregate is, and the remedy is splitting the track
    /// or closing work rather than editing any one task.
    ///
    /// **Live content, not file size.** `live_bytes` is `## Backlog` plus
    /// `## Parked`; Done is excluded because `[clean]` bounds it automatically
    /// and does so by oscillating between `done_bytes_retain` and
    /// `done_bytes_threshold`. Folding that swing in would mean the same track
    /// warns before a clean and goes quiet after one with the open work
    /// untouched — a warning that answers to the archiver's schedule rather
    /// than to anything its reader did. `file_bytes` rides along for context and
    /// decides nothing.
    ///
    /// Advisory, and no `--fix`: open work cannot be archived, and how much of
    /// it belongs in one track is a judgement frame is in no position to make.
    #[serde(rename = "oversize_track")]
    OversizeTrack {
        track_id: String,
        /// `## Backlog` + `## Parked`. The measure the limit is against.
        live_bytes: usize,
        /// The whole file, live and done together. Context only.
        file_bytes: usize,
        limit_bytes: usize,
    },
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
    /// The same problem one position over: a line indented past a task's
    /// metadata that is neither metadata, nor a subtask, nor part of a `- note:`
    /// block. Carried on the task it sits *under* and re-emitted there.
    ///
    /// A separate finding from `stranded_line` rather than the same one with a
    /// flag, because the likely remedy differs and the message should say so. A
    /// line *above* a task is usually mis-indented prose that belongs to
    /// whatever came before it. A line *under* a task, past its metadata, is
    /// usually a note that lost its `- note:` key — the fix is nearly always to
    /// add one back, which is a different thing to tell someone.
    ///
    /// Not repairable automatically, for `stranded_line`'s reason: "nearly
    /// always" is not always, and guessing wrong rewrites prose.
    #[serde(rename = "stranded_line_under")]
    StrandedLineUnder {
        track_id: String,
        /// The task the line sits under — `None` if that task has no ID yet.
        under_task_id: Option<String>,
        /// The title of that task, which identifies it when the ID is absent.
        under_title: String,
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
    /// Archived tasks carrying an ID prefix their track no longer uses.
    ///
    /// `fr track rename --prefix` renames the live tasks and the archive. Until
    /// it was fixed it renamed only the live ones — it read the archive as a
    /// track, found no `## Section` headers, and wrote nothing — so a project
    /// renamed before that has archived IDs on the old prefix to this day, and
    /// nothing said so: the command reported success and `fr check` called the
    /// result valid.
    ///
    /// Latent rather than broken, which is why it is a warning. The archived
    /// tasks are still readable and still unique. What makes it worth reporting
    /// is what happens if that abandoned prefix is ever handed to another track:
    /// the new track mints from its own files, cannot see this archive, and
    /// reissues numbers it already holds.
    #[serde(rename = "archived_prefix_stale")]
    ArchivedPrefixStale {
        track_id: String,
        /// The archive holding them, as check reports paths elsewhere.
        archive: String,
        /// The prefix on the archived IDs.
        found: String,
        /// The prefix the track uses now.
        expected: String,
        /// The IDs involved, sorted.
        task_ids: Vec<String>,
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
    /// The TUI left rescue copies behind and nobody has dealt with them.
    ///
    /// `frame/.rescue/` holds files the TUI could not save, written at exit as
    /// the last thing it does. The exit message names the directory once, on a
    /// terminal that is about to be closed — after which nothing mentions it
    /// again, and the copies sit there being the only version of that work.
    ///
    /// A **warning**, and with no `--fix`: moving a rescue copy into place would
    /// overwrite whatever is there now, which may be newer, and deleting it
    /// would destroy the thing the directory exists to protect. Both are the
    /// user's call. Clearing the directory clears the warning.
    ///
    /// Kept in the working copy rather than shared like the recovery log,
    /// deliberately: a rescue copy is read within minutes of the crash that
    /// produced it or not at all, and `frame/.rescue/` is where someone will
    /// actually look. This finding is what makes "or not at all" less likely.
    #[serde(rename = "unclaimed_rescue_copies")]
    UnclaimedRescueCopies {
        /// Absolute — the reader may not be in this working copy.
        path: String,
        /// File names, sorted, so the warning says what is waiting.
        files: Vec<String>,
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
    /// A `ref:`/`spec:` path that leaves the project — absolute, or escaping
    /// upward.
    ///
    /// A **warning**, where a path with no file behind it is an error, and the
    /// difference is the point: this one resolves. Nothing about the project is
    /// invalid *here*; what is wrong is that the reference means something else,
    /// or nothing, on any other machine. Errors exit non-zero, and a policy
    /// invented after these values were written should not fail a build that was
    /// passing yesterday.
    ///
    /// **No `--fix`.** Removing the ref discards intent, and rewriting it would
    /// mean guessing which file inside the project was meant — the same
    /// reasoning as [`CheckError::DanglingDep`].
    #[serde(rename = "ref_outside_project")]
    RefOutsideProject {
        track_id: String,
        task_id: String,
        /// `ref` or `spec` — one finding for both keys, since the rule and the
        /// remedy are identical.
        field: String,
        path: String,
        /// Why it cannot travel, already phrased for a message.
        reason: String,
    },
    /// A `ref:`/`spec:` path that git is ignoring, so it exists in this working
    /// copy and in nobody else's.
    ///
    /// Warning rather than error for the same reason as
    /// [`CheckWarning::RefOutsideProject`], and reported only for paths that
    /// resolve — one that does not is already a broken ref. A **tracked** file is
    /// never reported, however broadly a rule covers it: ignore rules do not
    /// apply to what is in the index, so it does travel.
    ///
    /// Silent outside a git repository, or when `git` cannot be run.
    ///
    /// **No `--fix`.** Un-ignoring the file is a decision about the repository,
    /// not about the task.
    #[serde(rename = "ref_gitignored")]
    RefGitignored {
        track_id: String,
        task_id: String,
        field: String,
        path: String,
    },
    /// A file in `archive/_tracks/` that no archived `[[tracks]]` entry claims.
    ///
    /// The archived counterpart of [`CheckError::TrackFileUnreferenced`], and a
    /// warning where that one is an error, because the consequence is milder: a
    /// stray file in `tracks/` is live work that should be visible and is not,
    /// while this is archived content, absent from views that would not have
    /// shown it anyway. Warnings exit 0, so archive residue does not fail a
    /// build — it just stops being invisible.
    ///
    /// Three shapes reach it, and [`Self::ArchivedTrackFileUnclaimed::state`]
    /// tells them apart. No config row at all (`None`) is residue: a merge, a
    /// manual `mv`, or a `fr track rename --new-id` run on an archived track by
    /// a frame old enough to have allowed it — that rename moved the done-task
    /// archive and left this file behind under the old id, and nothing reported
    /// it, because the roster check only ever scanned `tracks/`. A row in state
    /// `active` or `shelved` is a copy the archive still holds after the track
    /// came back out, and it pairs with a [`CheckError::TrackFileMissing`] on
    /// `tracks/<id>.md` when an unarchive was interrupted — the two findings
    /// together name the file to move and where it belongs.
    ///
    /// **No `--fix`.** Adopting the file invents an id, a name and an ID prefix;
    /// deleting it discards content. When it is the far half of an old rename
    /// the repair is a `mv`, and which id it should answer to is known only to
    /// the person who renamed it — the same reasoning as
    /// [`CheckError::TrackFileUnreferenced`].
    #[serde(rename = "archived_track_file_unclaimed")]
    ArchivedTrackFileUnclaimed {
        /// Path relative to `frame/`.
        path: String,
        /// The `# Title` the file carries, when it has one.
        title: Option<String>,
        /// The state of the `[[tracks]]` row whose id matches the filename, or
        /// `None` when no row does. It decides which of the three stories this
        /// is, so the message can say which.
        state: Option<String>,
    },
}

/// Informational messages (not errors or warnings).
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type")]
pub enum CheckInfo {
    /// Recovery log summary
    #[serde(rename = "recovery_log")]
    RecoveryLog {
        entry_count: usize,
        oldest: String,
        /// Absolute path to the log these entries are in. Worth naming: the log
        /// is shared by every worktree of a clone and its location is
        /// configurable, so "the recovery log" is not a place the reader can
        /// assume they know.
        path: String,
    },
}

// ---------------------------------------------------------------------------
// Main check entry point
// ---------------------------------------------------------------------------

/// Duplicate sections and unrecognised headings, in tracks and in the
/// section-less files (archives, the inbox) alike.
///
/// The archive and inbox halves read raw lines rather than a parsed model,
/// because their models have nowhere to put a heading: `Archive` is a header, a
/// flat task list and whatever trailed the last task, so a heading in the
/// middle is indistinguishable from prose once parsed. The raw scan is what can
/// still see it.
fn check_headings(project: &Project, result: &mut CheckResult) {
    for (track_id, track) in &project.tracks {
        for dup in track.duplicate_sections() {
            result.errors.push(CheckError::DuplicateSection {
                track_id: track_id.clone(),
                section: dup.kind,
                count: dup.count,
                hidden_tasks: dup.hidden_tasks,
            });
        }
        for unknown in track.unknown_headings() {
            result.errors.push(CheckError::UnknownSectionHeading {
                track_id: track_id.clone(),
                heading: unknown.heading,
                stranded_tasks: unknown.stranded_tasks,
            });
        }
    }

    // `archive/_tracks/` holds whole archived **track** files, headings and all,
    // so `## Backlog` there is correct rather than unrecognised — they get the
    // track rules. `archive/*.md` (per-track done archives) and `inbox.md` have
    // no section concept, so any `##` in one is a heading frame cannot read.
    if let Ok(entries) = std::fs::read_dir(project.frame_dir.join("archive/_tracks")) {
        for path in entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "md"))
        {
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            let track = crate::parse::parse_track(&text);
            for dup in track.duplicate_sections() {
                result.errors.push(CheckError::DuplicateSection {
                    track_id: name.clone(),
                    section: dup.kind,
                    count: dup.count,
                    hidden_tasks: dup.hidden_tasks,
                });
            }
            for unknown in track.unknown_headings() {
                result.errors.push(CheckError::UnknownSectionHeading {
                    track_id: name.clone(),
                    heading: unknown.heading,
                    stranded_tasks: unknown.stranded_tasks,
                });
            }
        }
    }

    // Through the parsed models, not a raw line scan. An inbox item's body and a
    // task's note are freeform markdown and may perfectly well contain a `##`
    // heading — a real project's inbox has five, all of them prose inside item
    // bodies, and a raw scan reported every one. The models put body text out of
    // reach: only what surrounds the content can hold a structural heading.
    let flag = |file: &str, lines: &[String], result: &mut CheckResult| {
        for line in lines {
            if let Some(rest) = line.strip_prefix("## ") {
                result.errors.push(CheckError::UnknownSectionHeading {
                    track_id: file.to_string(),
                    heading: rest.trim().to_string(),
                    // Nothing is *stranded* in these files: they have no sections,
                    // so a heading below the title ends the task list and the
                    // remainder is already carried as trailing text.
                    stranded_tasks: 0,
                });
            }
        }
    };

    if let Some(inbox) = &project.inbox {
        flag("inbox.md", &inbox.header_lines, result);
    }
    if let Ok(entries) = std::fs::read_dir(project.frame_dir.join("archive")) {
        for path in entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "md"))
        {
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            let archive = crate::parse::parse_archive(&text);
            flag(&name, &archive.header, result);
            flag(&name, &archive.trailing, result);
        }
    }
}

/// Warn on tracks holding more open work than `limits.track_warn_bytes`.
///
/// See [`CheckWarning::OversizeTrack`] for why this measures live content and
/// says nothing about individual tasks.
fn check_track_sizes(project: &Project, result: &mut CheckResult) {
    let Some(limit) = project.config.limits.track_warn_bytes else {
        return;
    };
    let limit_bytes = limit.bytes() as usize;
    for (track_id, track) in &project.tracks {
        let live_bytes = track.live_bytes();
        if live_bytes <= limit_bytes {
            continue;
        }
        // Off disk rather than off the model: this is the number a human sees
        // in a file listing, and it is context rather than the trigger, so it
        // should match what they would see there. Absent if the file cannot be
        // read, which every other check already reports on.
        let file_bytes = project
            .config
            .tracks
            .iter()
            .find(|tc| &tc.id == track_id)
            .and_then(|tc| std::fs::metadata(project.frame_dir.join(&tc.file)).ok())
            .map(|m| m.len() as usize)
            .unwrap_or(live_bytes);
        result.warnings.push(CheckWarning::OversizeTrack {
            track_id: track_id.clone(),
            live_bytes,
            file_bytes,
            limit_bytes,
        });
    }
}

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

    // `ref:`/`spec:` values that resolve here and nowhere else.
    check_ref_portability(project, &mut result);

    // Working-copy-local frame files leaking into git.
    check_local_files_ignored(&project.frame_dir, &mut result);

    // The merge driver, which is per-clone and so cannot arrive with a clone.
    check_merge_driver(&project.root, &mut result);

    // And whether `.gitattributes` — which *does* arrive with a clone — actually
    // routes anything, asked of git rather than inferred from the file.
    check_merge_routing(&project.root, &mut result);

    // Numbers handed out twice, where one holder is archived (invisible to the
    // live-tracks-only duplicate check above).
    check_archived_id_collisions(project, &mut result);
    check_archived_prefixes(project, &mut result);

    // The track roster in `project.toml` against what is actually in `tracks/`.
    // Every other check here runs over `project.tracks`, which only holds what
    // loaded — so a track whose file went missing is invisible to all of them.
    check_track_roster(project, &mut result);

    // The durable ID frontier store: unreadable, or reset at some point.
    check_id_frontier(&project.frame_dir, &mut result);
    check_inflight(&project.frame_dir, &mut result);
    check_rescue(&project.frame_dir, &mut result);

    // Inbox item bodies that leave a code fence open.
    if let Some(ref inbox) = project.inbox {
        check_inbox(inbox, &mut result);
    }

    // Sections duplicated by a text merge, and headings frame cannot read.
    check_headings(project, &mut result);

    // Tracks carrying more open work than one track should.
    check_track_sizes(project, &mut result);

    // Does the recovery log actually hold the other side of each conflict?
    resolve_conflict_evidence(&project.frame_dir, &mut result);

    // Recovery log summary
    if let Some(summary) = crate::io::recovery::recovery_summary(&project.frame_dir) {
        let oldest_str = summary
            .oldest
            .map(|ts| ts.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
            .unwrap_or_default();
        result.info.push(CheckInfo::RecoveryLog {
            entry_count: summary.entry_count,
            oldest: oldest_str,
            path: crate::io::recovery::recovery_log_path(&project.frame_dir)
                .display()
                .to_string(),
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

/// Fill in `evidence` on every unresolved-conflict error by looking the entry up.
///
/// **An entry counts when it names the task, not when its timestamp matches.**
/// The marker's stamp identifies the *merge run*, not the entry: `fr merge` takes
/// one `Utc::now()` per invocation, writes it onto every task it marks, and
/// stamps every recovery entry it logs with the same instant. So a stamp hit says
/// only that some conflict from that run reached this log — which is not the
/// question, and the two answers come apart for real reasons. `log_recovery` is
/// best-effort per entry, so a failure partway through the run's loop leaves the
/// earlier siblings behind and the rest unlogged; git invokes the merge driver
/// once per file, so two runs land in one second and share a stamp truncated to
/// seconds, while only one of them may have found a project to log into. Either
/// way an unlogged task borrows a logged sibling's stamp and this reports
/// evidence that is not here, which is the single thing the field exists to
/// prevent.
///
/// Naming the task loses nothing by comparison: the entry's description carries
/// the conflict key, so an entry that was really written for a task with an ID
/// always names it. The stamp survives only for a marker on a task with **no**
/// ID — an `ambiguous-title` conflict is by definition on one — where there is no
/// ID to look for and the run is the most that can be established.
///
/// Resolved by position rather than through a set keyed by task ID, so no two
/// markers can answer for each other. Two keys collide in practice: `""`, shared
/// by every ID-less marker, and an ID that is genuinely duplicated — itself a
/// [`CheckError::DuplicateId`], so it is a state this runs against.
///
/// Reading the log once for all markers rather than once per marker: a project
/// recovering from a large rebase can carry a dozen, and the log is the one file
/// here that is unbounded in size.
fn resolve_conflict_evidence(frame_dir: &Path, result: &mut CheckResult) {
    let markers: Vec<(usize, String, String)> = result
        .errors
        .iter()
        .enumerate()
        .filter_map(|(i, e)| match e {
            CheckError::UnresolvedMergeConflict {
                task_id, detail, ..
            } => Some((i, task_id.clone(), detail.clone())),
            _ => None,
        })
        .collect();
    if markers.is_empty() {
        return;
    }

    let entries = crate::io::recovery::read_recovery_entries(frame_dir, None, None);
    for (i, task_id, detail) in markers {
        let found = entries.iter().any(|entry| {
            if task_id.is_empty() {
                // The marker reads `<reason-slug> <timestamp>`; the stamp is last.
                let stamp = detail.split_whitespace().next_back().unwrap_or_default();
                !stamp.is_empty()
                    && entry
                        .timestamp
                        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
                        == stamp
            } else {
                crate::io::recovery::entry_names(entry, &task_id)
            }
        });
        if let CheckError::UnresolvedMergeConflict { evidence, .. } = &mut result.errors[i] {
            *evidence = found;
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

    // Error: a merge conflict nobody has resolved. Checked for every task,
    // subtasks included — a merge marks whichever task it could not decide.
    if let Some(detail) = task.metadata.iter().find_map(|m| match m {
        Metadata::Conflict(d) => Some(d.clone()),
        _ => None,
    }) {
        result.errors.push(CheckError::UnresolvedMergeConflict {
            track_id: track_id.to_string(),
            task_id: task_id.to_string(),
            detail,
            // Resolved in one pass at the end, by `resolve_conflict_evidence` —
            // reading the log once beats reading it per marker.
            evidence: false,
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

    // And the same, held *under* this task rather than above it: content past
    // its metadata that is neither metadata nor a subtask.
    for line in task.trailing_lines.iter().filter(|l| !l.trim().is_empty()) {
        result.warnings.push(CheckWarning::StrandedLineUnder {
            track_id: track_id.to_string(),
            under_task_id: task.id.as_ref().map(|id| id.to_string()),
            under_title: task.title.clone(),
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
                    if !refs_ops::exists(project_root, r) {
                        result.errors.push(CheckError::BrokenRef {
                            track_id: track_id.to_string(),
                            task_id: task_id.to_string(),
                            path: r.clone(),
                        });
                    }
                }
            }
            Metadata::Spec(specs) => {
                for spec in specs {
                    if !refs_ops::exists(project_root, spec) {
                        result.errors.push(CheckError::BrokenSpec {
                            track_id: track_id.to_string(),
                            task_id: task_id.to_string(),
                            path: spec.clone(),
                        });
                    }
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

/// Flag `ref:`/`spec:` values that resolve in this working copy and nowhere
/// else — absolute or escaping paths, and paths git is ignoring.
///
/// **Project-wide rather than per-task**, unlike every other metadata check, and
/// the reason is the git call: one `check-ignore` for every reference in the
/// project instead of one per task. The same rule the write path enforces
/// (`ops::refs`), applied to values that predate it or arrived past it — through
/// `--force`, through the TUI, or by hand.
fn check_ref_portability(project: &Project, result: &mut CheckResult) {
    // (track, task, field, value), in report order.
    let mut all: Vec<(String, String, &'static str, String)> = Vec::new();
    for (track_id, track) in &project.tracks {
        for node in &track.nodes {
            if let TrackNode::Section { tasks, .. } = node {
                collect_ref_values(tasks, track_id, &mut all);
            }
        }
    }
    if all.is_empty() {
        return;
    }

    for (track_id, task_id, field, value) in &all {
        if let Some(rejection) = refs_ops::containment(value) {
            result.warnings.push(CheckWarning::RefOutsideProject {
                track_id: track_id.clone(),
                task_id: task_id.clone(),
                field: field.to_string(),
                path: value.clone(),
                reason: rejection.reason().to_string(),
            });
        }
    }

    // Only values that stay inside are worth asking git about: an escaping one
    // is already reported, and `check-ignore` would answer about a path outside
    // the repo.
    let inside: Vec<String> = all
        .iter()
        .filter(|(_, _, _, v)| refs_ops::containment(v).is_none())
        .map(|(_, _, _, v)| v.clone())
        .collect();
    let ignored = refs_ops::ignored(&project.root, &inside);
    if ignored.is_empty() {
        return;
    }
    for (track_id, task_id, field, value) in &all {
        if ignored.contains(value) {
            result.warnings.push(CheckWarning::RefGitignored {
                track_id: track_id.clone(),
                task_id: task_id.clone(),
                field: field.to_string(),
                path: value.clone(),
            });
        }
    }
}

/// Every `ref:`/`spec:` value in `tasks` and their subtasks, tagged with where
/// it came from. Depth-first, so findings report in file order.
fn collect_ref_values(
    tasks: &[Task],
    track_id: &str,
    out: &mut Vec<(String, String, &'static str, String)>,
) {
    for task in tasks {
        let task_id = task.id.as_deref().unwrap_or("").to_string();
        for meta in &task.metadata {
            let (field, values) = match meta {
                Metadata::Ref(v) => ("ref", v),
                Metadata::Spec(v) => ("spec", v),
                _ => continue,
            };
            for value in values {
                out.push((track_id.to_string(), task_id.clone(), field, value.clone()));
            }
        }
        collect_ref_values(&task.subtasks, track_id, out);
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

/// Flag a clone that has not registered frame's merge driver.
///
/// Silent outside git, and silent when `git` cannot be run — the same rule
/// `check_local_files_ignored` follows, and for the same reason. A project that
/// is not in a repo has no merge to get wrong, and nagging it about a driver
/// would be noise about a problem it cannot have.
fn check_merge_driver(root: &Path, result: &mut CheckResult) {
    if crate::ops::git_setup::driver_registered(root) == Some(false) {
        result.warnings.push(CheckWarning::MergeDriverUnregistered);
    }
}

/// Ask git whether frame's files actually reach the merge driver.
///
/// The two halves of routing are decided in different places and nothing kept
/// them honest: `.gitattributes` decides what git hands the driver, and it is
/// easy to write a line there that looks right and matches nothing. Testing for
/// the *presence* of a pattern is a proxy; `git check-attr` is the real thing,
/// and it is what this asks.
///
/// Silent outside a repo and whenever git cannot be run, like every other git
/// check here. Probing paths that do not exist is fine and deliberate — a
/// project that has never run `fr clean` has no archive file, and its routing
/// still needs to be right before the day it does.
fn check_merge_routing(root: &Path, result: &mut CheckResult) {
    if crate::io::git::repo_paths(&root.join("frame")).is_none() {
        return;
    }
    let paths = crate::ops::git_setup::routed_paths();
    let Some(values) = crate::io::git::merge_attr_values(root, &paths) else {
        return;
    };
    let unrouted: Vec<String> = paths
        .iter()
        .zip(values)
        .filter(|(_, value)| value != crate::ops::git_setup::DRIVER_NAME)
        .map(|(path, _)| path.clone())
        .collect();
    if !unrouted.is_empty() {
        result
            .warnings
            .push(CheckWarning::MergeRoutingBroken { paths: unrouted });
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

/// Flag archived IDs whose prefix is not the one their track uses now.
///
/// Only the per-track done archives: `archive/<track>.md` is the file
/// `fr track rename --prefix` is responsible for, and the one it used to skip.
/// A whole archived track under `_tracks/` is not renamed by anything, so
/// reporting it would be a finding with no fix and no fault.
fn check_archived_prefixes(project: &Project, result: &mut CheckResult) {
    use std::collections::BTreeMap;

    let Ok(archives) = crate::io::project_io::load_archives(&project.frame_dir) else {
        return;
    };
    let mut archives = archives;
    archives.sort_by(|a, b| a.0.cmp(&b.0));

    for (track_id, tasks) in archives {
        let Some(expected) = project.config.ids.prefixes.get(&track_id) else {
            continue;
        };

        let mut by_prefix: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let mut ids: std::collections::HashMap<String, Vec<String>> =
            std::collections::HashMap::new();
        collect_id_locations(&tasks, &track_id, &mut ids);
        for id in ids.keys() {
            // A subtask id extends its parent's (`MAI-001.2`), so the prefix is
            // still whatever sits before the first dash.
            if let Some((prefix, _)) = id.split_once('-') {
                by_prefix
                    .entry(prefix.to_string())
                    .or_default()
                    .push(id.clone());
            }
        }

        for (found, mut task_ids) in by_prefix {
            if &found == expected {
                continue;
            }
            task_ids.sort();
            result.warnings.push(CheckWarning::ArchivedPrefixStale {
                track_id: track_id.clone(),
                archive: format!("archive/{}.md", track_id),
                found,
                expected: expected.clone(),
                task_ids,
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

/// Reconcile the track roster in `project.toml` against the files in `tracks/`.
///
/// This is the one check that cannot work from `project.tracks`. That vector
/// holds the tracks that *loaded*, and `load_project` skips a configured track
/// whose file is missing — so the failure this looks for is precisely the one
/// every other check is blind to. Reproduced by renaming a track file out from
/// under its config entry: `fr list` goes empty and `fr check` used to call the
/// project valid while the tasks sat unreferenced on disk.
///
/// A crash partway through `fr track rename --id` is one way in. A merge that
/// took one side's `project.toml` and the other's file layout, a manual `mv`,
/// an editor's "rename file", and a partial checkout all reach the same state,
/// which is why the detector matters more than any single window.
fn check_track_roster(project: &Project, result: &mut CheckResult) {
    let frame_dir = &project.frame_dir;
    let mut referenced: HashSet<String> = HashSet::new();
    // The ids entitled to a file in `archive/_tracks/`: exactly the archived
    // ones. Collected on the pass that already computes where each of them
    // should be, and used by the scan of that directory below.
    let mut archived_ids: HashSet<String> = HashSet::new();

    for tc in &project.config.tracks {
        if tc.state == "archived" {
            archived_ids.insert(tc.id.clone());
            // An archived track keeps its `file` pointing at `tracks/`, but the
            // file itself was moved to `archive/_tracks/` — see
            // `track_ops::archive_track_file`. Missing from `tracks/` is the
            // expected state, so look where it actually went.
            let archived = frame_dir
                .join("archive")
                .join("_tracks")
                .join(format!("{}.md", tc.id));
            if !archived.exists() {
                result.errors.push(CheckError::TrackFileMissing {
                    track_id: tc.id.clone(),
                    path: format!("archive/_tracks/{}.md", tc.id),
                    state: tc.state.clone(),
                });
            }
            continue;
        }

        referenced.insert(tc.file.clone());
        if !frame_dir.join(&tc.file).exists() {
            result.errors.push(CheckError::TrackFileMissing {
                track_id: tc.id.clone(),
                path: tc.file.clone(),
                state: tc.state.clone(),
            });
        }
    }

    // The other direction, for each of the two directories a track file can
    // live in. Separate functions because each gives up on its own missing
    // directory: as one body, the `tracks/` scan's early return silently took
    // the archive scan with it.
    check_unreferenced_track_files(frame_dir, &referenced, result);
    check_unclaimed_archived_track_files(project, &archived_ids, result);
}

/// A real file in `tracks/` that no `[[tracks]]` entry points at.
fn check_unreferenced_track_files(
    frame_dir: &Path,
    referenced: &HashSet<String>,
    result: &mut CheckResult,
) {
    let Ok(entries) = std::fs::read_dir(frame_dir.join("tracks")) else {
        return;
    };
    let mut unreferenced: Vec<(String, Option<String>)> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|e| e != "md") || !path.is_file() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let rel = format!("tracks/{}", name);
        if referenced.contains(&rel) {
            continue;
        }
        unreferenced.push((rel, track_title(&path)));
    }
    // `read_dir` order is filesystem order; sort so two runs agree.
    unreferenced.sort();
    for (path, title) in unreferenced {
        result
            .errors
            .push(CheckError::TrackFileUnreferenced { path, title });
    }
}

/// The same drift one directory over: a file in `archive/_tracks/` that no
/// archived `[[tracks]]` entry claims.
///
/// A track file gets here exactly one way — `track_ops::archive_track_file`,
/// which names it for the track's id — so the filename stem *is* the id it
/// claims to be, and the config row carrying that id (or the absence of one) is
/// what says whether the claim holds. See
/// [`CheckWarning::ArchivedTrackFileUnclaimed`] for the three shapes.
fn check_unclaimed_archived_track_files(
    project: &Project,
    archived_ids: &HashSet<String>,
    result: &mut CheckResult,
) {
    let dir = project.frame_dir.join("archive").join("_tracks");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return;
    };
    let mut unclaimed: Vec<(String, Option<String>, Option<String>)> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|e| e != "md") || !path.is_file() {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|n| n.to_str()) else {
            continue;
        };
        if archived_ids.contains(stem) {
            continue;
        }
        let state = project
            .config
            .tracks
            .iter()
            .find(|tc| tc.id == stem)
            .map(|tc| tc.state.clone());
        unclaimed.push((
            format!("archive/_tracks/{}.md", stem),
            track_title(&path),
            state,
        ));
    }
    unclaimed.sort();
    for (path, title, state) in unclaimed {
        result
            .warnings
            .push(CheckWarning::ArchivedTrackFileUnclaimed { path, title, state });
    }
}

/// The `# Title` a track file carries, if any — the name to give the track when
/// an unreferenced file is adopted.
fn track_title(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    text.lines()
        .find_map(|line| line.strip_prefix("# "))
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
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

/// Report rescue copies the TUI wrote at exit that nobody has dealt with.
fn check_rescue(frame_dir: &Path, result: &mut CheckResult) {
    let dir = frame_dir.join(crate::tui::app::RESCUE_DIR);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return;
    };

    let mut files: Vec<String> = entries
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_file())
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    if files.is_empty() {
        return;
    }
    files.sort();

    result.warnings.push(CheckWarning::UnclaimedRescueCopies {
        path: dir.canonicalize().unwrap_or(dir).display().to_string(),
        files,
    });
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
            recovery: Default::default(),
            limits: Default::default(),
        }
    }

    /// A project whose config and whose files agree, which is what
    /// `check_track_roster` requires of a clean project: the track file is
    /// written to disk at the path `make_config` says it lives at. Before that
    /// check existed these fixtures were config-only, and every one of them
    /// described a project with a missing track file.
    fn make_project_at(root: &Path, track_src: &str) -> Project {
        let track = parse_track(track_src);
        let frame_dir = root.join("frame");
        std::fs::create_dir_all(frame_dir.join("tracks")).unwrap();
        std::fs::write(frame_dir.join("tracks/main.md"), track_src).unwrap();
        Project {
            root: root.to_path_buf(),
            frame_dir,
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

        // These two live at the frame-dir root rather than in `tracks/`, which
        // the config is free to say. Write them where it says they are, so the
        // roster check has nothing to report.
        let frame_dir = tmp.path().join("frame");
        std::fs::create_dir_all(&frame_dir).unwrap();
        std::fs::write(frame_dir.join("a.md"), "# A\n").unwrap();
        std::fs::write(frame_dir.join("b.md"), "# B\n").unwrap();

        let project = Project {
            root: tmp.path().to_path_buf(),
            frame_dir,
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

    /// `ref:` and `spec:` resolve by the same rule. The anchor was stripped for
    /// one and not the other, so `doc/design.md#rationale` was a valid spec and
    /// a broken ref — and a line reference, which is how most refs to code get
    /// written, was broken in both.
    #[test]
    fn a_ref_resolves_exactly_like_a_spec() {
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join("doc")).unwrap();
        std::fs::create_dir_all(tmp.path().join("src")).unwrap();
        std::fs::write(tmp.path().join("doc/design.md"), "# Design").unwrap();
        std::fs::write(tmp.path().join("src/parser.rs"), "fn main() {}").unwrap();

        let project = make_project_at(
            tmp.path(),
            "\
# Main

## Backlog

- [ ] `M-001` Task one
  - added: 2025-05-01
  - ref: doc/design.md#rationale, src/parser.rs:807, src/parser.rs:807-820
  - spec: doc/design.md#rationale, src/parser.rs:807

## Done
",
        );

        let result = check_project(&project);
        let broken: Vec<_> = result
            .errors
            .iter()
            .filter(|e| {
                matches!(
                    e,
                    CheckError::BrokenRef { .. } | CheckError::BrokenSpec { .. }
                )
            })
            .collect();
        assert!(broken.is_empty(), "unexpected findings: {:?}", broken);
    }

    /// A suffix is not a way to make a missing file look present: the path in
    /// front of it still has to exist.
    #[test]
    fn a_suffix_does_not_excuse_a_missing_file() {
        let tmp = TempDir::new().unwrap();
        let project = make_project_at(
            tmp.path(),
            "\
# Main

## Backlog

- [ ] `M-001` Task one
  - added: 2025-05-01
  - ref: src/gone.rs:807, doc/gone.md#anchor

## Done
",
        );

        let result = check_project(&project);
        let broken: Vec<_> = result
            .errors
            .iter()
            .filter(|e| matches!(e, CheckError::BrokenRef { .. }))
            .collect();
        assert_eq!(broken.len(), 2, "{:?}", result.errors);
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

        // Indented past M-001's metadata, so it is carried *under* M-001 and
        // reported as such. It used to be reported against M-002, the task it
        // sits above — which is where the parser put it, not where it visually
        // hangs off, and that mismatch is what F12 was: anchored to a task it
        // did not belong to, and left behind when that task's neighbour moved.
        assert!(
            stranded.is_empty(),
            "this shape is no longer carried on the following task: {stranded:?}"
        );

        let under: Vec<_> = result
            .warnings
            .iter()
            .filter_map(|w| match w {
                CheckWarning::StrandedLineUnder {
                    under_task_id,
                    line,
                    ..
                } => Some((under_task_id.clone(), line.clone())),
                _ => None,
            })
            .collect();
        assert_eq!(
            under,
            vec![(
                Some("M-001".to_string()),
                "**Shape.** A line that lost its indent.".to_string()
            )]
        );

        // Nothing structural is broken — the file still parses and still writes
        // back unchanged.
        assert!(result.valid);
    }

    /// A line stranded *between* two tasks, at the metadata indent rather than
    /// past it, still goes to the following task. The two shapes render in the
    /// same place, so moving this one would change which task carries every
    /// existing stranded line for no gain.
    #[test]
    fn test_warn_stranded_line_between_tasks() {
        let tmp = TempDir::new().unwrap();
        let project = make_project_at(
            tmp.path(),
            "\
# Main

## Done

- [x] `M-001` Sharded map lowering
  - resolved: 2026-07-20
  a line at the metadata indent
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
        assert_eq!(
            stranded,
            vec![(
                Some("M-002".to_string()),
                "a line at the metadata indent".to_string()
            )]
        );
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
