use clap::{Args, Parser, Subcommand};

#[derive(Parser)]
// The banner stays on the bare crate version; only `--version` (and `fr info`)
// carry the build's commit.
#[command(name = "fr", about = concat!("[>] frame v", env!("CARGO_PKG_VERSION"), " - your backlog is plain text"), version = crate::version::LONG)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Commands>,

    /// Output as JSON
    #[arg(long, global = true)]
    pub json: bool,

    /// Run against a different project directory
    #[arg(short = 'C', long = "project-dir", global = true)]
    pub project_dir: Option<String>,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Initialize a new frame project in the current directory
    Init(InitArgs),
    /// List tasks in a track
    List(ListArgs),
    /// Show task details
    Show(ShowArgs),
    /// Show ready (unblocked) tasks
    Ready(ReadyArgs),
    /// Show blocked tasks and their blockers
    Blocked,
    /// Search tasks by regex
    Search(SearchArgs),
    /// List inbox items, or add a new one
    Inbox(InboxCmd),
    /// List all tracks
    Tracks,
    /// Show task statistics
    Stats(StatsArgs),
    /// Show recently completed tasks
    Recent(RecentArgs),
    /// Show dependency tree for a task
    Deps(DepsArgs),
    /// Validate project integrity; `--fix` repairs what can be repaired safely
    Check(CheckArgs),
    /// Show project identity (version, name, frame dir, actor, track count)
    Info,
    /// Add a task to a track's backlog (bottom)
    Add(AddArgs),
    /// Push a task to top of a track's backlog
    Push(PushArgs),
    /// Add a subtask
    Sub(SubArgs),
    /// Change task state
    State(StateArgs),
    /// Start a task (shortcut for state <ID> active)
    Start(StartArgs),
    /// Mark a task done (shortcut for state <ID> done)
    Done(DoneArgs),
    /// Add or remove tags
    Tag(TagArgs),
    /// Add or remove dependencies
    Dep(DepArgs),
    /// Set task note
    Note(NoteArgs),
    /// Add, remove or set file references
    Ref(PathFieldArgs),
    /// Add, remove or set spec references
    Spec(PathFieldArgs),
    /// Change task title
    Title(TitleArgs),
    /// Move a task (reorder or cross-track)
    Mv(MvArgs),
    /// Triage an inbox item to a track
    Triage(TriageArgs),
    /// Track management
    Track(TrackCmd),
    /// Run maintenance and cleanup
    Clean(CleanArgs),
    /// Import tasks from a markdown file
    Import(ImportArgs),
    /// Permanently delete tasks
    Delete(DeleteArgs),
    /// Manage project registry
    Projects(ProjectsCmd),
    /// Manage this working copy's actor token
    Actor(ActorCmd),
    /// View or manage the recovery log
    Recovery(RecoveryCmd),
    /// Three-way merge two versions of a track or the inbox (see also: `fr actor merge`)
    Merge(MergeArgs),
    /// Git integration for this clone
    Git(GitCmd),
}

// ---------------------------------------------------------------------------
// Git args
// ---------------------------------------------------------------------------

#[derive(Args)]
pub struct GitCmd {
    #[command(subcommand)]
    pub action: GitAction,
}

#[derive(Subcommand)]
pub enum GitAction {
    /// Configure this clone: .gitignore, .gitattributes, and the merge driver
    Setup(GitSetupArgs),
}

#[derive(Args)]
pub struct GitSetupArgs {
    /// Report what would change without writing anything
    #[arg(long)]
    pub dry_run: bool,
}

// ---------------------------------------------------------------------------
// Merge args
// ---------------------------------------------------------------------------

