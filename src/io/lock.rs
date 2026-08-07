use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Advisory file lock for serializing writes to the Frame project.
///
/// Uses platform-native flock (Unix) to coordinate between the TUI
/// and CLI processes.
pub struct FileLock {
    _file: File,
    _path: PathBuf,
}

/// Error type for lock operations
#[derive(Debug, thiserror::Error)]
pub enum LockError {
    #[error("could not create lock file at {path}: {source}")]
    CreateError {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("could not acquire lock on {path}: another frame process may be writing")]
    Timeout { path: PathBuf },
    #[error("lock error: {0}")]
    IoError(#[from] std::io::Error),
}

impl FileLock {
    /// Acquire an advisory lock on the frame directory.
    /// Blocks up to `timeout` waiting for the lock.
    pub fn acquire(frame_dir: &Path, timeout: Duration) -> Result<Self, LockError> {
        Self::lock_file(frame_dir.join(".lock"), timeout)
    }

    /// Acquire an advisory lock on an arbitrary path.
    ///
    /// Used where the *guarded* file is replaced by `rename(2)` and so cannot
    /// be locked in place — the id frontier and the recovery log.
    pub fn acquire_at(lock_path: &Path, timeout: Duration) -> Result<Self, LockError> {
        Self::lock_file(lock_path.to_path_buf(), timeout)
    }

    /// A lock file is **never unlinked**, and that is not tidiness lost.
    ///
    /// `flock` is on the open file description, not on the path. Removing the
    /// file on release leaves any waiter holding a descriptor on an unlinked
    /// inode — it goes on to win the lock on a file that no longer has a name —
    /// while the next process to ask finds nothing at the path, creates a fresh
    /// file, and locks that. Two writers, one "lock", and precisely when there
    /// is contention to serialize.
    ///
    /// The project lock used to unlink and the other two did not, on the
    /// grounds that only a `rename(2)`-replaced file was exposed. The race
    /// needs no rename: a release with a waiter is enough, which is the
    /// ordinary shape of contention.
    ///
    /// What is left behind is an empty `frame/.lock`. It is in
    /// `LOCAL_ONLY_FRAME_FILES`, covered by the `frame/.*` ignore, and carries
    /// no state — its presence never means "locked", so there is no stale lock
    /// to clear.
    fn lock_file(lock_path: PathBuf, timeout: Duration) -> Result<Self, LockError> {
        let timeout = capped(timeout);
        let file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(&lock_path)
            .map_err(|e| LockError::CreateError {
                path: lock_path.clone(),
                source: e,
            })?;

        let start = Instant::now();
        loop {
            match try_lock(&file) {
                Ok(()) => {
                    return Ok(FileLock {
                        _file: file,
                        _path: lock_path,
                    });
                }
                Err(_) if start.elapsed() < timeout => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(_) => {
                    return Err(LockError::Timeout { path: lock_path });
                }
            }
        }
    }

