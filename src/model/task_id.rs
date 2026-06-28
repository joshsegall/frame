//! The task-ID grammar, centralized in one type.
//!
//! ```text
//! task_id  = prefix "-" segment ("." segment)*
//! segment  = token? number
//! token    = lowercase letters, one or more   (None = "null" namespace)
//! number   = digits
//! prefix   = the track's configured prefix (e.g. EFF)
//! ```
//!
//! A segment is a maximal run of lowercase letters (the optional token) followed
//! by a maximal run of digits (the number); letters and digits are disjoint, so
//! no delimiter is needed between them.
//!
//! Anything that does not match the grammar is preserved verbatim as a [`Raw`]
//! ID so that parsing never rejects an input and round-trip byte-identity is
//! retained. `Raw` IDs are invisible to the mint/scan logic: they never perturb
//! the next minted number.
//!
//! [`Raw`]: ParsedId::Raw

use std::fmt;
use std::hash::{Hash, Hasher};
use std::ops::Deref;
use std::str::FromStr;

use serde::de::{Deserialize, Deserializer};
use serde::ser::{Serialize, Serializer};

/// A namespace token: one or more lowercase ASCII letters.
///
/// In Phase 1 tokens are never minted (every segment is tokenless / null
/// namespace), but the grammar and type are ready for token namespacing.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Token(String);