/// Arguments mirror what a version control system hands a custom merge driver:
/// an ancestor, two sides, and the path the result belongs at. Git spells them
/// `%O %A %B %P`; Mercurial and jj use the same four in different words.
#[derive(Args)]
pub struct MergeArgs {
    /// Common ancestor version (git: %O)
    #[arg(long, value_name = "FILE", required_unless_present = "resolve")]
    pub base: Option<String>,
    /// Our version; the merged result is written here (git: %A)
    #[arg(long, value_name = "FILE", required_unless_present = "resolve")]
    pub ours: Option<String>,
    /// Their version (git: %B)
    #[arg(long, value_name = "FILE", required_unless_present = "resolve")]
    pub theirs: Option<String>,
    /// Path the result belongs at, used to tell a track from the inbox (git: %P)
    #[arg(long, value_name = "PATH")]
    pub path: Option<String>,
    /// Force the file kind instead of inferring it from --path
    #[arg(long, value_parser = ["track", "archive", "inbox"])]
    pub kind: Option<String>,
    /// Clear the conflict marker on tasks whose conflict you have resolved
    #[arg(
        long,
        value_name = "ID",
        num_args = 1..,
        conflicts_with_all = ["base", "ours", "theirs", "path", "kind"]
    )]
    pub resolve: Vec<String>,
    /// Preview without writing: report what would change, and change nothing
    #[arg(long)]
    pub dry_run: bool,
}

// ---------------------------------------------------------------------------
// Init args
// ---------------------------------------------------------------------------

#[derive(Args)]
pub struct InitArgs {
    /// Project name (default: inferred from directory name)
    #[arg(long)]
    pub name: Option<String>,
    /// Create an initial track: --track <id> "name" (repeatable)
    #[arg(long, num_args = 2, value_names = ["ID", "NAME"], action = clap::ArgAction::Append)]
    pub track: Vec<String>,
    /// Reinitialize even if frame/ already exists
    #[arg(long)]
    pub force: bool,
    /// Preview without writing: report what would change, and change nothing
    #[arg(long)]
    pub dry_run: bool,
}

// ---------------------------------------------------------------------------
// Read command args
// ---------------------------------------------------------------------------

#[derive(Args)]
pub struct ListArgs {
    /// Track to list (default: all active tracks)
    pub track: Option<String>,
    /// Filter by state (todo, active, blocked, done, parked)
    #[arg(long)]
    pub state: Option<String>,
    /// Filter by tag
    #[arg(long)]
    pub tag: Option<String>,
    /// Include all tracks (shelved, archived)
    #[arg(long)]
    pub all: bool,
}

#[derive(Args)]
pub struct ShowArgs {
    /// Task ID to show
    pub id: String,
    /// Include ancestor context (parent chain)
    #[arg(long)]
    pub context: bool,
    /// Skip archived tasks (searched when a live track has no such ID)
    #[arg(long)]
    pub no_archive: bool,
}

#[derive(Args)]
pub struct ReadyArgs {
    /// Show only cc-tagged tasks on cc-focus track
    #[arg(long)]
    pub cc: bool,
    /// Filter to specific track
    #[arg(long)]
    pub track: Option<String>,
    /// Filter by tag
    #[arg(long)]
    pub tag: Option<String>,
}

#[derive(Args)]
pub struct SearchArgs {
    /// Regex pattern to search for
    pub pattern: String,
    /// Limit search to specific track
    #[arg(long)]
    pub track: Option<String>,
    /// Skip archived tasks (searched by default)
    #[arg(long)]
    pub no_archive: bool,
}

#[derive(Args)]
pub struct InboxCmd {
    /// Text to add (if omitted, lists inbox items)
    pub text: Option<String>,
    /// Tag(s) to add to the new inbox item
    #[arg(long)]
    pub tag: Vec<String>,
    /// Note body for the new inbox item
    #[arg(long)]
    pub note: Option<String>,
    /// Preview without writing: report what would change, and change nothing
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Args)]
pub struct StatsArgs {
    /// Include shelved tracks
    #[arg(long)]
    pub all: bool,
}

#[derive(Args)]
pub struct RecentArgs {
    /// Maximum number of recent items to show
    #[arg(long, default_value = "20")]
    pub limit: usize,
}

#[derive(Args)]
pub struct DepsArgs {
    /// Task ID to show dependency tree for
    pub id: String,
}

// ---------------------------------------------------------------------------
// Write command args
// ---------------------------------------------------------------------------

