//! Dependency-tree traversal.
//!
//! One traversal, rendered two ways: `cli::output::format_dep_tree` for humans
//! and `cli::output::dep_tree_to_json` for `--json`. Building the tree once and
//! rendering it twice is deliberate — a listing implemented separately per
//! surface is what `b664a3e` was, and `tests/parity.rs` exists because of it.

use std::cell::OnceCell;
use std::collections::{HashMap, HashSet};
use std::path::Path;

use crate::model::Project;
use crate::model::task::{Metadata, Task, TaskState};

/// Why a node in the tree looks the way it does.
///
/// The human surface used to have two names for these four cases, with
/// `(circular)` covering both a genuine cycle and a task simply reached twice.
/// A diamond — `A` depends on `B` and `C`, both of which depend on `D` — is the
/// ordinary shape of a real backlog, and it was being reported as a cycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DepStatus {
    /// Found, and its own dependencies are expanded below.
    Resolved,
    /// This id is its own ancestor on this branch. A real cycle.
    Cycle,
    /// Already expanded elsewhere in this tree. Not an error — the full record
    /// is somewhere else in the same output.
    Repeat,
    /// Found in an archive rather than a live track: the dependency is **done**,
    /// and its record moved out of the working file when it was archived.
    ///
    /// Terminal, like `Cycle` and `Repeat`, but for a different reason: an
    /// archived task's own dependencies are history — every one of them was
    /// satisfied before it could be marked done — so expanding them answers a
    /// question nobody asked and reads a second file per node to do it.
    Archived,
    /// No task anywhere in the project holds this id — not a live track, not an
    /// archive.
    Missing,
}

/// A node in a dependency tree.
///
/// `track_id`, `title` and `state` are populated for [`DepStatus::Resolved`]
/// and [`DepStatus::Archived`], the two statuses that found a task: a `Cycle` or
/// `Repeat` node is a pointer to a record that appears elsewhere in the same
/// tree, and a `Missing` one has no record at all. `tags` is `Resolved` only —
/// an archived task's tags describe work that is over.
#[derive(Debug, Clone)]
pub struct DepNode {
    pub id: String,
    pub status: DepStatus,
    pub track_id: Option<String>,
    pub title: Option<String>,
    pub state: Option<TaskState>,
    pub tags: Vec<String>,
    pub deps: Vec<DepNode>,
}

