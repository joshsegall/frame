//! The one chokepoint for minting **top-level task ID numbers**.
//!
//! Every path that mints a new top-level number goes through [`Mint`] — task add,
//! inbox triage, import, cross-track move, promote-to-top-level, and `fr clean`'s
//! ID assignment and duplicate resolution. Anything that computes `max + 1` on
//! its own would silently reintroduce the collisions this exists to prevent.
//!
//! A number comes from `max(floor, recorded) + 1`, where:
//!
//! - the **floor** is what this working copy can see — the live track plus its
//!   archives, so archiving or deleting a task never hands its number back out;
//! - the **record** is the durable, clone-shared frontier in [`crate::io::ids`],
//!   which is what stops two git worktrees of one clone from minting the same ID.
//!
//! Two allocators deliberately stay outside this:
//!
//! - **child IDs** (`BAC-153.2`), numbered per parent by
//!   [`crate::ops::task_ops::next_child_number`] — two worktrees adding a subtask
//!   to the *same* parent still collide. Detected and repaired rather than
//!   prevented; see below.
//! - **`fr actor merge`** ([`crate::ops::actor_merge`]), which renumbers a whole
//!   namespace in bulk from its own scan of every ID in the project. Its target
//!   namespace belongs to a different clone, whose frontier is a different file,
//!   so this clone's record has nothing useful to say about it.
//!
//! # Why child IDs are not covered
//!
//! A frontier for child numbers would have to be keyed *per parent*, so the
//! store would grow with the number of tasks — against the bound stated in
//! [`crate::io::ids`], and each entry is dead the moment the other worktree
//! merges the task in. Numbering children from the top-level counter instead
//! (`BAC-153.207`) keeps the store flat but gives up the `.1 .2 .3` reading of a
//! file people hand-edit.
//!
//! Neither cost is worth paying, because the two collisions are not equally
//! serious. A reissued *top-level* number cannot be repaired by machine —
//! renumbering a live task is `IdReissuedAfterArchive`, which `fr check`
//! deliberately reports without a fix, since which of two legitimate holders
//! should move is a judgment call. A child number means nothing outside its
//! parent: there is one right answer, `fr clean` applies it, and the deps follow.
//!
//! So the collision stays possible and is handled after the fact:
//!
//! - `fr check` reports it as a `DuplicateId` error (the duplicate scan has
//!   always recursed into subtasks);
//! - `fr clean` resolves it by renumbering the later copy **under its own
//!   parent** — the allocator split in [`crate::ops::clean`];
//! - `ChildIdNotUnderParent` catches the case where that went wrong anyway, with
//!   a repair behind `fr check --fix`.
//!
//! Pinned end to end by `test_colliding_subtask_ids_are_detected_and_repaired`.

use std::path::Path;

use crate::model::task_id::Token;
use crate::model::track::Track;
use crate::ops::task_ops::{find_max_id_in_tasks, find_max_id_in_track};

/// The project files a mint consults beyond the in-memory track.
#[derive(Debug, Clone, Copy)]
struct OnDisk<'a> {
    frame_dir: &'a Path,
    /// Which track's archives count toward the floor.
    track_id: &'a str,
}

/// Everything a top-level mint needs: the namespace to mint in, and where the
/// durable frontier lives.
#[derive(Debug, Clone, Copy)]
pub struct Mint<'a> {
    on_disk: Option<OnDisk<'a>>,
    prefix: &'a str,
    token: Option<&'a Token>,
}

impl<'a> Mint<'a> {
    /// A mint backed by the project on disk: the clone-shared frontier store and
    /// `track_id`'s archives. This is the form every production path uses.
    pub fn new(
        frame_dir: &'a Path,
        track_id: &'a str,
        prefix: &'a str,
        token: Option<&'a Token>,
    ) -> Self {
        Mint {
            on_disk: Some(OnDisk {
                frame_dir,
                track_id,
            }),
            prefix,
            token,
        }
    }

    /// A mint with **no durable frontier**: numbers come from the in-memory scan
    /// alone, exactly as frame behaved before the store existed. For callers
    /// holding a bare `Track` with no project on disk — tests, and merge
    /// simulations that deliberately model divergent working copies.
    pub fn scan_only(prefix: &'a str, token: Option<&'a Token>) -> Self {
        Mint {
            on_disk: None,
            prefix,
            token,
        }
    }

    pub fn prefix(&self) -> &'a str {
        self.prefix
    }

    pub fn token(&self) -> Option<&'a Token> {
        self.token
    }

    /// Reserve the next number for this namespace.
    pub fn next(&self, track: &Track) -> u32 {
        self.next_n(track, 1)
    }

    /// Reserve `n` consecutive numbers and return the first. One store write for
    /// the whole block, so batch paths (`fr import`, `fr clean`'s ID assignment)
    /// stay a single reservation.
    pub fn next_n(&self, track: &Track, n: u32) -> u32 {
        self.next_n_above(track, 0, n)
    }

    /// Reserve the next number above both the visible floor and `extra_floor` —
    /// for a batch that has already handed out numbers which are not yet present
    /// in `track` (duplicate-ID resolution stages its reassignments).
    pub fn next_above(&self, track: &Track, extra_floor: u32) -> u32 {
        self.next_n_above(track, extra_floor, 1)
    }

    fn next_n_above(&self, track: &Track, extra_floor: u32, n: u32) -> u32 {
        let floor = self.floor(track).max(extra_floor);
        match self.on_disk {
            Some(at) => crate::io::ids::reserve(at.frame_dir, self.prefix, self.token, floor, n),
            None => floor + 1,
        }
    }

    /// The highest number visible to this working copy: the live track, plus the
    /// track's done-task archive and its archived whole-track file. Archives
    /// count so that `fr clean` archiving a task — or `fr track archive` taking a
    /// whole track out of `tracks/` — can never hand its number back out.
    fn floor(&self, track: &Track) -> u32 {
        let prefix_dash = format!("{}-", self.prefix);
        let mut max = 0usize;
        find_max_id_in_track(track, &prefix_dash, self.token, &mut max);
        if let Some(at) = self.on_disk {
            let archive = at.frame_dir.join("archive");
            let file = format!("{}.md", at.track_id);
            archived_tasks_max(&archive.join(&file), &prefix_dash, self.token, &mut max);
            archived_track_max(
                &archive.join("_tracks").join(&file),
                &prefix_dash,
                self.token,
                &mut max,
            );
        }
        max as u32
    }
}

