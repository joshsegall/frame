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
//! write instead, and `FRAME_FAIL_WRITE=<substring>:*` fails **every** one.
//!
//! `:*` is for a *sustained* failure — an unwritable `frame/` rather than a
//! one-off. A retry loop that gives up after one injected failure proves only
//! that the first attempt was made; the interesting behaviour (backoff, an
//! indicator that stays up, work dumped at exit) needs the failure to persist.
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
/// Which matching writes to fail: one particular occurrence, or all of them.
#[cfg(debug_assertions)]
#[derive(Debug, PartialEq, Eq)]
enum Which {
    Nth(usize),
    Every,
}

/// Parse the `FRAME_FAIL_WRITE` value. `None` disables injection.
///
/// Separate from [`maybe_fail`] so it can be tested: the live parse happens once
/// per process behind a `OnceLock`, which a test cannot re-run.
#[cfg(debug_assertions)]
fn parse_target(raw: &str) -> Option<(String, Which)> {
    let (substring, which) = match raw.rsplit_once(':') {
        Some((s, "*")) => (s.to_string(), Which::Every),
        Some((s, n)) if n.parse::<usize>().is_ok() => {
            (s.to_string(), Which::Nth(n.parse().unwrap()))
        }
        _ => (raw.to_string(), Which::Nth(1)),
    };
    if substring.is_empty() || which == Which::Nth(0) {
        return None;
    }
    Some((substring, which))
}

/// Call at the top of a write entry point, before anything is modified.
#[cfg(debug_assertions)]
pub fn maybe_fail(path: &std::path::Path) -> std::io::Result<()> {
    use std::sync::atomic::{AtomicUsize, Ordering};

    static MATCHES: AtomicUsize = AtomicUsize::new(0);

    // Parsed once per process. Absent or empty disables injection, which is
    // every normal run and every test that does not opt in.
    static TARGET: std::sync::OnceLock<Option<(String, Which)>> = std::sync::OnceLock::new();
    let target = TARGET.get_or_init(|| parse_target(&std::env::var("FRAME_FAIL_WRITE").ok()?));
    let Some((substring, which)) = target else {
        return Ok(());
    };

    if !path.to_string_lossy().contains(substring.as_str()) {
        return Ok(());
    }

    let n = MATCHES.fetch_add(1, Ordering::SeqCst) + 1;
    let fail = match which {
        Which::Nth(nth) => n == *nth,
        Which::Every => true,
    };
    if fail {
        return Err(std::io::Error::other(format!(
            "injected write failure: {} (match #{n} for {substring:?})",
            path.display()
        )));
    }
    Ok(())
}

#[cfg(all(test, debug_assertions))]
mod tests {
    use super::*;

    #[test]
    fn a_bare_path_fails_the_first_match() {
        assert_eq!(
            parse_target("tracks/a.md"),
            Some(("tracks/a.md".to_string(), Which::Nth(1)))
        );
    }

    #[test]
    fn a_trailing_number_selects_that_match() {
        assert_eq!(
            parse_target("tracks/a.md:3"),
            Some(("tracks/a.md".to_string(), Which::Nth(3)))
        );
    }

    /// The sustained form: every matching write fails, so a retry loop keeps
    /// failing rather than succeeding on its second go.
    #[test]
    fn a_trailing_star_fails_every_match() {
        assert_eq!(
            parse_target("tracks/a.md:*"),
            Some(("tracks/a.md".to_string(), Which::Every))
        );
    }

    /// A Windows-style path or any other stray colon must not be read as a
    /// count and silently truncate the path being targeted.
    #[test]
    fn a_colon_that_is_not_a_count_stays_part_of_the_path() {
        assert_eq!(
            parse_target("weird:name.md"),
            Some(("weird:name.md".to_string(), Which::Nth(1)))
        );
    }

    #[test]
    fn an_empty_or_zeroth_target_disables_injection() {
        assert_eq!(parse_target(""), None);
        assert_eq!(parse_target("tracks/a.md:0"), None);
        assert_eq!(parse_target(":*"), None);
    }
}

#[cfg(not(debug_assertions))]
#[inline(always)]
pub fn maybe_fail(_path: &std::path::Path) -> std::io::Result<()> {
    Ok(())
}
