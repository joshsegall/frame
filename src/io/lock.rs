use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Advisory file lock for serializing writes to the Frame project.
///
/// Uses platform-native flock (Unix) to coordinate between the TUI
/// and CLI processes.
pub struct FileLock {
    _file: File,
    path: PathBuf,
    /// Whether releasing the lock also unlinks the lock file. Safe for the
    /// project lock, whose file is only ever locked in place; **not** safe for a
    /// lock guarding a file that gets replaced by `rename(2)` — see
    /// [`FileLock::acquire_at`].
    remove_on_drop: bool,
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
        Self::lock_file(frame_dir.join(".lock"), timeout, true)
    }

    /// Acquire an advisory lock on an arbitrary path, leaving the lock file in
    /// place when the lock is released.
    ///
    /// A lock guarding a file that is replaced by `rename(2)` **must** use a
    /// dedicated lock file locked this way. Unlinking it would let a waiter
    /// inherit the lock on an unlinked inode while a newcomer creates a fresh
    /// file and locks that — two writers, one "lock".
    pub fn acquire_at(lock_path: &Path, timeout: Duration) -> Result<Self, LockError> {
        Self::lock_file(lock_path.to_path_buf(), timeout, false)
    }

    fn lock_file(
        lock_path: PathBuf,
        timeout: Duration,
        remove_on_drop: bool,
    ) -> Result<Self, LockError> {
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
                        path: lock_path,
                        remove_on_drop,
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

impl Drop for FileLock {
    fn drop(&mut self) {
        // Lock is released automatically when the file is dropped (flock semantics)
        // Optionally clean up the lock file
        if self.remove_on_drop {
            let _ = fs::remove_file(&self.path);
        }
    }
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
        fs::create_dir_all(&frame_dir).unwrap();

        let lock = FileLock::acquire_default(&frame_dir);
        assert!(lock.is_ok());

        // Lock should be released when dropped
        drop(lock);

        // Should be able to acquire again
        let lock2 = FileLock::acquire_default(&frame_dir);
        assert!(lock2.is_ok());
    }

    #[test]
    fn test_lock_contention() {
        let tmp = TempDir::new().unwrap();
        let frame_dir = tmp.path().join("frame");
        fs::create_dir_all(&frame_dir).unwrap();

        // Acquire first lock
        let _lock1 = FileLock::acquire_default(&frame_dir).unwrap();

        // Second lock should timeout quickly
        let lock2 = FileLock::acquire(&frame_dir, Duration::from_millis(50));
        assert!(lock2.is_err());
    }
}