#[derive(Args)]
pub struct AddArgs {
    /// Track to add the task to
    pub track: String,
    /// Task title
    pub title: String,
    /// Insert after this task ID
    #[arg(long)]
    pub after: Option<String>,
    /// Note that this task was found while working on another task
    #[arg(long)]
    pub found_from: Option<String>,
    /// Preview without writing: report what would change, and change nothing
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Args)]
pub struct PushArgs {
    /// Track to push the task to
    pub track: String,
    /// Task title
    pub title: String,
    /// Preview without writing: report what would change, and change nothing
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Args)]
pub struct SubArgs {
    /// Parent task ID
    pub id: String,
    /// Subtask title
    pub title: String,
    /// Preview without writing: report what would change, and change nothing
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Args)]
pub struct StateArgs {
    /// Task ID
    pub id: String,
    /// New state (todo, active, blocked, done, parked)
    pub state: String,
    /// Preview without writing: report what would change, and change nothing
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Args)]
pub struct StartArgs {
    /// Task ID
    pub id: String,
    /// Preview without writing: report what would change, and change nothing
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Args)]
pub struct DoneArgs {
    /// Task ID
    pub id: String,
    /// Preview without writing: report what would change, and change nothing
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Args)]
pub struct TagArgs {
    /// Task ID
    pub id: String,
    /// Action: "add" or "rm"
    pub action: String,
    /// Tag name
    pub tag: String,
    /// Preview without writing: report what would change, and change nothing
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Args)]
pub struct DepArgs {
    /// Task ID
    pub id: String,
    /// Action: "add" or "rm"
    pub action: String,
    /// Dependency task ID
    pub dep_id: String,
    /// Preview without writing: report what would change, and change nothing
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Args)]
pub struct NoteArgs {
    /// Task ID
    pub id: String,
    /// Note text. Omit when `--file` is given.
    #[arg(required_unless_present = "file")]
    pub text: Option<String>,
    /// Read the note text from a file instead of the argument.
    ///
    /// The way to write anything multi-line, or anything starting with `-` —
    /// a markdown bullet list cannot be passed as an argument at all, because
    /// the parser reads a leading `-` as a flag. The path is read relative to
    /// the working directory and may live anywhere, including outside the
    /// project: it is text on its way into the note, not a `ref:`.
    #[arg(long, value_name = "PATH", conflicts_with = "text")]
    pub file: Option<std::path::PathBuf>,
    /// Discard the existing note and write this instead, rather than appending.
    ///
    /// Total: the whole note goes, however long. What it discarded is reported
    /// (`note replaced (780B → 3B)`), including under `--dry-run`.
    #[arg(long)]
    pub replace: bool,
    /// Preview without writing: report what would change, and change nothing
    #[arg(long)]
    pub dry_run: bool,
}

/// Shared by `fr ref` and `fr spec`: the two fields hold the same kind of value
/// and take the same actions, so they take the same arguments.
#[derive(Args)]
pub struct PathFieldArgs {
    /// Task ID
    pub id: String,
    /// Action: "add", "rm", or "set" (set replaces the whole list)
    pub action: String,
    /// File paths relative to the project root, each optionally carrying a
    /// `#anchor`, `:line`, `:line-range` or `:line:col`
    #[arg(required = true, num_args = 1..)]
    pub paths: Vec<String>,
    /// Accept paths that do not exist in the project. Never needed for "rm".
    #[arg(long)]
    pub force: bool,
    /// Preview without writing: report what would change, and change nothing
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Args)]
pub struct TitleArgs {
    /// Task ID
    pub id: String,
    /// New title
    pub title: String,
    /// Preview without writing: report what would change, and change nothing
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Args)]
pub struct MvArgs {
    /// Task ID
    pub id: String,
    /// Numeric position (0-indexed)
    pub position: Option<usize>,
    /// Move to top of backlog
    #[arg(long)]
    pub top: bool,
    /// Move after this task ID
    #[arg(long)]
    pub after: Option<String>,
    /// Move to a different track
    #[arg(long, conflicts_with_all = ["promote", "parent"])]
    pub track: Option<String>,
    /// Promote subtask to top-level
    #[arg(long, conflicts_with = "parent")]
    pub promote: bool,
    /// Reparent under the given task ID
    #[arg(long)]
    pub parent: Option<String>,
    /// Preview without writing: report what would change, and change nothing
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Args)]
pub struct TriageArgs {
    /// Inbox item index (1-based)
    pub index: usize,
    /// Target track
    #[arg(long)]
    pub track: String,
    /// Insert at top of backlog
    #[arg(long)]
    pub top: bool,
    /// Insert at bottom of backlog (default)
    #[arg(long)]
    pub bottom: bool,
    /// Insert after this task ID
    #[arg(long)]
    pub after: Option<String>,
    /// Preview without writing: report what would change, and change nothing
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Args)]
pub struct DeleteArgs {
    /// Task IDs to delete
    #[arg(required = true)]
    pub ids: Vec<String>,
    /// Skip confirmation prompt
    #[arg(long)]
    pub yes: bool,
    /// Preview without writing: report what would change, and change nothing
    #[arg(long)]
    pub dry_run: bool,
}

