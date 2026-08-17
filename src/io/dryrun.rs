//! The write barrier behind `--dry-run`.
//!
//! A preview flag can be built two ways. The first is to teach every handler to
//! skip its own saves — which is what frame did, on six commands, each with its
//! own `if !args.dry_run`. That is how `fr clean --dry-run` came to advance the
//! durable ID frontier while its documentation said it wrote nothing: the guard
//! covered the archive append, and the mint underneath it was never in view.
//!
//! The second is to arm a barrier once and let the handler run unchanged. Frame's
//! writes funnel through a small, enumerable set of primitives — very nearly the
//! same set [`crate::io::fault`] guards for crash injection — so the barrier has
//! few places to sit, and a write path nobody thought about is covered by the same
//! stroke. That is what this is.
//!
//! # Using it
//!
//! A handler calls [`arm`] once, as its first statement, and then does not think
//! about it again:
//!
//! ```ignore
//! fn cmd_mv(args: MvArgs, json: bool) -> Result<(), Box<dyn std::error::Error>> {
//!     dryrun::arm(args.dry_run);
//!     ...
//! }
//! ```
//!
//! Every write entry point asks [`blocked`] before it touches the filesystem, and
//! returns success without doing so when the answer is yes. The path is recorded
//! on the way past, so [`would_write`] can hand the handler the list of files the
//! run would have changed — which is what makes the preview worth printing.
//!
//! Process-global rather than threaded through, for the same reason
//! [`crate::io::fault`] is: the alternative is a mode parameter on every function
//! between the handler and the write, most of which have no opinion about it, and
//! one missed thread is a write that escapes. `fr` runs one command per process.
//!
//! # What it does not cover
//!
//! **`frame/.lock`.** A dry run still takes the project lock, so a preview is
//! computed against a project no other process is halfway through writing. The
//! lock file is opened `O_CREAT`, so a dry run against a project that has never
//! been locked creates it. It is in
//! [`crate::io::project_io::LOCAL_ONLY_FRAME_FILES`], gitignored, and says nothing
//! about project content — but it is the one file "wrote nothing" does not mean.
//!
//! **The TUI.** `.state.json` and `.rescue/` are written by a surface that has no
//! dry run to be in, and arming the barrier is a CLI handler's job.

use std::cell::{Cell, RefCell};
use std::io;
use std::path::{Path, PathBuf};

thread_local! {
    static ACTIVE: Cell<bool> = const { Cell::new(false) };
    static WOULD_WRITE: RefCell<Vec<PathBuf>> = const { RefCell::new(Vec::new()) };
}

/// Arm or disarm the barrier. Called once by a handler, before anything else.
pub fn arm(on: bool) {
    ACTIVE.with(|a| a.set(on));
    if on {
        record_reset();
    }
}

/// Whether writes are currently being suppressed.
///
/// **Thread-local, not process-global**, for two reasons pointing the same way.
/// `fr` runs one command on the main thread — no write in this crate happens on a
/// spawned one — so the two are equivalent in the binary. And in the test harness
/// they are not: `cargo test` runs unit tests in parallel threads of one process,
/// where a global would let this test arm the barrier and silently swallow the
/// writes of every test running beside it.
pub fn is_active() -> bool {
    ACTIVE.with(|a| a.get())
}

/// Ask before writing: `true` means don't, and the path has been recorded.
///
/// Call at the top of a write entry point, before anything is modified — the same
/// contract as [`crate::io::fault::maybe_fail`], and usually the line after it.
///
/// For a write that knows the bytes it was about to lay down, prefer
/// [`blocked_with`]: it records only a file whose content would actually differ.
pub fn blocked(path: &Path) -> bool {
    if !is_active() {
        return false;
    }
    record(path);
    true
}

