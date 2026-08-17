use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A single project entry in the global registry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectEntry {
    pub name: String,
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_accessed_tui: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_accessed_cli: Option<DateTime<Utc>>,
    /// The root of the clone's **main** working tree, when this entry is a linked
    /// git worktree rather than a project in its own right.
    ///
    /// Recorded when the entry is created and never revisited, because that is
    /// the only moment the answer is knowable: `git worktree remove` deletes the
    /// directory *and* prunes git's own record of it, so from then on nothing can
    /// say whose worktree the path used to be. It is what lets a listing group
    /// worktrees under their project and [`heal_worktrees_from`] retire them.
    ///
    /// Absent on an entry registered before frame recorded this — such an entry
    /// is treated as a project of its own until it is registered afresh.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree_of: Option<String>,
}

/// The global project registry
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProjectRegistry {
    #[serde(default)]
    pub projects: Vec<ProjectEntry>,
}

/// Get the registry file path, respecting XDG_CONFIG_HOME
pub fn registry_path() -> PathBuf {
    let config_dir = std::env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| dirs_home().join(".config"));
    config_dir.join("frame").join("projects.toml")
}

/// Get the user's home directory
fn dirs_home() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/"))
}

/// Read the project registry from a specific path.
/// If the file doesn't exist, returns an empty registry.
/// If the file is corrupted, backs it up as .bak and returns empty.
pub fn read_registry_from(path: &Path) -> ProjectRegistry {
    if !path.exists() {
        return ProjectRegistry::default();
    }

    match fs::read_to_string(path) {
        Ok(content) => match toml::from_str::<ProjectRegistry>(&content) {
            Ok(reg) => reg,
            Err(e) => {
                // Corrupted — back up and start fresh
                let bak = path.with_extension("toml.bak");
                let _ = fs::copy(path, &bak);
                eprintln!(
                    "warning: could not parse {} (backed up as {}): {}",
                    path.display(),
                    bak.display(),
                    e
                );
                ProjectRegistry::default()
            }
        },
        Err(_) => ProjectRegistry::default(),
    }
}

/// Read the project registry from the default location.
pub fn read_registry() -> ProjectRegistry {
    read_registry_from(&registry_path())
}

/// Write the project registry to a specific path.
pub fn write_registry_to(path: &Path, reg: &ProjectRegistry) -> Result<(), std::io::Error> {
    if let Some(parent) = path.parent() {
        crate::io::dryrun::create_dir_all(parent)?;
    }
    let content = toml::to_string_pretty(reg).map_err(|e| std::io::Error::other(e.to_string()))?;
    crate::io::dryrun::write(path, content)
}

/// Write the project registry to the default location.
pub fn write_registry(reg: &ProjectRegistry) -> Result<(), std::io::Error> {
    write_registry_to(&registry_path(), reg)
}

/// Where a new entry's [`ProjectEntry::worktree_of`] comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Provenance {
    /// Ask git. Costs one `git rev-parse`, and only when the entry turns out to
    /// be new — never once per command.
    Detect,
    /// Use this value as given; `None` means "a project in its own right".
    Known(Option<String>),
}

/// Register a project in the registry. If already registered (by path), updates the name.
/// Returns true if this was a new registration.
pub fn register_project(name: &str, abs_path: &Path) -> bool {
    let reg_path = registry_path();
    register_project_in(&reg_path, name, abs_path, Provenance::Detect)
}

/// Register a project in a specific registry file.
pub fn register_project_in(
    reg_path: &Path,
    name: &str,
    abs_path: &Path,
    provenance: Provenance,
) -> bool {
    let path_str = abs_path.to_string_lossy().to_string();
    let mut reg = read_registry_from(reg_path);

    if let Some(entry) = reg.projects.iter_mut().find(|e| e.path == path_str) {
        // Already registered — update name in case it changed. Provenance is
        // deliberately not revisited: it was resolved when the entry was created,
        // and re-resolving it here would put a `git` call on every command.
        entry.name = name.to_string();
        let _ = write_registry_to(reg_path, &reg);
        return false;
    }

    let worktree_of = match provenance {
        Provenance::Detect => detect_worktree_of(abs_path),
        Provenance::Known(value) => value,
    };

    reg.projects.push(ProjectEntry {
        name: name.to_string(),
        path: path_str,
        last_accessed_tui: None,
        last_accessed_cli: None,
        worktree_of,
    });
    let _ = write_registry_to(reg_path, &reg);
    true
}

