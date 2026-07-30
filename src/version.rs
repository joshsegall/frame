//! Build identity — the crate version plus the commit it was built from.
//!
//! [`LONG`] is what `fr --version` and `fr info` print. Everything else (the
//! `--help` banner, the JSON `version` field) stays on the bare crate version, so
//! nothing that parses a version string has to cope with a suffix.
//!
//! `build.rs` resolves the commit at compile time and leaves it unset when the
//! build didn't come from a git checkout, in which case [`LONG`] is just
//! [`VERSION`].

/// The crate version alone (`0.1.6`) — the form to expose to machines.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The short commit this binary was built from, or `None` when it wasn't built
/// from a git checkout.
pub const COMMIT: Option<&str> = option_env!("FRAME_GIT_HASH");

/// Version with the build's commit when there is one (`0.1.6 (ad763b0)`),
/// otherwise just the version.
pub const LONG: &str = env!("FRAME_VERSION_LONG");
