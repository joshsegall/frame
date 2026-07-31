//! Deliberate write failures, for testing multi-file operations.
//!
//! Frame has several operations that are only complete after two or more files
//! are written. Single-file writes are safe by construction —
//! [`crate::io::recovery::atomic_write`] does temp-file + rename, so a crash
//! leaves either the old file or the new one — but a *sequence* has a window in
//! between where the project is half-updated.
//!
//! `fr clean` is the model for handling that: append to the archive first,
//! remove from the track second, so an interruption duplicates rather than
//! loses, and make the duplicate self-healing (`9e183a8`). That ordering is a
//! *claim*, and until now nothing checked it. Reconstructing the half-applied
//! state by hand — which
//! `clean::tests::test_archive_does_not_duplicate_an_already_archived_task`
//! does — verifies that frame can recover from a state you chose, but it cannot
//! tell you whether the ordering leaves a recoverable state in the first place.
//! Only failing a real write partway through a real sequence answers that.
//!
//! # Using it
//!
//! `FRAME_FAIL_WRITE=<substring>` fails the first guarded write whose path
//! contains `<substring>`. `FRAME_FAIL_WRITE=<substring>:N` fails the Nth such
//! write instead.
//!
//! ```text
//! FRAME_FAIL_WRITE=tracks/other.md fr mv SEC-3 --track other
//! ```
//!
//! Selecting by path rather than by a global write count is deliberate. A
//! sequence is surrounded by incidental writes — minting alone updates the ID
//! frontier before a task is touched — so "fail the second write" targets
//! whatever happens to be second, which changes whenever an unrelated write is
//! added. Naming the file targets the step under test and keeps the test
//! readable: `FRAME_FAIL_WRITE=tracks/b.md` says which half of a cross-track
//! move is being cut.
//!
//! # What it does and does not simulate
//!
//! It makes a write **fail**, which is weaker than the process **dying**: the
//! error path the code has still runs, and abrupt death would skip it. That
//! distinction matters — `fr mv --track` logs the target track to the recovery
//! log when the target write returns an error, a mitigation that a kill would
//! bypass entirely.
//!
//! So tests using this assert on **files on disk**, not on the recovery log.
//! That measures the ordering, which is the property that survives either kind
//! of interruption, rather than the mitigation, which only covers one.
//!
//! # Cost
//!
//! Compiled out entirely without `debug_assertions`: [`maybe_fail`] is an
//! `#[inline(always)]` `Ok(())` in release builds, so a shipped binary carries
//! no fault path and reads no environment.

/// Fail this write when the environment selects its path.
///
/// Call at the top of a write entry point, before anything is modified.
#[cfg(debug_assertions)]
pub fn maybe_fail(path: &std::path::Path) -> std::io::Result<()> {
    use std::sync::atomic::{AtomicUsize, Ordering};

    static MATCHES: AtomicUsize = AtomicUsize::new(0);

    // Parsed once per process. Absent or empty disables injection, which is
    // every normal run and every test that does not opt in.
    static TARGET: std::sync::OnceLock<Option<(String, usize)>> = std::sync::OnceLock::new();
    let target = TARGET.get_or_init(|| {
        let raw = std::env::var("FRAME_FAIL_WRITE").ok()?;
        let (substring, nth) = match raw.rsplit_once(':') {
            Some((s, n)) if n.parse::<usize>().is_ok() => (s.to_string(), n.parse().unwrap()),
            _ => (raw, 1),
        };
        if substring.is_empty() || nth == 0 {
            return None;
        }
        Some((substring, nth))
    });
    let Some((substring, nth)) = target else {
        return Ok(());
    };

    if !path.to_string_lossy().contains(substring.as_str()) {
        return Ok(());
    }

    let n = MATCHES.fetch_add(1, Ordering::SeqCst) + 1;
    if n == *nth {
        return Err(std::io::Error::other(format!(
            "injected write failure: {} (match #{n} for {substring:?})",
            path.display()
        )));
    }
    Ok(())
}

#[cfg(not(debug_assertions))]
#[inline(always)]
pub fn maybe_fail(_path: &std::path::Path) -> std::io::Result<()> {
    Ok(())
}