impl Token {
    /// Construct a token, validating that it is non-empty and all lowercase
    /// ASCII letters. Returns `None` otherwise.
    pub fn new(s: impl Into<String>) -> Option<Token> {
        let s = s.into();
        if !s.is_empty() && s.bytes().all(|b| b.is_ascii_lowercase()) {
            Some(Token(s))
        } else {
            None
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A single `token? number` segment of a structured ID.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Segment {
    /// Optional namespace token (`None` = null namespace).
    pub token: Option<Token>,
    /// The numeric value of the segment.
    pub number: u32,
    /// Minimum rendered digit width, preserving zero-padding (e.g. `014` → 3).
    pub width: usize,
}

impl Segment {
    fn render(&self, out: &mut String) {
        use std::fmt::Write;
        if let Some(token) = &self.token {
            out.push_str(token.as_str());
        }
        let _ = write!(out, "{:0width$}", self.number, width = self.width);
    }
}

/// The parsed structure of a task ID, or `Raw` for non-conforming IDs.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum ParsedId {
    Structured {
        prefix: String,
        segments: Vec<Segment>,
    },
    Raw,
}

/// A task ID.
///
/// Equality, hashing, ordering, [`Display`](fmt::Display) and the [`Deref`] to
/// `str` all operate on the canonical text form, so a `TaskId` is a drop-in for
/// the bare `String` that used to hold task IDs — whole-string comparison
/// semantics are preserved exactly. The parsed structure is used only by the
/// mint/scan primitives.
#[derive(Debug, Clone)]
pub struct TaskId {
    /// Canonical / verbatim text. For a parsed ID this is the original input;
    /// for a minted ID this is the rendered form. Always equals the rendered
    /// form of a structured ID.
    text: String,
    parsed: ParsedId,
}

impl TaskId {
    /// Parse a string into a `TaskId`. Never fails: anything that does not match
    /// the grammar becomes a `Raw` ID that round-trips verbatim.
    pub fn parse(s: &str) -> TaskId {
        match parse_structured(s) {
            Some((prefix, segments)) => TaskId {
                text: s.to_string(),
                parsed: ParsedId::Structured { prefix, segments },
            },
            None => TaskId {
                text: s.to_string(),
                parsed: ParsedId::Raw,
            },
        }
    }

    /// Construct a top-level structured ID with the given prefix and number,
    /// zero-padded to a minimum width of 3 (matching the legacy `{:03}` mint
    /// format). The single segment is tokenless (null namespace).
    pub fn with_number(prefix: &str, number: u32) -> TaskId {
        let segments = vec![Segment {
            token: None,
            number,
            width: 3,
        }];
        let text = render(prefix, &segments);
        TaskId {
            text,
            parsed: ParsedId::Structured {
                prefix: prefix.to_string(),
                segments,
            },
        }
    }

    /// Construct a child ID by appending a tokenless, unpadded segment to the
    /// given parent (matching the legacy `{}` child-number format). If the
    /// parent is `Raw`, the child is `Raw` too (`"{parent}.{number}"`).
    pub fn child_of(parent: &TaskId, number: u32) -> TaskId {
        match &parent.parsed {
            ParsedId::Structured { prefix, segments } => {
                let mut segments = segments.clone();
                segments.push(Segment {
                    token: None,
                    number,
                    width: 1,
                });
                let text = render(prefix, &segments);
                TaskId {
                    text,
                    parsed: ParsedId::Structured {
                        prefix: prefix.clone(),
                        segments,
                    },
                }
            }
            ParsedId::Raw => TaskId::parse(&format!("{}.{}", parent.text, number)),
        }
    }

    /// The top-level number for this ID if it is a structured, null-namespace ID
    /// under `prefix`. Used by max-scan: `Raw` IDs and token (non-null) segments
    /// return `None` and so never perturb the next minted number.
    pub fn top_level_number(&self, prefix: &str) -> Option<u32> {
        match &self.parsed {
            ParsedId::Structured {
                prefix: p,
                segments,
            } if p == prefix => {
                let first = segments.first()?;
                if first.token.is_none() {
                    Some(first.number)
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    /// If `self` is a direct, null-namespace child of `parent` (one extra
    /// segment whose preceding segments match the parent exactly), return the
    /// child number. Used by the gap-safe child-number scan.
    pub fn child_number_of(&self, parent: &TaskId) -> Option<u32> {
        match (&self.parsed, &parent.parsed) {
            (
                ParsedId::Structured {
                    prefix: cp,
                    segments: cs,
                },
                ParsedId::Structured {
                    prefix: pp,
                    segments: ps,
                },
            ) if cp == pp && cs.len() == ps.len() + 1 && cs[..ps.len()] == ps[..] => {
                let last = cs.last()?;
                if last.token.is_none() {
                    Some(last.number)
                } else {
                    None
                }
            }
            _ => None,
        }
    }
}

/// Render a structured ID to its canonical text form.
fn render(prefix: &str, segments: &[Segment]) -> String {
    let mut out = String::with_capacity(prefix.len() + 1 + segments.len() * 4);
    out.push_str(prefix);
    out.push('-');
    for (i, seg) in segments.iter().enumerate() {
        if i > 0 {
            out.push('.');
        }
        seg.render(&mut out);
    }
    out
}

/// Try to parse `s` as a structured ID. Returns `None` for non-conforming input.
fn parse_structured(s: &str) -> Option<(String, Vec<Segment>)> {
    let dash = s.find('-')?;
    let prefix = &s[..dash];
    let rest = &s[dash + 1..];
    if prefix.is_empty() || !prefix.bytes().all(|b| b.is_ascii_alphanumeric()) {
        return None;
    }
    if rest.is_empty() {
        return None;
    }
    let mut segments = Vec::new();
    for piece in rest.split('.') {
        segments.push(parse_segment(piece)?);
    }
    Some((prefix.to_string(), segments))
}

fn parse_segment(piece: &str) -> Option<Segment> {
    if piece.is_empty() {
        return None;
    }
    let bytes = piece.as_bytes();
    let mut i = 0;
    while i < bytes.len() && bytes[i].is_ascii_lowercase() {
        i += 1;
    }
    let token_str = &piece[..i];
    let num_str = &piece[i..];
    if num_str.is_empty() || !num_str.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let number: u32 = num_str.parse().ok()?;
    let token = if token_str.is_empty() {
        None
    } else {
        Some(Token::new(token_str)?)
    };
    Some(Segment {
        token,
        number,
        width: num_str.len(),
    })
}

// --- Canonical-form comparison, hashing, ordering, display ---

impl PartialEq for TaskId {
    fn eq(&self, other: &Self) -> bool {
        self.text == other.text
    }
}

impl Eq for TaskId {}

impl Hash for TaskId {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.text.hash(state);
    }
}

impl PartialOrd for TaskId {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for TaskId {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.text.cmp(&other.text)
    }
}

impl Deref for TaskId {
    type Target = str;
    fn deref(&self) -> &str {
        &self.text
    }
}

impl fmt::Display for TaskId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.text)
    }
}

impl From<&str> for TaskId {
    fn from(s: &str) -> Self {
        TaskId::parse(s)
    }
}

impl From<String> for TaskId {
    fn from(s: String) -> Self {
        TaskId::parse(&s)
    }
}

impl FromStr for TaskId {
    type Err = std::convert::Infallible;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(TaskId::parse(s))
    }
}

impl Serialize for TaskId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.text)
    }
}