// ---------------------------------------------------------------------------
// Track management
// ---------------------------------------------------------------------------

#[derive(Args)]
pub struct TrackCmd {
    #[command(subcommand)]
    pub action: TrackAction,
}

#[derive(Subcommand)]
pub enum TrackAction {
    /// Create a new track
    New(TrackNewArgs),
    /// Shelve a track
    Shelve(TrackIdArg),
    /// Activate a track
    Activate(TrackIdArg),
    /// Archive a track
    Archive(TrackIdArg),
    /// Delete an empty track
    Delete(TrackIdArg),
    /// Move (reorder) a track
    Mv(TrackMvArgs),
    /// Set or clear the cc-focus track
    CcFocus(CcFocusArgs),
    /// Rename a track (name, id, or prefix)
    Rename(TrackRenameArgs),
}

#[derive(Args)]
pub struct TrackNewArgs {
    /// Track ID (short identifier)
    pub id: String,
    /// Track name
    pub name: String,
    /// Preview without writing: report what would change, and change nothing
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Args)]
pub struct TrackIdArg {
    /// Track ID
    pub id: String,
    /// Preview without writing: report what would change, and change nothing
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Args)]
pub struct CcFocusArgs {
    /// Track ID (omit with --clear)
    pub id: Option<String>,
    /// Clear the cc-focus setting
    #[arg(long)]
    pub clear: bool,
    /// Preview without writing: report what would change, and change nothing
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Args)]
pub struct TrackMvArgs {
    /// Track ID
    pub id: String,
    /// New position (0-indexed among active tracks)
    pub position: usize,
    /// Preview without writing: report what would change, and change nothing
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Args)]
pub struct TrackRenameArgs {
    /// Track ID
    pub id: String,
    /// New display name
    #[arg(long)]
    pub name: Option<String>,
    /// New track ID
    #[arg(long, value_name = "NEW_ID")]
    pub new_id: Option<String>,
    /// New prefix (bulk-rewrites task IDs)
    #[arg(long)]
    pub prefix: Option<String>,
    /// Preview changes without writing
    #[arg(long)]
    pub dry_run: bool,
    /// Auto-confirm prefix rename
    #[arg(long, short)]
    pub yes: bool,
}

// ---------------------------------------------------------------------------
// Maintenance
// ---------------------------------------------------------------------------

#[derive(Args)]
pub struct CheckArgs {
    /// Apply the repairs check would otherwise only describe
    #[arg(long)]
    pub fix: bool,
    /// With --fix, show the repair plan without writing anything
    #[arg(long)]
    pub dry_run: bool,
    /// With --fix, skip the confirmation prompt for repairs that delete data
    #[arg(long)]
    pub yes: bool,
}

#[derive(Args)]
pub struct CleanArgs {
    /// Show what would be done without making changes
    #[arg(long)]
    pub dry_run: bool,
    /// Also rewrite every task whose fields are out of canonical order
    #[arg(long)]
    pub normalize: bool,
}

#[derive(Args)]
pub struct ImportArgs {
    /// Markdown file to import
    pub file: String,
    /// Target track
    #[arg(long)]
    pub track: String,
    /// Insert at top of backlog
    #[arg(long)]
    pub top: bool,
    /// Insert after this task ID
    #[arg(long)]
    pub after: Option<String>,
    /// Preview without writing: report what would change, and change nothing
    #[arg(long)]
    pub dry_run: bool,
}

// ---------------------------------------------------------------------------
// Project registry
// ---------------------------------------------------------------------------

#[derive(Args)]
pub struct ProjectsCmd {
    #[command(subcommand)]
    pub action: Option<ProjectsAction>,
}

#[derive(Subcommand)]
pub enum ProjectsAction {
    /// List registered projects (default)
    List,
    /// Register a project by path
    Add(ProjectsAddArgs),
    /// Remove a project from the registry
    Remove(ProjectsRemoveArgs),
    /// Remove registry entries whose project directory no longer exists
    Prune(ProjectsPruneArgs),
}

