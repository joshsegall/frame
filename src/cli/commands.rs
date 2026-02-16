use clap::{Args, Parser, Subcommand};

#[derive(Parser)]
#[command(name = "fr", about = concat!("[>] frame v", env!("CARGO_PKG_VERSION"), " - your backlog is plain text"), version)]
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
    /// Validate project integrity
    Check,
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
    /// Add file reference
    Ref(RefArgs),
    /// Set spec reference
    Spec(SpecArgs),
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
    /// View or manage the recovery log
    Recovery(RecoveryCmd),
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
    /// Also search archived tasks
    #[arg(short, long)]
    pub archive: bool,
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
}

#[derive(Args)]
pub struct PushArgs {
    /// Track to push the task to
    pub track: String,
    /// Task title
    pub title: String,
}

#[derive(Args)]
pub struct SubArgs {
    /// Parent task ID
    pub id: String,
    /// Subtask title
    pub title: String,
}

#[derive(Args)]
pub struct StateArgs {
    /// Task ID
    pub id: String,
    /// New state (todo, active, blocked, done, parked)
    pub state: String,
}

#[derive(Args)]
pub struct StartArgs {
    /// Task ID
    pub id: String,
}

#[derive(Args)]
pub struct DoneArgs {
    /// Task ID
    pub id: String,
}

#[derive(Args)]
pub struct TagArgs {
    /// Task ID
    pub id: String,
    /// Action: "add" or "rm"
    pub action: String,
    /// Tag name
    pub tag: String,
}

#[derive(Args)]
pub struct DepArgs {
    /// Task ID
    pub id: String,
    /// Action: "add" or "rm"
    pub action: String,
    /// Dependency task ID
    pub dep_id: String,
}

#[derive(Args)]
pub struct NoteArgs {
    /// Task ID
    pub id: String,
    /// Note text
    pub text: String,
    /// Replace existing note instead of appending
    #[arg(long)]
    pub replace: bool,
}

#[derive(Args)]
pub struct RefArgs {
    /// Task ID
    pub id: String,
    /// File path
    pub path: String,
}

#[derive(Args)]
pub struct SpecArgs {
    /// Task ID
    pub id: String,
    /// Spec path (e.g., doc/spec.md#section)
    pub path: String,
}

#[derive(Args)]
pub struct TitleArgs {
    /// Task ID
    pub id: String,
    /// New title
    pub title: String,
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
    #[arg(long)]
    pub track: Option<String>,
    /// Promote subtask to top-level
    #[arg(long)]
    pub promote: bool,
    /// Reparent under the given task ID
    #[arg(long)]
    pub parent: Option<String>,
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
}

#[derive(Args)]
pub struct DeleteArgs {
    /// Task IDs to delete
    #[arg(required = true)]
    pub ids: Vec<String>,
    /// Skip confirmation prompt
    #[arg(long)]
    pub yes: bool,
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
}

#[derive(Args)]
pub struct TrackIdArg {
    /// Track ID
    pub id: String,
}

#[derive(Args)]
pub struct CcFocusArgs {
    /// Track ID (omit with --clear)
    pub id: Option<String>,
    /// Clear the cc-focus setting
    #[arg(long)]
    pub clear: bool,
}

#[derive(Args)]
pub struct TrackMvArgs {
    /// Track ID
    pub id: String,
    /// New position (0-indexed among active tracks)
    pub position: usize,
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
pub struct CleanArgs {
    /// Show what would be done without making changes
    #[arg(long)]
    pub dry_run: bool,
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
}

#[derive(Args)]
pub struct ProjectsAddArgs {
    /// Path to the project directory
    pub path: String,
}

#[derive(Args)]
pub struct ProjectsRemoveArgs {
    /// Project name or path
    pub name_or_path: String,
}

// ---------------------------------------------------------------------------
// Recovery log
// ---------------------------------------------------------------------------

#[derive(Args)]
pub struct RecoveryCmd {
    #[command(subcommand)]
    pub action: Option<RecoveryAction>,
    /// Maximum number of entries to show (default: 10)
    #[arg(long)]
    pub limit: Option<usize>,
    /// Show entries after this timestamp (ISO-8601)
    #[arg(long)]
    pub since: Option<String>,
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
}
