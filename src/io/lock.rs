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
        let lock_path = frame_dir.join(".lock");
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

impl Drop for FileLock {
    fn drop(&mut self) {
        // Lock is released automatically when the file is dropped (flock semantics)
        // Optionally clean up the lock file
        let _ = fs::remove_file(&self.path);
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