#[derive(Args)]
pub struct ProjectsAddArgs {
    /// Path to the project directory
    pub path: String,
    /// Preview without writing: report what would change, and change nothing
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Args)]
pub struct ProjectsPruneArgs {
    /// Show what would be removed without modifying the registry
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Args)]
pub struct ProjectsRemoveArgs {
    /// Project name or path
    pub name_or_path: String,
    /// Preview without writing: report what would change, and change nothing
    #[arg(long)]
    pub dry_run: bool,
}

// ---------------------------------------------------------------------------
// Actor tokens
// ---------------------------------------------------------------------------

#[derive(Args)]
pub struct ActorCmd {
    #[command(subcommand)]
    pub action: Option<ActorAction>,
}

#[derive(Subcommand)]
pub enum ActorAction {
    /// Auto-claim a token from the frontier
    Claim(ActorClaimArgs),
    /// Claim a specific token (manual; accepts multi-char and `null`)
    Set(ActorSetArgs),
    /// Retire (tombstone) a token — leaves the frontier, stays reclaimable
    Retire(ActorRetireArgs),
    /// Merge one or more tokens into a single target (renumbers ids, retires sources)
    Merge(ActorMergeArgs),
    /// List all tokens with state and provenance
    List,
}

#[derive(Args)]
pub struct ActorMergeArgs {
    /// Source tokens to merge away — their ids are renumbered into `--into`
    #[arg(required = true)]
    pub from: Vec<String>,
    /// Target token to merge into (must be an existing, active token)
    #[arg(long)]
    pub into: String,
    /// Preview the full id remap and reference changes without writing anything
    #[arg(long)]
    pub dry_run: bool,
    /// Also rewrite id mentions inside note/spec/ref prose (skips git citations)
    #[arg(long)]
    pub rewrite_notes: bool,
}

#[derive(Args)]
pub struct ActorClaimArgs {
    /// Provenance name for the registry row (default: machine hostname)
    #[arg(long)]
    pub name: Option<String>,
    /// Claim only for this worktree (write `frame/.actor`), not the shared,
    /// clone-wide token that sibling worktrees inherit
    #[arg(long)]
    pub local: bool,
    /// Preview without writing: report what would change, and change nothing
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Args)]
pub struct ActorSetArgs {
    /// Token to claim (a single safe letter, a multi-char token, or `null`)
    pub token: String,
    /// Provenance name for the registry row (default: machine hostname)
    #[arg(long)]
    pub name: Option<String>,
    /// Claim only for this worktree (write `frame/.actor`), not the shared,
    /// clone-wide token that sibling worktrees inherit. Implied for `null`.
    #[arg(long)]
    pub local: bool,
    /// Preview without writing: report what would change, and change nothing
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Args)]
pub struct ActorRetireArgs {
    /// Token to retire
    pub token: String,
    /// Preview without writing: report what would change, and change nothing
    #[arg(long)]
    pub dry_run: bool,
}

// ---------------------------------------------------------------------------
// Recovery log
// ---------------------------------------------------------------------------

#[derive(Args)]
pub struct RecoveryCmd {
    #[command(subcommand)]
    pub action: Option<RecoveryAction>,
    /// Maximum number of entries to show (default: 10, or all with --for)
    #[arg(long)]
    pub limit: Option<usize>,
    /// Show entries after this timestamp (ISO-8601)
    #[arg(long)]
    pub since: Option<String>,
    /// Show only entries naming this task ID, or the RFC 3339 timestamp from a
    /// `conflict:` marker
    #[arg(long = "for", value_name = "ID")]
    pub for_id: Option<String>,
    /// Show only entries written from this working tree (the log is shared by
    /// every git worktree of a clone)
    #[arg(long)]
    pub here: bool,
    /// Output as JSON
    #[arg(long)]
    pub json: bool,
}

#[derive(Subcommand)]
pub enum RecoveryAction {
    /// Remove old entries
    Prune(RecoveryPruneArgs),
    /// Print the absolute path to the recovery log
    Path,
}

#[derive(Args)]
pub struct RecoveryPruneArgs {
    /// Remove entries older than this timestamp (default: 30 days ago)
    #[arg(long)]
    pub before: Option<String>,
    /// Remove all entries
    #[arg(long)]
    pub all: bool,
    /// Preview without writing: report what would change, and change nothing
    #[arg(long)]
    pub dry_run: bool,
}