/// The main working tree's root, when `root` is a linked git worktree.
///
/// A bare or `--separate-git-dir` repo yields `None` even though it is linked:
/// there is no main working tree to point a row at, so the honest answer is "a
/// project of its own" rather than a guessed path.
fn detect_worktree_of(root: &Path) -> Option<String> {
    match crate::io::git::worktree_kind(&root.join("frame")) {
        crate::io::git::WorktreeKind::Linked {
            main_root: Some(main_root),
        } => Some(main_root.to_string_lossy().to_string()),
        _ => None,
    }
}

/// Update the last_accessed_tui timestamp for a project path.
pub fn touch_tui(abs_path: &Path) {
    let reg_path = registry_path();
    let path_str = abs_path.to_string_lossy().to_string();
    let mut reg = read_registry_from(&reg_path);
    if let Some(entry) = reg.projects.iter_mut().find(|e| e.path == path_str) {
        entry.last_accessed_tui = Some(Utc::now());
        let _ = write_registry_to(&reg_path, &reg);
    }
}

/// Update the last_accessed_cli timestamp for a project path.
pub fn touch_cli(abs_path: &Path) {
    let reg_path = registry_path();
    let path_str = abs_path.to_string_lossy().to_string();
    let mut reg = read_registry_from(&reg_path);
    if let Some(entry) = reg.projects.iter_mut().find(|e| e.path == path_str) {
        entry.last_accessed_cli = Some(Utc::now());
        let _ = write_registry_to(&reg_path, &reg);
    }
}

/// Remove a project from the registry by name or path.
/// Returns the removed entry, or None if not found.
/// If name is ambiguous (multiple matches), returns Err with count.
pub fn remove_project(name_or_path: &str) -> Result<Option<ProjectEntry>, String> {
    let reg_path = registry_path();
    remove_project_from(&reg_path, name_or_path)
}

/// Remove a project from a specific registry file.
pub fn remove_project_from(
    reg_path: &Path,
    name_or_path: &str,
) -> Result<Option<ProjectEntry>, String> {
    let mut reg = read_registry_from(reg_path);

    // Try exact path match first
    let abs_path = fs::canonicalize(name_or_path).ok();
    if let Some(ref abs) = abs_path {
        let abs_str = abs.to_string_lossy().to_string();
        if let Some(idx) = reg.projects.iter().position(|e| e.path == abs_str) {
            let removed = reg.projects.remove(idx);
            let _ = write_registry_to(reg_path, &reg);
            return Ok(Some(removed));
        }
    }

    // Also try raw string match on path
    if let Some(idx) = reg.projects.iter().position(|e| e.path == name_or_path) {
        let removed = reg.projects.remove(idx);
        let _ = write_registry_to(reg_path, &reg);
        return Ok(Some(removed));
    }

    // Try name match
    let matches: Vec<usize> = reg
        .projects
        .iter()
        .enumerate()
        .filter(|(_, e)| e.name == name_or_path)
        .map(|(i, _)| i)
        .collect();

    match matches.len() {
        0 => Ok(None),
        1 => {
            let removed = reg.projects.remove(matches[0]);
            let _ = write_registry_to(reg_path, &reg);
            Ok(Some(removed))
        }
        // Worktrees of one clone all carry the project's committed name, so this
        // is the ordinary case rather than a rare one. Listing the candidates
        // makes the instruction followable — telling someone to specify a path
        // without saying which paths there are sends them off to look it up.
        n => Err(format!(
            "ambiguous: {} projects named \"{}\". Specify by path instead:\n{}",
            n,
            name_or_path,
            matches
                .iter()
                .map(|i| format!("  {}", reg.projects[*i].path))
                .collect::<Vec<_>>()
                .join("\n")
        )),
    }
}