/// [`blocked`], for a write whose content is known.
///
/// Records `path` only when `content` differs from what is on disk. Frame has
/// write paths that lay a file down whether or not it changed — `fr clean` saves
/// every track, on the grounds that a task it did not touch serializes verbatim —
/// and a preview that listed all of them would be reporting the shape of the save
/// loop rather than the effect of the command. "Would change" means the bytes
/// differ.
pub fn blocked_with(path: &Path, content: &[u8]) -> bool {
    if !is_active() {
        return false;
    }
    // An unreadable or absent file is a change: the write would create it.
    match std::fs::read(path) {
        Ok(existing) if existing == content => {}
        _ => record(path),
    }
    true
}

/// The files this run would have written, in the order they were first reached.
///
/// Drains the record, so a handler that prints them does not print them twice.
pub fn would_write() -> Vec<PathBuf> {
    WOULD_WRITE.with(|w| std::mem::take(&mut *w.borrow_mut()))
}

/// Record `path` once. A single command writes the same file more than once —
/// `fr clean` rewrites a track it has already archived out of — and a preview
/// naming it twice reads as two changes.
fn record(path: &Path) {
    WOULD_WRITE.with(|w| {
        let mut recorded = w.borrow_mut();
        if !recorded.iter().any(|p| p == path) {
            recorded.push(path.to_path_buf());
        }
    });
}

fn record_reset() {
    WOULD_WRITE.with(|w| w.borrow_mut().clear());
}

// ---------------------------------------------------------------------------
// Guarded `std::fs` wrappers
// ---------------------------------------------------------------------------
//
// The write sites that do not go through `crate::io::recovery::atomic_write` —
// file moves, marker files, directory creation. Call these rather than `std::fs`
// directly so the barrier cannot be forgotten at a new one.

/// [`std::fs::write`], suppressed under a dry run.
pub fn write(path: &Path, contents: impl AsRef<[u8]>) -> io::Result<()> {
    if blocked_with(path, contents.as_ref()) {
        return Ok(());
    }
    std::fs::write(path, contents)
}

/// [`std::fs::rename`], suppressed under a dry run.
///
/// Both ends are recorded: a move changes the directory at the source as much as
/// at the destination, and a preview that named only one would be describing half
/// the operation.
pub fn rename(from: &Path, to: &Path) -> io::Result<()> {
    if is_active() {
        record(from);
        record(to);
        return Ok(());
    }
    std::fs::rename(from, to)
}

/// [`std::fs::remove_file`], suppressed under a dry run.
pub fn remove_file(path: &Path) -> io::Result<()> {
    if blocked(path) {
        return Ok(());
    }
    std::fs::remove_file(path)
}

/// [`std::fs::create_dir_all`], suppressed under a dry run.
///
/// Not recorded: a directory is scaffolding for the write that follows, and the
/// write records itself. Listing `frame/archive/` alongside
/// `frame/archive/main.md` tells a reader nothing they did not already have.
pub fn create_dir_all(path: &Path) -> io::Result<()> {
    if is_active() {
        return Ok(());
    }
    std::fs::create_dir_all(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// One test rather than several, because arming is a mode: the interesting
    /// assertions are about what happens on either side of it, in order. Safe to
    /// run beside every other test because the flag is thread-local.
    #[test]
    fn the_barrier_suppresses_and_records() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("a.md");

        // Disarmed: writes land, nothing is recorded.
        arm(false);
        write(&path, b"real").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "real");
        assert!(would_write().is_empty());

        // Armed: writes are suppressed and recorded.
        arm(true);
        write(&path, b"preview").unwrap();
        remove_file(&path).unwrap();
        create_dir_all(&tmp.path().join("sub")).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "real");
        assert!(!tmp.path().join("sub").exists());

        // A write laying down bytes the file already holds is not a change.
        write(&path, b"real").unwrap();

        // Recorded once each, and a directory is not listed.
        let seen = would_write();
        assert_eq!(seen, vec![path.clone()]);
        // Draining leaves the record empty.
        assert!(would_write().is_empty());

        // A rename records both ends and moves nothing.
        let to = tmp.path().join("b.md");
        rename(&path, &to).unwrap();
        assert!(path.exists() && !to.exists());
        assert_eq!(would_write(), vec![path, to]);

        // Arming again clears whatever the last run left behind.
        arm(true);
        assert!(would_write().is_empty());
        arm(false);
    }
}
