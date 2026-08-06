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
//!
//! It also says how a value is **spelled** ([`normalize`]). One file has many
//! spellings — `real.md`, `./real.md`, `sub/../real.md` — and every one of them
//! resolves, so a stored list could carry the same file twice and `rm` could
//! fail to find what was plainly there. Storing the normal form and comparing by
//! it is what makes the list behave like a set of files rather than a set of
//! strings.

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

/// The value with `.` and `..` segments folded away, so that every spelling of
/// one file gives one string.
///
/// **Lexical, and over the whole value.** Nothing is read from disk, so a
/// symlink cannot change the answer and a path that does not exist normalizes
/// just as well as one that does. Folding runs over `/`-separated segments of
/// the entire value rather than a stripped path part, which is what keeps the
/// suffix safe: neither a `#anchor` nor a `:line` can contain a `/`, so
/// `./sub/../real.md:807` folds to `real.md:807` while `doc/issue#3.md` and
/// `src/odd:9.rs` are left exactly as they are. Doing it the other way round
/// would mean choosing a candidate first — and `candidates` tries the literal
/// value ahead of the stripped ones precisely because that choice cannot be made
/// reliably.
///
/// Two things survive folding on purpose: a leading `/`, so the value stays
/// absolute, and a leading `..` with nothing to pop, so it still escapes.
/// Whether either is *allowed* is a separate question from how it is spelled.
pub fn normalize(value: &str) -> String {
    let value = value.trim();
    // An empty value stays empty. Folding it to `.` would name the project root,
    // which exists — turning a value `exists` refuses into one it accepts.
    if value.is_empty() {
        return String::new();
    }
    let absolute = value.starts_with('/');
    let mut out: Vec<&str> = Vec::new();
    for segment in value.split('/') {
        match segment {
            // `.` and the empty segment from `//` (or a trailing slash) carry no
            // information; an absolute path's leading empty segment is already
            // recorded in `absolute`.
            "." | "" => {}
            ".." => match out.last() {
                // Nothing to cancel: at the root of an absolute path `..` is
                // itself, and a leading `..` is kept so containment can see it.
                None | Some(&"..") => out.push(segment),
                Some(_) => {
                    out.pop();
                }
            },
            _ => out.push(segment),
        }
    }
    let joined = out.join("/");
    match (absolute, joined.is_empty()) {
        (true, _) => format!("/{joined}"),
        // Everything folded away — `./` or `sub/..` — which names the project
        // root itself.
        (false, true) => ".".to_string(),
        (false, false) => joined,
    }
}

/// Why a value cannot be stored, beyond the file not being there.
///
/// Each of these resolves perfectly well *here* — that is what makes them worth
/// refusing rather than reporting broken. What they do not do is survive the
/// trip to another clone, where the project lives at a different absolute path
/// and whatever sits outside it is not the same.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathRejection {
    /// An absolute path: `/etc/hosts`, or even the project's own files spelled
    /// from the filesystem root.
    Absolute,
    /// Escapes the project root after folding: `../outside.md`.
    Escapes,
}

impl PathRejection {
    /// The clause a message puts after the path, in the shape "<path> …".
    pub fn reason(self) -> &'static str {
        match self {
            PathRejection::Absolute => {
                "is absolute — a ref is relative to the project root, so this one \
                 means nothing on another machine"
            }
            PathRejection::Escapes => {
                "leaves the project root — nothing outside it travels with the project"
            }
        }
    }
}

/// Whether a value names something the project can actually point at, or `None`
/// when it is fine.
///
/// Pure: no disk, no git, no `project_root` argument. A path that escapes does
/// so by its own spelling, and asking the filesystem could only weaken the
/// answer — `../outside.md` resolving to something real is the problem, not a
/// mitigation.
pub fn containment(value: &str) -> Option<PathRejection> {
    let normalized = normalize(value);
    if normalized.is_empty() {
        return None; // Not a containment question; `exists` refuses it already.
    }
    if Path::new(&normalized).is_absolute() {
        return Some(PathRejection::Absolute);
    }
    if normalized == ".." || normalized.starts_with("../") {
        return Some(PathRejection::Escapes);
    }
    None
}