/// True if a registered entry's project directory still exists on disk.
/// Mirrors the "(not found)" criterion used by `projects list` and the picker:
/// the path must contain a `frame/` directory.
pub fn entry_exists(entry: &ProjectEntry) -> bool {
    Path::new(&entry.path).join("frame").exists()
}

/// Whether `fr projects prune` should remove this entry.
///
/// For a project this is [`entry_exists`]'s criterion. For a **worktree** it is
/// the working tree's own directory instead: a live worktree checked out to a
/// branch that predates the project has no `frame/` in it, and pruning that row
/// would remove something that is sitting right there.
pub fn is_prunable(entry: &ProjectEntry) -> bool {
    if entry.worktree_of.is_some() {
        return !Path::new(&entry.path).exists();
    }
    !entry_exists(entry)
}

/// Remove registry entries whose project directory no longer exists.
/// Returns the removed entries (empty if nothing was pruned).
pub fn prune_missing() -> Vec<ProjectEntry> {
    prune_missing_from(&registry_path())
}

/// Remove not-found entries from a specific registry file.
pub fn prune_missing_from(reg_path: &Path) -> Vec<ProjectEntry> {
    let mut reg = read_registry_from(reg_path);
    let mut removed = Vec::new();
    reg.projects.retain(|e| {
        let keep = !is_prunable(e);
        if !keep {
            removed.push(e.clone());
        }
        keep
    });
    if !removed.is_empty() {
        let _ = write_registry_to(reg_path, &reg);
    }
    removed
}

/// Retire the entries of git worktrees that no longer exist.
///
/// Unlike [`prune_missing`], this needs no asking: a worktree's entry is
/// *derivative*. The project it is a view of has its own row, and everything a
/// removed worktree held that exists nowhere else — the ID frontier, the recovery
/// log — lives in the git common directory, which the removal does not touch. So
/// dropping the row silently costs nothing, where dropping a missing *project*
/// silently could discard the only record of where it was.
///
/// Two guards keep it to that case. The entry must carry
/// [`ProjectEntry::worktree_of`], so a project is never taken for a worktree; and
/// the parent's root must still be present, so an unmounted volume or a moved
/// clone — where the worktrees are missing for a reason that will reverse — takes
/// nothing with it.
///
/// Returns what it removed, empty when nothing was.
pub fn heal_worktrees() -> Vec<ProjectEntry> {
    heal_worktrees_from(&registry_path())
}

/// Retire dead worktree entries in a specific registry file.
pub fn heal_worktrees_from(reg_path: &Path) -> Vec<ProjectEntry> {
    let mut reg = read_registry_from(reg_path);
    let mut removed = Vec::new();
    reg.projects.retain(|e| {
        let keep = !is_dead_worktree(e);
        if !keep {
            removed.push(e.clone());
        }
        keep
    });
    if !removed.is_empty() {
        let _ = write_registry_to(reg_path, &reg);
    }
    removed
}

/// Whether an entry is a worktree that has been removed.
///
/// The test is on the working tree's own directory, not on the `frame/` inside it
/// the way [`entry_exists`] tests: a live worktree switched to a branch that
/// predates the project has no `frame/` and must survive a listing.
fn is_dead_worktree(entry: &ProjectEntry) -> bool {
    let Some(parent) = &entry.worktree_of else {
        return false;
    };
    !Path::new(&entry.path).exists() && Path::new(parent).exists()
}

/// How a listing orders its top-level projects.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectSort {
    /// Most recently opened in the TUI first.
    RecentTui,
    /// Most recently used on the CLI first.
    RecentCli,
    /// By name, case-insensitively.
    Name,
}

