//! What a `ref:` or `spec:` value points at, and whether it is there.
//!
//! The two metadata keys mean different things — a spec is the document a task
//! implements, a ref is a file it touches — but they are written identically and
//! carry the same kind of value: a path relative to the project root, optionally
//! followed by a `#anchor`, a `:line` or `:line:col`. So they resolve
//! identically, and this module is the one place that says how.
//!
//! It exists because they did not. `spec:` stripped the anchor before looking on
//! disk and `ref:` did not, so `doc/design.md#rationale` was a valid spec and a
//! broken ref — the same string, the same file, two answers, in two copies of
//! the rule that had drifted apart in `check` and in `clean`. Neither knew about
//! `src/parser.rs:807` at all, which is how most refs to code get written.
//!
//! **Only the file is validated.** An anchor may name a heading that moved and a
//! line number goes stale on the next edit above it; neither is a broken
//! reference in the sense worth reporting, and frame does not read the target to
//! find out.

use std::path::Path;

/// Every reading of a `ref:`/`spec:` value, most literal first.
///
/// The whole value comes first so a filename that genuinely contains `#` or `:`
/// is reachable — the suffixes are stripped only when the literal path is not
/// there, which makes this strictly more permissive than checking either form
/// alone.
fn candidates(value: &str) -> Vec<&str> {
    let mut out = vec![value];
    let anchorless = strip_anchor(value);
    let lineless = strip_line_ref(value);
    for c in [anchorless, lineless, strip_line_ref(anchorless)] {
        if !c.is_empty() && !out.contains(&c) {
            out.push(c);
        }
    }
    out
}

/// The path part of a value carrying an anchor: everything before the first `#`.
pub fn strip_anchor(value: &str) -> &str {
    value.split('#').next().unwrap_or(value)
}

/// The path part of a value carrying a line reference: `:N`, `:N-M`, or a
/// second such segment for a column (`:N:C`).
///
/// Bounded to two strippings so a path of the shape `a:1:2:3` cannot be eaten
/// down to `a` — beyond a line and a column, the colon is part of the name.
pub fn strip_line_ref(value: &str) -> &str {
    let mut out = value;
    for _ in 0..2 {
        match out.rsplit_once(':') {
            Some((head, tail)) if !head.is_empty() && is_line_segment(tail) => out = head,
            _ => break,
        }
    }
    out
}

/// `807` or `807-820` — a line, or a range of them.
fn is_line_segment(s: &str) -> bool {
    let (start, end) = match s.split_once('-') {
        Some((a, b)) => (a, Some(b)),
        None => (s, None),
    };
    let digits = |x: &str| !x.is_empty() && x.bytes().all(|b| b.is_ascii_digit());
    digits(start) && end.is_none_or(digits)
}

/// Whether a `ref:`/`spec:` value resolves to a file in the project.
pub fn exists(project_root: &Path, value: &str) -> bool {
    let value = value.trim();
    if value.is_empty() {
        return false;
    }
    candidates(value)
        .into_iter()
        .any(|c| project_root.join(c).exists())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn project() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("doc")).unwrap();
        fs::create_dir_all(dir.path().join("src")).unwrap();
        fs::write(dir.path().join("doc/design.md"), "x").unwrap();
        fs::write(dir.path().join("doc/issue#3.md"), "x").unwrap();
        fs::write(dir.path().join("src/parser.rs"), "x").unwrap();
        fs::write(dir.path().join("src/odd:9.rs"), "x").unwrap();
        dir
    }

    #[test]
    fn a_plain_path_resolves() {
        let dir = project();
        assert!(exists(dir.path(), "doc/design.md"));
        assert!(!exists(dir.path(), "doc/missing.md"));
    }

    #[test]
    fn an_anchor_is_ignored() {
        let dir = project();
        assert!(exists(dir.path(), "doc/design.md#rationale"));
        assert!(!exists(dir.path(), "doc/missing.md#rationale"));
    }

    #[test]
    fn a_line_reference_is_ignored() {
        let dir = project();
        assert!(exists(dir.path(), "src/parser.rs:807"));
        assert!(exists(dir.path(), "src/parser.rs:807-820"));
        assert!(exists(dir.path(), "src/parser.rs:807:12"));
        assert!(!exists(dir.path(), "src/missing.rs:807"));
        assert!(!exists(dir.path(), "src/missing.rs:807-820"));
    }

    /// The literal value wins, so a `#` or a `:` in a filename is not mistaken
    /// for a suffix.
    #[test]
    fn a_hash_or_colon_in_the_filename_still_resolves() {
        let dir = project();
        assert!(exists(dir.path(), "doc/issue#3.md"));
        assert!(exists(dir.path(), "src/odd:9.rs"));
    }

    #[test]
    fn an_empty_or_suffix_only_value_resolves_to_nothing() {
        let dir = project();
        assert!(!exists(dir.path(), ""));
        assert!(!exists(dir.path(), "   "));
        assert!(!exists(dir.path(), "#anchor"));
        assert!(!exists(dir.path(), ":807"));
    }

    #[test]
    fn a_colon_run_is_not_eaten_past_a_column() {
        assert_eq!(strip_line_ref("a:1:2:3"), "a:1");
        assert_eq!(strip_line_ref("src/parser.rs"), "src/parser.rs");
        assert_eq!(strip_line_ref("src/parser.rs:807"), "src/parser.rs");
        assert_eq!(strip_line_ref("src/parser.rs:807-820"), "src/parser.rs");
    }

    /// Only a line, a range, or a column follows the colon. Anything else is
    /// part of the path — a stray suffix must not make a missing file look
    /// present by resolving to its parent directory.
    #[test]
    fn a_non_numeric_suffix_is_part_of_the_path() {
        assert_eq!(strip_line_ref("src/parser.rs:main"), "src/parser.rs:main");
        assert_eq!(strip_line_ref("src/parser.rs:8a"), "src/parser.rs:8a");
        assert_eq!(strip_line_ref("src/parser.rs:-8"), "src/parser.rs:-8");
        assert_eq!(strip_line_ref("src/parser.rs:8-"), "src/parser.rs:8-");
    }
}