    /// Acquire with default timeout (5 seconds)
    pub fn acquire_default(frame_dir: &Path) -> Result<Self, LockError> {
        Self::acquire(frame_dir, Duration::from_secs(5))
    }
}

/// The ceiling [`cap_waits`] has put on how long any acquisition may wait.
/// `u64::MAX` — the default — means no ceiling at all.
#[cfg(debug_assertions)]
static WAIT_CAP_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(u64::MAX);

/// Cap what every lock acquisition in this process may wait for. Tests only.
///
/// It exists for one reason: a test that puts a *contended* acquisition inside
/// a generated sequence pays `acquire_default`'s full five seconds for every
/// collision it generates. `tests/concurrency.rs` interleaves a CLI writer's
/// lock window with TUI keystrokes hundreds of times per run, and the whole
/// point is that some of those keystrokes land while the lock is held. At five
/// seconds each that suite would take hours, so it would not exist.
///
/// A cap rather than a fixed value: the retry path asks for zero and must keep
/// getting zero, and a test that wants a shorter wait than the cap still gets
/// it.
///
/// Only the *waiting* changes. Whether the lock is contended, who wins, and
/// what a loser does are untouched — which is what keeps a suite using this
/// honest about the behaviour it is testing. Compiled out in release builds,
/// like [`crate::io::fault`].
///
/// Process-global and never restored, deliberately: a per-call parameter would
/// have to be threaded through every write path in the TUI, which is the code
/// under test.
#[cfg(debug_assertions)]
pub fn cap_waits(limit: Duration) {
    WAIT_CAP_MS.store(
        limit.as_millis().min(u64::MAX as u128) as u64,
        std::sync::atomic::Ordering::Relaxed,
    );
}

#[cfg(debug_assertions)]
fn capped(timeout: Duration) -> Duration {
    let cap = WAIT_CAP_MS.load(std::sync::atomic::Ordering::Relaxed);
    if cap == u64::MAX {
        return timeout;
    }
    timeout.min(Duration::from_millis(cap))
}

#[cfg(not(debug_assertions))]
#[inline(always)]
fn capped(timeout: Duration) -> Duration {
    timeout
}

/// Try to acquire an exclusive flock on the file (non-blocking)
#[cfg(unix)]
fn try_lock(file: &File) -> Result<(), std::io::Error> {
    use std::os::unix::io::AsRawFd;
    let fd = file.as_raw_fd();
    let result = unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) };
    if result == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(not(unix))]
fn try_lock(_file: &File) -> Result<(), std::io::Error> {
    // On non-Unix platforms, just succeed (advisory locking)
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_acquire_and_release_lock() {
        let tmp = TempDir::new().unwrap();
        let frame_dir = tmp.path().join("frame");
        std::fs::create_dir_all(&frame_dir).unwrap();

        let lock = FileLock::acquire_default(&frame_dir);
        assert!(lock.is_ok());

        // Lock should be released when dropped
        drop(lock);

        // Should be able to acquire again
        let lock2 = FileLock::acquire_default(&frame_dir);
        assert!(lock2.is_ok());
    }

    /// The unlink race, as a case. A lock file that is removed on release
    /// leaves a waiter holding a descriptor on an unlinked inode, which it goes
    /// on to lock, while the next process finds nothing at the path and locks a
    /// fresh file it creates — two writers, one "lock", exactly when there is
    /// contention to serialize. Needs no rename of the guarded file: a release
    /// with a waiter is enough.
    #[test]
    fn releasing_the_lock_does_not_let_two_writers_hold_it() {
        let tmp = TempDir::new().unwrap();
        let frame_dir = tmp.path().join("frame");
        std::fs::create_dir_all(&frame_dir).unwrap();

        // A holds it. B opens the same file and starts waiting on it.
        let a = FileLock::acquire_default(&frame_dir).unwrap();
        let dir = frame_dir.clone();
        let waiter = std::thread::spawn(move || FileLock::acquire(&dir, Duration::from_secs(5)));
        std::thread::sleep(Duration::from_millis(100));

        // A releases. If that unlinked the file, B is now holding a lock on an
        // inode with no name.
        drop(a);
        let b = waiter.join().unwrap().expect("the waiter gets the lock");

        // C asks for the lock while B holds it. It must not get it.
        let c = FileLock::acquire(&frame_dir, Duration::from_millis(50));
        assert!(
            c.is_err(),
            "two writers hold the project lock at once: the waiter inherited an \
             unlinked inode and the newcomer created and locked a fresh file"
        );
        drop(b);
    }

    #[test]
    fn test_lock_contention() {
        let tmp = TempDir::new().unwrap();
        let frame_dir = tmp.path().join("frame");
        std::fs::create_dir_all(&frame_dir).unwrap();

        // Acquire first lock
        let _lock1 = FileLock::acquire_default(&frame_dir).unwrap();

        // Second lock should timeout quickly
        let lock2 = FileLock::acquire(&frame_dir, Duration::from_millis(50));
        assert!(lock2.is_err());
    }
}