/// One line of a project listing: a project, or one of its worktrees.
#[derive(Debug, Clone)]
pub struct ProjectRow {
    pub entry: ProjectEntry,
    /// The branch this worktree has checked out. `None` on a project row, on a
    /// detached worktree, and when git could not be asked — filled in by
    /// [`label_branches`], which is separate so ordering stays testable without
    /// a repo.
    pub branch: Option<String>,
    /// Whether this row is a worktree shown underneath its project.
    pub nested: bool,
}

impl ProjectRow {
    /// What to print in the name column, without indentation.
    ///
    /// A worktree's rows all carry the same project name — `project.toml` is
    /// committed — so the name is no use for telling them apart. The branch is,
    /// and it is what a person calls the worktree. Failing that (detached, or git
    /// unavailable) the directory name at least differs between them.
    pub fn label(&self) -> String {
        if !self.nested {
            return self.entry.name.clone();
        }
        let name = self.branch.clone().unwrap_or_else(|| {
            Path::new(&self.entry.path)
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| self.entry.name.clone())
        });
        format!("\u{2514} {}", name)
    }
}

/// Order entries for display: each project, then its live worktrees.
///
/// A worktree whose project is not itself registered stays top-level — it is
/// still marked as a worktree, but there is no row to nest it under. Sorting
/// applies within each level, so a worktree used more recently than its project
/// never floats above it and away from the row that explains it.
pub fn arrange(entries: Vec<ProjectEntry>, sort: ProjectSort) -> Vec<ProjectRow> {
    let roots: Vec<String> = entries.iter().map(|e| e.path.clone()).collect();

    // A worktree is nested only when its parent has a row of its own.
    let (mut children, mut parents): (Vec<ProjectEntry>, Vec<ProjectEntry>) =
        entries.into_iter().partition(|e| {
            e.worktree_of
                .as_ref()
                .is_some_and(|p| roots.iter().any(|root| same_path(root, p)))
        });

    // An orphaned worktree stayed in `parents`, so it gets a top-level row of its
    // own rather than disappearing for want of somewhere to sit.
    sort_entries(&mut parents, sort);
    sort_entries(&mut children, sort);

    let mut rows = Vec::with_capacity(parents.len() + children.len());
    for parent in parents {
        let mine: Vec<ProjectEntry> = children
            .iter()
            .filter(|c| {
                c.worktree_of
                    .as_ref()
                    .is_some_and(|p| same_path(&parent.path, p))
            })
            .cloned()
            .collect();
        rows.push(ProjectRow {
            entry: parent,
            branch: None,
            nested: false,
        });
        rows.extend(mine.into_iter().map(|entry| ProjectRow {
            entry,
            branch: None,
            nested: true,
        }));
    }
    rows
}

fn sort_entries(entries: &mut [ProjectEntry], sort: ProjectSort) {
    match sort {
        ProjectSort::Name => entries.sort_by_key(|e| e.name.to_lowercase()),
        ProjectSort::RecentTui => entries.sort_by(|a, b| {
            b.last_accessed_tui
                .unwrap_or_default()
                .cmp(&a.last_accessed_tui.unwrap_or_default())
        }),
        ProjectSort::RecentCli => entries.sort_by(|a, b| {
            b.last_accessed_cli
                .unwrap_or_default()
                .cmp(&a.last_accessed_cli.unwrap_or_default())
        }),
    }
}