impl<'de> Deserialize<'de> for TaskId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Ok(TaskId::parse(&s))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn roundtrip(s: &str) {
        let id = TaskId::parse(s);
        assert_eq!(id.to_string(), s, "display must round-trip {s}");
        assert_eq!(&*id, s, "deref must round-trip {s}");
    }

    #[test]
    fn grammar_round_trip() {
        for s in [
            "EFF-14",
            "EFF-a14",
            "EFF-foo14",
            "EFF-14.2",
            "EFF-a14.b2",
            "EFF-a14.b2.c3",
            "EFF-14.b1",
        ] {
            roundtrip(s);
            assert!(
                matches!(TaskId::parse(s).parsed, ParsedId::Structured { .. }),
                "{s} should parse structured"
            );
        }
    }

    #[test]
    fn structured_parts() {
        let id = TaskId::parse("EFF-a14.b2");
        match &id.parsed {
            ParsedId::Structured { prefix, segments } => {
                assert_eq!(prefix, "EFF");
                assert_eq!(segments.len(), 2);
                assert_eq!(segments[0].token.as_ref().unwrap().as_str(), "a");
                assert_eq!(segments[0].number, 14);
                assert_eq!(segments[1].token.as_ref().unwrap().as_str(), "b");
                assert_eq!(segments[1].number, 2);
            }
            ParsedId::Raw => panic!("expected structured"),
        }
    }

    #[test]
    fn raw_preservation() {
        for s in ["weird id!", "EFF-", "EFF-1..2", "-5", "EFF-1a", "lower-1.x"] {
            let id = TaskId::parse(s);
            assert_eq!(id.to_string(), s, "raw must round-trip verbatim: {s}");
        }
        assert!(matches!(TaskId::parse("weird id!").parsed, ParsedId::Raw));
        // `EFF-1a` has trailing letters after digits → not a valid segment.
        assert!(matches!(TaskId::parse("EFF-1a").parsed, ParsedId::Raw));
    }

    #[test]
    fn raw_invisible_to_top_level_scan() {
        let raw = TaskId::parse("weird id!");
        assert_eq!(raw.top_level_number("EFF"), None);
        // A token (non-null) segment is also invisible to the null-namespace scan.
        assert_eq!(TaskId::parse("EFF-a14").top_level_number("EFF"), None);
        // A structured, null-namespace ID is visible.
        assert_eq!(TaskId::parse("EFF-14").top_level_number("EFF"), Some(14));
        // Subtasks expose their parent's top-level number.
        assert_eq!(TaskId::parse("EFF-14.2").top_level_number("EFF"), Some(14));
        // Different prefix → invisible.
        assert_eq!(TaskId::parse("EFF-14").top_level_number("INF"), None);
    }

    #[test]
    fn child_number_scan() {
        let parent = TaskId::parse("T-001");
        assert_eq!(TaskId::parse("T-001.1").child_number_of(&parent), Some(1));
        assert_eq!(TaskId::parse("T-001.4").child_number_of(&parent), Some(4));
        // Grandchildren are not direct children.
        assert_eq!(TaskId::parse("T-001.1.1").child_number_of(&parent), None);
        // Different parent.
        assert_eq!(TaskId::parse("T-002.1").child_number_of(&parent), None);
    }

    #[test]
    fn padding_preserved() {
        assert_eq!(TaskId::parse("EFF-014").to_string(), "EFF-014");
        assert_eq!(TaskId::parse("ST-001").to_string(), "ST-001");
        // Mint reproduces the legacy 3-wide zero padding.
        assert_eq!(TaskId::with_number("T", 5).to_string(), "T-005");
        assert_eq!(TaskId::with_number("T", 142).to_string(), "T-142");
        assert_eq!(TaskId::with_number("T", 1000).to_string(), "T-1000");
    }

    #[test]
    fn child_of_construction() {
        let parent = TaskId::parse("EFF-014");
        assert_eq!(TaskId::child_of(&parent, 1).to_string(), "EFF-014.1");
        assert_eq!(TaskId::child_of(&parent, 12).to_string(), "EFF-014.12");
        let grandparent = TaskId::child_of(&parent, 2);
        assert_eq!(TaskId::child_of(&grandparent, 3).to_string(), "EFF-014.2.3");
        // Raw parent → raw child, matching the legacy format.
        let raw = TaskId::parse("weird");
        assert_eq!(TaskId::child_of(&raw, 1).to_string(), "weird.1");
    }

    #[test]
    fn hash_and_eq() {
        assert_eq!(TaskId::parse("EFF-a14"), TaskId::parse("EFF-a14"));
        assert_ne!(TaskId::parse("EFF-a14"), TaskId::parse("EFF-14"));
        assert_ne!(TaskId::parse("EFF-a14"), TaskId::parse("EFF-b14"));

        let mut map: HashMap<TaskId, i32> = HashMap::new();
        map.insert(TaskId::parse("EFF-014"), 1);
        map.insert(TaskId::parse("EFF-014.2"), 2);
        assert_eq!(map.get(&TaskId::parse("EFF-014")), Some(&1));
        assert_eq!(map.get(&TaskId::parse("EFF-014.2")), Some(&2));
        assert_eq!(map.get(&TaskId::parse("EFF-999")), None);
    }

    #[test]
    fn deref_enables_str_ops() {
        let id = TaskId::parse("EFF-014.2");
        assert!(id.starts_with("EFF-"));
        assert!(id.contains('.'));
        assert_eq!(id.len(), "EFF-014.2".len());
    }
}