impl DepNode {
    fn terminal(id: &str, status: DepStatus) -> Self {
        DepNode {
            id: id.to_string(),
            status,
            track_id: None,
            title: None,
            state: None,
            tags: Vec::new(),
            deps: Vec::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// The archive, as part of the dependency universe
// ---------------------------------------------------------------------------

/// What an archive still knows about a task it holds.
///
/// Enough to name it in a dependency tree and no more. The task's own `dep:`
/// lines are deliberately not carried: nothing expands an archived node.
#[derive(Debug, Clone)]
pub struct ArchivedTask {
    /// The track it was archived from, derived from the archive's filename.
    pub track_id: String,
    pub title: String,
    pub state: TaskState,
}

/// The tasks the archives hold, read at most once and only if something asks.
///
/// **A dep pointing into an archive is satisfied, not missing.** An archive
/// holds done work — `fr clean` moves tasks there once they are resolved, and
/// `fr track archive` moves a track whole — so a `dep:` whose target was
/// archived describes a blocker that is finished, which is the most benign
/// state a dependency can be in. Resolving deps against live tracks alone made
/// `fr clean` manufacture errors: archiving a task past the done threshold
/// turned every dep on it into a `dangling_dep`, reported by the *next* `fr
/// check` rather than the run that caused it. `tests/conservation.rs` has read
/// the archives as part of the project since it was written (claim 5, "every
/// dep that resolved still resolves"); this is the product agreeing with it.
///
/// **Lazy, because auto-clean is not.** `clean.auto_clean` runs a full clean
/// after every file reload in the TUI, and an archive is the one frame file
/// with no size ceiling — megabytes, in a project that has been running a
/// while. Nothing here is read until a dep actually fails against the live
/// tracks, which in a healthy project is never.
pub struct ArchiveIndex<'a> {
    frame_dir: &'a Path,
    tasks: OnceCell<HashMap<String, ArchivedTask>>,
}

impl<'a> ArchiveIndex<'a> {
    pub fn new(frame_dir: &'a Path) -> Self {
        ArchiveIndex {
            frame_dir,
            tasks: OnceCell::new(),
        }
    }

    /// Whether any archive holds this id.
    pub fn contains(&self, id: &str) -> bool {
        self.get(id).is_some()
    }

    /// The archived task with this id, reading the archives on first use.
    ///
    /// **First archive wins**, matching how a live lookup takes the first match
    /// in track order — `archived_task_lists` returns done-task archives sorted
    /// by track id, then whole archived tracks, so the answer is stable across
    /// runs rather than dependent on `read_dir` order.
    pub fn get(&self, id: &str) -> Option<&ArchivedTask> {
        self.tasks.get_or_init(|| self.load()).get(id)
    }

    fn load(&self) -> HashMap<String, ArchivedTask> {
        let mut out = HashMap::new();
        for list in crate::io::project_io::archived_task_lists(self.frame_dir) {
            for task in &list.tasks {
                index_task(task, &list.track_id, &mut out);
            }
        }
        out
    }
}

fn index_task(task: &Task, track_id: &str, out: &mut HashMap<String, ArchivedTask>) {
    if let Some(id) = &task.id {
        out.entry(id.to_string()).or_insert_with(|| ArchivedTask {
            track_id: track_id.to_string(),
            title: task.title.clone(),
            state: task.state,
        });
    }
    for sub in &task.subtasks {
        index_task(sub, track_id, out);
    }
}

/// The ids a task declares as dependencies, in declaration order.
pub fn task_deps(task: &Task) -> Vec<String> {
    let mut deps = Vec::new();
    for m in &task.metadata {
        if let Metadata::Dep(d) = m {
            deps.extend(d.iter().cloned());
        }
    }
    deps
}

fn find_task<'a>(project: &'a Project, id: &str) -> Option<(&'a str, &'a Task)> {
    project.tracks.iter().find_map(|(track_id, track)| {
        crate::ops::task_ops::find_task_in_track(track, id).map(|task| (track_id.as_str(), task))
    })
}

/// Build the dependency tree rooted at `root_id`.
///
/// The root is a [`DepStatus::Missing`] node when no task holds that id; the
/// caller decides whether that is an error. It is [`DepStatus::Archived`] when
/// the id belongs to work that has been archived — which for a *root* the
/// caller also decides about, since a tree rooted at history has nothing under
/// it to walk.
pub fn dep_tree(project: &Project, root_id: &str) -> DepNode {
    let mut path = Vec::new();
    let mut expanded = HashSet::new();
    let archive = ArchiveIndex::new(&project.frame_dir);
    build(project, &archive, root_id, &mut path, &mut expanded)
}

/// Two sets, checked in this order, and the order matters.
///
/// `path` is the ancestor chain of the branch being walked, pushed on the way
/// down and **popped on the way back up**, so it holds exactly the ids that
/// reaching this one again would make a cycle. The previous implementation used
/// a single set that was never popped, which made every re-encounter a "cycle".
///
/// `expanded` is every id expanded anywhere so far and is never cleared. It
/// serves two purposes at once: it names the `Repeat` case, and — now that
/// `path` alone no longer stops re-entry — it is what keeps a wide diamond from
/// expanding combinatorially. Each id is expanded at most once per tree.
fn build(
    project: &Project,
    archive: &ArchiveIndex<'_>,
    id: &str,
    path: &mut Vec<String>,
    expanded: &mut HashSet<String>,
) -> DepNode {
    if path.iter().any(|p| p == id) {
        return DepNode::terminal(id, DepStatus::Cycle);
    }
    let Some((track_id, task)) = find_task(project, id) else {
        // The live tracks do not have it. Before calling it missing, ask the
        // archives — a dep on archived work is satisfied, not broken. This is
        // the only place the archives are read, and only for an id that has
        // already failed everywhere else.
        if let Some(archived) = archive.get(id) {
            return DepNode {
                id: id.to_string(),
                status: DepStatus::Archived,
                track_id: Some(archived.track_id.clone()),
                title: Some(archived.title.clone()),
                state: Some(archived.state),
                tags: Vec::new(),
                deps: Vec::new(),
            };
        }
        // Not recorded as expanded: there is nothing to expand, so a second
        // reference to the same dangling id should report Missing again rather
        // than pointing at a record that does not exist.
        return DepNode::terminal(id, DepStatus::Missing);
    };
    if !expanded.insert(id.to_string()) {
        return DepNode::terminal(id, DepStatus::Repeat);
    }

    path.push(id.to_string());
    let deps = task_deps(task)
        .iter()
        .map(|dep_id| build(project, archive, dep_id, path, expanded))
        .collect();
    path.pop();

    DepNode {
        id: id.to_string(),
        status: DepStatus::Resolved,
        track_id: Some(track_id.to_string()),
        title: Some(task.title.clone()),
        state: Some(task.state),
        tags: task.tags.clone(),
        deps,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ProjectConfig, ProjectInfo};
    use crate::parse::parse_track;
    use std::path::PathBuf;

    fn project(track_md: &str) -> Project {
        Project {
            root: PathBuf::from("/tmp/deps-test"),
            frame_dir: PathBuf::from("/tmp/deps-test/frame"),
            config: ProjectConfig {
                project: ProjectInfo {
                    name: "Test".into(),
                },
                agent: Default::default(),
                tracks: vec![],
                clean: Default::default(),
                ids: Default::default(),
                ui: Default::default(),
                recovery: Default::default(),
                limits: Default::default(),
            },
            tracks: vec![("main".to_string(), parse_track(track_md))],
            inbox: None,
        }
    }

    /// `M-001` → `M-002`, `M-003`; both → `M-004`. `M-005` ↔ `M-006` is a real
    /// cycle. `M-007` points at nothing.
    fn fixture() -> Project {
        project(
            "# Main\n\n## Backlog\n\n\
             - [ ] `M-001` Root\n  - dep: M-002, M-003\n\
             - [ ] `M-002` Left\n  - dep: M-004\n\
             - [ ] `M-003` Right\n  - dep: M-004\n\
             - [ ] `M-004` Shared leaf\n\
             - [ ] `M-005` Cycle a\n  - dep: M-006\n\
             - [ ] `M-006` Cycle b\n  - dep: M-005\n\
             - [ ] `M-007` Dangling\n  - dep: M-999\n\n## Done\n",
        )
    }

    fn kid(node: &DepNode, index: usize) -> &DepNode {
        &node.deps[index]
    }

    #[test]
    fn a_shared_dependency_is_a_repeat_not_a_cycle() {
        let tree = dep_tree(&fixture(), "M-001");
        assert_eq!(tree.status, DepStatus::Resolved);

        // First branch expands M-004 in full.
        let left = kid(&tree, 0);
        assert_eq!(left.id, "M-002");
        assert_eq!(kid(left, 0).id, "M-004");
        assert_eq!(kid(left, 0).status, DepStatus::Resolved);

        // Second branch reaches the same task and says so honestly.
        let right = kid(&tree, 1);
        assert_eq!(right.id, "M-003");
        assert_eq!(kid(right, 0).id, "M-004");
        assert_eq!(
            kid(right, 0).status,
            DepStatus::Repeat,
            "a diamond is not a cycle"
        );
    }

    #[test]
    fn a_real_cycle_stops_at_the_root() {
        // The root used to be left out of the visited set, so a cycle through it
        // ran one lap too far: M-005 → M-006 → M-005 → M-006 (circular).
        let tree = dep_tree(&fixture(), "M-005");
        let b = kid(&tree, 0);
        assert_eq!(b.id, "M-006");
        assert_eq!(b.status, DepStatus::Resolved);

        let back = kid(b, 0);
        assert_eq!(back.id, "M-005");
        assert_eq!(back.status, DepStatus::Cycle);
        assert!(back.deps.is_empty(), "a cycle node expands nothing");
    }

    #[test]
    fn a_dangling_dep_is_missing() {
        let tree = dep_tree(&fixture(), "M-007");
        let dangling = kid(&tree, 0);
        assert_eq!(dangling.id, "M-999");
        assert_eq!(dangling.status, DepStatus::Missing);
        assert_eq!(dangling.title, None);
    }

    /// A dep whose target was archived resolves — as done work, in the file it
    /// went to. The alternative is what `fr check` used to say about every task
    /// `fr clean` archived out from under a dependent: "not found".
    #[test]
    fn a_dep_into_the_archive_is_archived_not_missing() {
        let tmp = tempfile::TempDir::new().unwrap();
        let frame_dir = tmp.path().join("frame");
        std::fs::create_dir_all(frame_dir.join("archive")).unwrap();
        std::fs::write(
            frame_dir.join("archive/main.md"),
            "# Archive \u{2014} main\n\n- [x] `M-900` Finished long ago\n  - resolved: 2025-04-01\n",
        )
        .unwrap();

        let mut p =
            project("# Main\n\n## Backlog\n\n- [ ] `M-001` Root\n  - dep: M-900\n\n## Done\n");
        p.root = tmp.path().to_path_buf();
        p.frame_dir = frame_dir;

        let tree = dep_tree(&p, "M-001");
        let dep = kid(&tree, 0);
        assert_eq!(dep.id, "M-900");
        assert_eq!(dep.status, DepStatus::Archived);
        assert_eq!(dep.title.as_deref(), Some("Finished long ago"));
        assert_eq!(dep.track_id.as_deref(), Some("main"));
        assert!(dep.deps.is_empty(), "an archived node expands nothing");
    }

    /// The archives are read only when the live tracks come up empty, and a
    /// project with no archive directory at all still answers.
    #[test]
    fn an_id_in_neither_place_is_still_missing() {
        let tmp = tempfile::TempDir::new().unwrap();
        let frame_dir = tmp.path().join("frame");
        std::fs::create_dir_all(frame_dir.join("archive")).unwrap();
        std::fs::write(
            frame_dir.join("archive/main.md"),
            "# Archive \u{2014} main\n\n- [x] `M-900` Finished long ago\n  - resolved: 2025-04-01\n",
        )
        .unwrap();

        let mut p = fixture();
        p.root = tmp.path().to_path_buf();
        p.frame_dir = frame_dir;

        let tree = dep_tree(&p, "M-007");
        assert_eq!(kid(&tree, 0).status, DepStatus::Missing);
    }

    #[test]
    fn a_root_with_no_deps_has_no_children() {
        let tree = dep_tree(&fixture(), "M-004");
        assert_eq!(tree.status, DepStatus::Resolved);
        assert!(tree.deps.is_empty());
    }

    #[test]
    fn an_unknown_root_is_missing() {
        assert_eq!(dep_tree(&fixture(), "M-999").status, DepStatus::Missing);
    }

    #[test]
    fn a_self_dependency_is_a_cycle() {
        let p = project("# Main\n\n## Backlog\n\n- [ ] `M-001` Self\n  - dep: M-001\n\n## Done\n");
        let tree = dep_tree(&p, "M-001");
        assert_eq!(tree.status, DepStatus::Resolved);
        assert_eq!(kid(&tree, 0).status, DepStatus::Cycle);
    }
}