/// Whether two recorded paths name the same directory.
///
/// String equality is the common case — the registry stores absolute paths, and
/// `worktree_of` comes from git canonicalized. Canonicalizing both covers a clone
/// reached through a symlink, where the two spellings differ but the directory
/// does not; a path that no longer exists cannot be canonicalized, and falls back
/// to the comparison it came in with.
fn same_path(a: &str, b: &str) -> bool {
    if a == b {
        return true;
    }
    match (fs::canonicalize(a), fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// What a survey of the registered projects learned from git: for each linked
/// worktree, the clone's main working tree and the branch it has checked out.
///
/// Keys are canonicalized, since they are compared against paths on disk.
#[derive(Debug, Default)]
pub struct WorktreeSurvey {
    trees: Vec<(PathBuf, Option<PathBuf>, Option<String>)>,
}

impl WorktreeSurvey {
    /// The branch a registered path has checked out, when git named one.
    pub fn branch_of(&self, path: &str) -> Option<String> {
        let canonical = fs::canonicalize(path).ok()?;
        self.trees
            .iter()
            .find(|(p, _, _)| *p == canonical)
            .and_then(|(_, _, branch)| branch.clone())
    }
}

/// Ask git about the registered projects, and record what it says.
///
/// This does two jobs from the same calls. It **stamps provenance** onto any entry
/// that turns out to be a linked worktree — which is how an entry registered
/// before frame recorded provenance, or by another `fr` on this machine, comes to
/// group and to retire itself. And it reports each worktree's **branch** for the
/// listing to label rows with.
///
/// One `git worktree list` per *clone*, not per entry: asking from any working
/// tree of a clone returns the whole set, so an entry already covered by an
/// earlier answer costs nothing. Called only where a person is about to read a
/// listing — never on the path of an ordinary command.
pub fn survey_worktrees(reg_path: &Path, entries: &mut [ProjectEntry]) -> WorktreeSurvey {
    let mut survey = WorktreeSurvey::default();

    for entry in entries.iter() {
        let Ok(path) = fs::canonicalize(&entry.path) else {
            continue; // gone from disk: nothing to ask, and nothing to relabel
        };
        if survey.trees.iter().any(|(p, _, _)| *p == path) {
            continue; // already covered by another tree of the same clone
        }
        let Some(trees) = crate::io::git::worktree_list(&path) else {
            continue; // not a repo, or git unavailable
        };
        // `git worktree list` puts the main working tree first; every other
        // record is a linked worktree of it.
        let mut iter = trees.into_iter();
        let Some(main) = iter.next() else { continue };
        survey.trees.push((main.path.clone(), None, main.branch));
        for tree in iter {
            survey
                .trees
                .push((tree.path, Some(main.path.clone()), tree.branch));
        }
    }

    // Stamp what git said, for the entries that did not already know it.
    let mut changed = false;
    for entry in entries.iter_mut() {
        let Ok(path) = fs::canonicalize(&entry.path) else {
            continue;
        };
        let Some((_, main, _)) = survey.trees.iter().find(|(p, _, _)| *p == path) else {
            continue;
        };
        let expected = main.as_ref().map(|m| m.to_string_lossy().to_string());
        if entry.worktree_of != expected {
            entry.worktree_of = expected;
            changed = true;
        }
    }
    if changed {
        let mut reg = read_registry_from(reg_path);
        for entry in entries.iter() {
            if let Some(stored) = reg.projects.iter_mut().find(|e| e.path == entry.path) {
                stored.worktree_of = entry.worktree_of.clone();
            }
        }
        let _ = write_registry_to(reg_path, &reg);
    }

    survey
}

/// Fill in [`ProjectRow::branch`] from a survey. Rows whose directory is gone,
/// and clones git could not answer for, keep `None` — [`ProjectRow::label`] falls
/// back to the directory name.
pub fn label_branches(rows: &mut [ProjectRow], survey: &WorktreeSurvey) {
    for row in rows.iter_mut() {
        if row.entry.worktree_of.is_some() {
            row.branch = survey.branch_of(&row.entry.path);
        }
    }
}

/// Remove a project from the registry by exact path string.
pub fn remove_by_path(path_str: &str) -> Option<ProjectEntry> {
    let reg_path = registry_path();
    let mut reg = read_registry_from(&reg_path);
    if let Some(idx) = reg.projects.iter().position(|e| e.path == path_str) {
        let removed = reg.projects.remove(idx);
        let _ = write_registry_to(&reg_path, &reg);
        Some(removed)
    } else {
        None
    }
}

/// Abbreviate a path by replacing $HOME with ~
pub fn abbreviate_path(path: &str) -> String {
    if let Ok(home) = std::env::var("HOME")
        && let Some(rest) = path.strip_prefix(&home)
    {
        return format!("~{}", rest);
    }
    path.to_string()
}

/// Format a relative time string like "2 min ago", "yesterday", "3 days ago"
pub fn relative_time(dt: &DateTime<Utc>) -> String {
    let now = Utc::now();
    let duration = now.signed_duration_since(*dt);

    let secs = duration.num_seconds();
    if secs < 60 {
        return "just now".to_string();
    }
    let mins = duration.num_minutes();
    if mins < 60 {
        return format!("{} min ago", mins);
    }
    let hours = duration.num_hours();
    if hours < 24 {
        return format!("{} hr ago", hours);
    }
    let days = duration.num_days();
    if days == 1 {
        return "yesterday".to_string();
    }
    if days < 7 {
        return format!("{} days ago", days);
    }
    let weeks = days / 7;
    if weeks < 5 {
        return format!("{} weeks ago", weeks);
    }
    format!("{} months ago", days / 30)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn temp_registry() -> (TempDir, PathBuf) {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("frame").join("projects.toml");
        (tmp, path)
    }

    /// Register without asking git, so ordering and healing stay testable
    /// without a repo on disk.
    fn register(reg_path: &Path, name: &str, path: &Path) -> bool {
        register_project_in(reg_path, name, path, Provenance::Known(None))
    }

    fn entry(name: &str, path: &str, worktree_of: Option<&str>) -> ProjectEntry {
        ProjectEntry {
            name: name.to_string(),
            path: path.to_string(),
            last_accessed_tui: None,
            last_accessed_cli: None,
            worktree_of: worktree_of.map(|s| s.to_string()),
        }
    }

    #[test]
    fn test_empty_registry() {
        let (_tmp, path) = temp_registry();
        let reg = read_registry_from(&path);
        assert!(reg.projects.is_empty());
    }

    #[test]
    fn test_register_and_read() {
        let (_tmp, path) = temp_registry();
        let is_new = register(&path, "test-proj", Path::new("/tmp/test"));
        assert!(is_new);
        let reg = read_registry_from(&path);
        assert_eq!(reg.projects.len(), 1);
        assert_eq!(reg.projects[0].name, "test-proj");
        assert_eq!(reg.projects[0].path, "/tmp/test");
    }

    #[test]
    fn test_register_duplicate_path() {
        let (_tmp, path) = temp_registry();
        register(&path, "proj", Path::new("/tmp/test"));
        let is_new = register(&path, "proj-renamed", Path::new("/tmp/test"));
        assert!(!is_new);
        let reg = read_registry_from(&path);
        assert_eq!(reg.projects.len(), 1);
        assert_eq!(reg.projects[0].name, "proj-renamed");
    }

    #[test]
    fn test_remove_by_name() {
        let (_tmp, path) = temp_registry();
        register(&path, "my-proj", Path::new("/tmp/my-proj"));
        let removed = remove_project_from(&path, "my-proj").unwrap();
        assert!(removed.is_some());
        assert_eq!(removed.unwrap().name, "my-proj");
        let reg = read_registry_from(&path);
        assert!(reg.projects.is_empty());
    }

    #[test]
    fn test_remove_not_found() {
        let (_tmp, path) = temp_registry();
        let removed = remove_project_from(&path, "nonexistent").unwrap();
        assert!(removed.is_none());
    }

    #[test]
    fn test_prune_missing() {
        let (_tmp, path) = temp_registry();
        // A live project (its `frame/` dir exists on disk).
        let live = TempDir::new().unwrap();
        fs::create_dir_all(live.path().join("frame")).unwrap();
        register(&path, "live", live.path());
        // A stale entry whose directory no longer exists.
        register(&path, "ghost", Path::new("/tmp/does-not-exist-xyz"));

        let removed = prune_missing_from(&path);
        assert_eq!(removed.len(), 1);
        assert_eq!(removed[0].name, "ghost");

        let reg = read_registry_from(&path);
        assert_eq!(reg.projects.len(), 1);
        assert_eq!(reg.projects[0].name, "live");
    }

    #[test]
    fn test_prune_missing_noop_when_all_live() {
        let (_tmp, path) = temp_registry();
        let live = TempDir::new().unwrap();
        fs::create_dir_all(live.path().join("frame")).unwrap();
        register(&path, "live", live.path());
        assert!(prune_missing_from(&path).is_empty());
        assert_eq!(read_registry_from(&path).projects.len(), 1);
    }

    /// A removed worktree's row goes without being asked; a project's does not.
    #[test]
    fn heal_retires_dead_worktrees_and_leaves_projects_alone() {
        let (_tmp, path) = temp_registry();
        let live = TempDir::new().unwrap();
        let main = live.path().join("main");
        fs::create_dir_all(main.join("frame")).unwrap();
        let main_str = main.to_string_lossy().to_string();

        let mut reg = ProjectRegistry::default();
        reg.projects.push(entry("demo", &main_str, None));
        // A worktree that is gone: its row is derivative, so it goes.
        let gone = live.path().join("wt-gone");
        reg.projects
            .push(entry("demo", &gone.to_string_lossy(), Some(&main_str)));
        // A worktree that is still there stays, even with no `frame/` inside it —
        // a branch predating the project is not a removed worktree.
        let live_wt = live.path().join("wt-live");
        fs::create_dir_all(&live_wt).unwrap();
        reg.projects
            .push(entry("demo", &live_wt.to_string_lossy(), Some(&main_str)));
        // A missing *project* is left for `fr projects prune` to ask about.
        reg.projects
            .push(entry("ghost", "/tmp/does-not-exist-xyz", None));
        write_registry_to(&path, &reg).unwrap();

        let removed = heal_worktrees_from(&path);
        assert_eq!(removed.len(), 1);
        assert!(removed[0].path.ends_with("wt-gone"));

        let kept: Vec<String> = read_registry_from(&path)
            .projects
            .into_iter()
            .map(|e| e.path)
            .collect();
        assert_eq!(
            kept,
            vec![
                main_str,
                live_wt.to_string_lossy().to_string(),
                "/tmp/does-not-exist-xyz".to_string()
            ]
        );
    }

    /// The guard against a whole volume going missing: with the parent gone too,
    /// the worktree rows are not evidence of a removal.
    #[test]
    fn heal_keeps_worktrees_when_the_parent_is_missing_too() {
        let (_tmp, path) = temp_registry();
        let mut reg = ProjectRegistry::default();
        reg.projects
            .push(entry("demo", "/nowhere-xyz/wt", Some("/nowhere-xyz/main")));
        write_registry_to(&path, &reg).unwrap();

        assert!(heal_worktrees_from(&path).is_empty());
        assert_eq!(read_registry_from(&path).projects.len(), 1);
    }

    #[test]
    fn arrange_nests_worktrees_under_their_project() {
        let older = Utc::now() - chrono::Duration::hours(2);
        let newer = Utc::now();
        let mut project = entry("demo", "/p/demo", None);
        project.last_accessed_cli = Some(older);
        let mut worktree = entry("demo", "/p/demo/.wt/alt", Some("/p/demo"));
        worktree.last_accessed_cli = Some(newer);
        let mut other = entry("other", "/p/other", None);
        other.last_accessed_cli = Some(newer);

        let rows = arrange(
            vec![worktree.clone(), other.clone(), project.clone()],
            ProjectSort::RecentCli,
        );
        let shape: Vec<(&str, bool)> = rows
            .iter()
            .map(|r| (r.entry.path.as_str(), r.nested))
            .collect();
        // `other` is the most recent project, and the worktree stays with `demo`
        // rather than floating to the top away from the row that explains it.
        assert_eq!(
            shape,
            vec![
                ("/p/other", false),
                ("/p/demo", false),
                ("/p/demo/.wt/alt", true)
            ]
        );
    }

    /// A worktree beside its parent nests just the same — the relationship is
    /// git's answer, not a path prefix.
    #[test]
    fn arrange_nests_a_sibling_worktree() {
        let rows = arrange(
            vec![
                entry("demo", "/p/demo-alt", Some("/p/demo")),
                entry("demo", "/p/demo", None),
            ],
            ProjectSort::Name,
        );
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].entry.path, "/p/demo");
        assert!(rows[1].nested);
    }

    /// A worktree whose clone was never registered still gets a row.
    #[test]
    fn arrange_keeps_an_orphaned_worktree_top_level() {
        let rows = arrange(
            vec![entry("demo", "/p/wt", Some("/p/unregistered"))],
            ProjectSort::Name,
        );
        assert_eq!(rows.len(), 1);
        assert!(!rows[0].nested);
        assert_eq!(rows[0].label(), "demo");
    }

    #[test]
    fn nested_rows_are_labelled_by_branch_then_directory() {
        let mut row = ProjectRow {
            entry: entry("demo", "/p/demo/.wt/alt", Some("/p/demo")),
            branch: Some("feature".into()),
            nested: true,
        };
        assert_eq!(row.label(), "\u{2514} feature");
        // Detached, or git unavailable: the directory name still tells them apart.
        row.branch = None;
        assert_eq!(row.label(), "\u{2514} alt");
    }

    #[test]
    fn ambiguous_removal_names_the_candidates() {
        let (_tmp, path) = temp_registry();
        register(&path, "demo", Path::new("/p/demo"));
        register(&path, "demo", Path::new("/p/demo-alt"));
        let err = remove_project_from(&path, "demo").unwrap_err();
        assert!(err.contains("/p/demo"), "{err}");
        assert!(err.contains("/p/demo-alt"), "{err}");
        // Nothing was removed on the way to the error.
        assert_eq!(read_registry_from(&path).projects.len(), 2);
    }

    #[test]
    fn test_abbreviate_path() {
        let home = std::env::var("HOME").unwrap_or_default();
        let p = format!("{}/code/frame", home);
        let abbrev = abbreviate_path(&p);
        assert!(abbrev.starts_with("~/"));
    }

    #[test]
    fn test_relative_time() {
        let now = Utc::now();
        assert_eq!(relative_time(&now), "just now");

        let five_min_ago = now - chrono::Duration::minutes(5);
        assert_eq!(relative_time(&five_min_ago), "5 min ago");

        let yesterday = now - chrono::Duration::days(1);
        assert_eq!(relative_time(&yesterday), "yesterday");

        let three_days = now - chrono::Duration::days(3);
        assert_eq!(relative_time(&three_days), "3 days ago");
    }

    #[test]
    fn test_corrupted_registry_backup() {
        let (_tmp, path) = temp_registry();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "not valid toml [[[").unwrap();
        let reg = read_registry_from(&path);
        assert!(reg.projects.is_empty());
        // Backup should exist
        let bak = path.with_extension("toml.bak");
        assert!(bak.exists());
    }

    #[test]
    fn test_round_trip_serialization() {
        let (_tmp, path) = temp_registry();
        let mut reg = ProjectRegistry::default();
        reg.projects.push(ProjectEntry {
            name: "test".to_string(),
            path: "/tmp/test".to_string(),
            last_accessed_tui: Some(Utc::now()),
            last_accessed_cli: None,
            worktree_of: Some("/tmp/main".to_string()),
        });
        write_registry_to(&path, &reg).unwrap();
        let loaded = read_registry_from(&path);
        assert_eq!(loaded.projects.len(), 1);
        assert_eq!(loaded.projects[0].name, "test");
        assert!(loaded.projects[0].last_accessed_tui.is_some());
        assert!(loaded.projects[0].last_accessed_cli.is_none());
        assert_eq!(loaded.projects[0].worktree_of.as_deref(), Some("/tmp/main"));
    }

    /// A registry written before frame recorded provenance still loads, and its
    /// entries read as projects of their own.
    #[test]
    fn test_registry_without_provenance_still_loads() {
        let (_tmp, path) = temp_registry();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "[[projects]]\nname = \"demo\"\npath = \"/p/demo\"\n").unwrap();
        let reg = read_registry_from(&path);
        assert_eq!(reg.projects.len(), 1);
        assert_eq!(reg.projects[0].worktree_of, None);
    }
}