/// Whether two values name the same file, whatever spelling each uses.
///
/// The suffix is part of the identity: `real.md` and `real.md:807` are different
/// references, and removing one must not take the other with it.
pub fn same_path(a: &str, b: &str) -> bool {
    normalize(a) == normalize(b)
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

    #[test]
    fn normalize_folds_dot_and_dotdot() {
        assert_eq!(normalize("./sub/../real.md"), "real.md");
        assert_eq!(normalize("doc/../src/parser.rs"), "src/parser.rs");
        assert_eq!(normalize("./real.md"), "real.md");
        assert_eq!(normalize("a//b"), "a/b");
        assert_eq!(normalize("doc/"), "doc");
        assert_eq!(normalize("  real.md  "), "real.md");
        // Already normal: folding is idempotent, which is what lets a stored
        // value be compared against a fresh one without re-deriving either.
        assert_eq!(normalize("src/parser.rs"), "src/parser.rs");
        assert_eq!(normalize(normalize("./sub/../real.md").as_str()), "real.md");
    }

    /// The suffix rides along untouched — folding whole `/`-separated segments
    /// cannot see inside one, which is the entire reason it is done that way.
    #[test]
    fn normalize_leaves_the_suffix_alone() {
        assert_eq!(normalize("./sub/../real.md:807"), "real.md:807");
        assert_eq!(normalize("./doc/../design.md#why"), "design.md#why");
        assert_eq!(normalize("doc/issue#3.md"), "doc/issue#3.md");
        assert_eq!(normalize("src/odd:9.rs"), "src/odd:9.rs");
        assert_eq!(normalize("src/parser.rs:807-820"), "src/parser.rs:807-820");
    }

    /// Absoluteness and an unpoppable `..` are the two things a later
    /// containment check needs to still be able to see.
    #[test]
    fn normalize_keeps_what_makes_a_path_escape() {
        assert_eq!(normalize("../outside.md"), "../outside.md");
        assert_eq!(normalize("a/../../b.md"), "../b.md");
        assert_eq!(normalize("../../b.md"), "../../b.md");
        assert_eq!(normalize("/etc/hosts"), "/etc/hosts");
        assert_eq!(normalize("/etc/../etc/hosts"), "/etc/hosts");
        // `..` above the root of an absolute path has nowhere to go.
        assert_eq!(normalize("/../etc/hosts"), "/../etc/hosts");
    }

    /// A value that folds to the project root becomes `.` — but an *empty* one
    /// stays empty, because `.` exists and empty does not, and normalization
    /// must never turn a value `exists` refuses into one it accepts.
    #[test]
    fn normalize_handles_values_that_fold_to_nothing() {
        assert_eq!(normalize("."), ".");
        assert_eq!(normalize("./"), ".");
        assert_eq!(normalize("sub/.."), ".");
        assert_eq!(normalize(""), "");
        assert_eq!(normalize("   "), "");
        assert!(!exists(project().path(), &normalize("")));
    }

    /// A file with a name of its own that happens to contain `..` is not a
    /// traversal — only a whole segment counts.
    #[test]
    fn normalize_does_not_fold_inside_a_segment() {
        assert_eq!(normalize("doc/..hidden.md"), "doc/..hidden.md");
        assert_eq!(normalize("doc/a..b.md"), "doc/a..b.md");
        assert_eq!(normalize("...md"), "...md");
    }

    #[test]
    fn containment_refuses_what_will_not_travel() {
        assert_eq!(containment("../outside.md"), Some(PathRejection::Escapes));
        assert_eq!(containment("a/../../b.md"), Some(PathRejection::Escapes));
        assert_eq!(containment(".."), Some(PathRejection::Escapes));
        assert_eq!(containment("/etc/hosts"), Some(PathRejection::Absolute));
        assert_eq!(
            containment("/etc/../etc/hosts"),
            Some(PathRejection::Absolute)
        );
        // Absolute *into* the project is still absolute: it names this machine.
        assert_eq!(
            containment("/Users/x/proj/doc/design.md"),
            Some(PathRejection::Absolute)
        );
    }

    /// A path that dips out and comes back stays inside, and the awkward
    /// spellings that fold to something contained are fine.
    #[test]
    fn containment_allows_everything_that_stays_inside() {
        assert_eq!(containment("doc/design.md"), None);
        assert_eq!(containment("./sub/../real.md"), None);
        assert_eq!(containment("doc/../src/parser.rs:807"), None);
        assert_eq!(containment("."), None);
        assert_eq!(containment("doc/..hidden.md"), None);
        // Empty is not a containment question — `exists` refuses it on its own,
        // and answering here would give it two different error messages.
        assert_eq!(containment(""), None);
    }

    #[test]
    fn same_path_sees_through_spelling_but_not_through_the_suffix() {
        assert!(same_path("real.md", "./sub/../real.md"));
        assert!(same_path("./sub/../real.md", "real.md"));
        assert!(same_path("./sub/../real.md", "./sub/../real.md"));
        assert!(same_path("doc/design.md#why", "./doc/design.md#why"));
        // Different references to the same file, and `rm` must keep them apart.
        assert!(!same_path("real.md", "real.md:807"));
        assert!(!same_path("doc/design.md", "doc/design.md#why"));
        assert!(!same_path("real.md", "other.md"));
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