/// Scan `frame/archive/<track>.md` — a bare task list under an `# Archive` header
/// — raising `max`. A missing or unreadable file contributes nothing.
fn archived_tasks_max(path: &Path, prefix_dash: &str, token: Option<&Token>, max: &mut usize) {
    let Ok(content) = std::fs::read_to_string(path) else {
        return;
    };
    let archive = crate::parse::parse_archive(&content);
    find_max_id_in_tasks(&archive.tasks, prefix_dash, token, max);
}

/// Scan `frame/archive/_tracks/<track>.md` — a whole archived track file, so it
/// parses as a track — raising `max`.
fn archived_track_max(path: &Path, prefix_dash: &str, token: Option<&Token>, max: &mut usize) {
    let Ok(content) = std::fs::read_to_string(path) else {
        return;
    };
    let track = crate::parse::parse_track(&content);
    find_max_id_in_track(&track, prefix_dash, token, max);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn track_with(ids: &[&str]) -> Track {
        let mut text = String::from("# Demo\n\n## Backlog\n\n");
        for id in ids {
            text.push_str(&format!("- [ ] `{}` task\n", id));
        }
        text.push_str("\n## Done\n");
        crate::parse::parse_track(&text)
    }

    fn frame_dir(tmp: &TempDir) -> std::path::PathBuf {
        let dir = tmp.path().join("frame");
        fs::create_dir_all(dir.join("archive").join("_tracks")).unwrap();
        dir
    }

    #[test]
    fn scan_only_mints_above_the_live_maximum() {
        let track = track_with(&["DEM-001", "DEM-004"]);
        assert_eq!(Mint::scan_only("DEM", None).next(&track), 5);
    }

    #[test]
    fn archived_tasks_hold_their_numbers() {
        let tmp = TempDir::new().unwrap();
        let frame = frame_dir(&tmp);
        fs::write(
            frame.join("archive").join("demo.md"),
            "# Archive — demo\n\n- [x] `DEM-003` task 3\n  - resolved: 2026-07-29\n",
        )
        .unwrap();

        // The live track has nothing left, but DEM-003 is spoken for.
        let empty = track_with(&[]);
        let mint = Mint::new(&frame, "demo", "DEM", None);
        assert_eq!(mint.next(&empty), 4);
    }

    #[test]
    fn an_archived_whole_track_holds_its_numbers() {
        let tmp = TempDir::new().unwrap();
        let frame = frame_dir(&tmp);
        fs::write(
            frame.join("archive").join("_tracks").join("demo.md"),
            "# Demo\n\n## Backlog\n\n- [ ] `DEM-012` task\n\n## Done\n",
        )
        .unwrap();

        let empty = track_with(&[]);
        assert_eq!(Mint::new(&frame, "demo", "DEM", None).next(&empty), 13);
    }

    #[test]
    fn a_mint_is_durable_once_the_track_no_longer_shows_it() {
        let tmp = TempDir::new().unwrap();
        let frame = frame_dir(&tmp);
        let mint = Mint::new(&frame, "demo", "DEM", None);

        let track = track_with(&["DEM-001"]);
        assert_eq!(mint.next(&track), 2);
        // The caller crashed before writing DEM-002, so the track still shows
        // only DEM-001 — but 2 was handed out and is not offered again.
        assert_eq!(mint.next(&track), 3);
    }

    #[test]
    fn a_batch_reserves_a_contiguous_block() {
        let tmp = TempDir::new().unwrap();
        let frame = frame_dir(&tmp);
        let mint = Mint::new(&frame, "demo", "DEM", None);
        let track = track_with(&["DEM-002"]);

        assert_eq!(mint.next_n(&track, 4), 3); // 3,4,5,6
        assert_eq!(mint.next(&track), 7);
    }

    #[test]
    fn an_extra_floor_covers_numbers_not_yet_in_the_track() {
        let track = track_with(&["DEM-002"]);
        let mint = Mint::scan_only("DEM", None);
        assert_eq!(mint.next_above(&track, 9), 10);
    }

    #[test]
    fn other_namespaces_are_invisible() {
        let tmp = TempDir::new().unwrap();
        let frame = frame_dir(&tmp);
        let track = track_with(&["DEM-b040", "DEM-002"]);
        let token = Token::new("b").unwrap();

        assert_eq!(Mint::new(&frame, "demo", "DEM", None).next(&track), 3);
        assert_eq!(
            Mint::new(&frame, "demo", "DEM", Some(&token)).next(&track),
            41
        );
    }
}
