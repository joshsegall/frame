//! Merge one or more actor-token namespaces into a single target namespace.
//!
//! Actor tokens proliferate when a workflow spins up many working copies (e.g. a
//! new git worktree per session): each unclaimed clone auto-claims a fresh token
//! on its first mint, so one machine ends up owning several. `fr actor merge`
//! collapses those namespaces back into one, renumbering the minted ids and
//! rewriting every reference.
//!
//! The remap is **per-segment**, not per-subtree. A task id is a chain of
//! `token? number` segments (see [`crate::model::task_id`]); merging tokens
//! `{d, f}` into `b` rewrites only the segments minted by `d`/`f` and leaves
//! every other segment — null, `b` itself, or a third actor's — verbatim. So
//! `SEC-d1.a3` becomes `SEC-b2.a3`, never `SEC-b2.b1`: actor `a`'s child segment
//! is preserved. This is deliberately different from [`rekey_subtree`], which
//! flattens a whole subtree into one namespace for reparent/move.
//!
//! [`rekey_subtree`]: crate::ops::task_ops::rekey_subtree

use std::collections::{HashMap, HashSet};

use crate::model::task::{Metadata, Task};
use crate::model::task_id::{Segment, TaskId, Token};
use crate::model::track::{Track, TrackNode};

/// Validate the syntactic shape of a merge request (token grammar, non-empty
/// `from`, and no self-merge). Registry-level checks (existence/active state)
/// are layered on by the caller, which holds the registry.
pub fn validate_merge_request(from: &[String], into: &str) -> Result<(), String> {
    crate::io::actors::validate_token(into)?;
    if into == "null" {
        return Err(
            "cannot merge into the null (primary) namespace — pick a tokened target".to_string(),
        );
    }
    if from.is_empty() {
        return Err(
            "no source tokens given (usage: fr actor merge <from>... --into <token>)".into(),
        );
    }
    let mut seen = HashSet::new();
    for tok in from {
        crate::io::actors::validate_token(tok)?;
        if tok == "null" {
            return Err("cannot merge the null (primary) namespace away".to_string());
        }
        if tok == into {
            return Err(format!("token '{}' is both a source and the target", tok));
        }
        if !seen.insert(tok.clone()) {
            return Err(format!("token '{}' listed more than once", tok));
        }
    }
    Ok(())
}

/// Whether a segment sits in the `into` namespace (`into = None` means null).
fn seg_is_into(seg: &Segment, into: Option<&Token>) -> bool {
    seg.token.as_ref() == into
}

/// Whether an id's last segment was minted by one of the merged tokens.
fn last_in_from(id: &TaskId, from: &HashSet<String>) -> bool {
    id.segments()
        .and_then(|(_, segs)| segs.last())
        .and_then(|s| s.token.as_ref())
        .map(|t| from.contains(t.as_str()))
        .unwrap_or(false)
}

/// The new segments of an id's parent (all but the last segment), looked up from
/// `memo`. Empty for a top-level id. Falls back to the verbatim parent segments
/// if the parent is somehow absent from the id set (defensive).
fn parent_new_segments(id: &TaskId, memo: &HashMap<String, TaskId>) -> Vec<Segment> {
    let (prefix, segs) = id.segments().expect("structured id");
    if segs.len() <= 1 {
        return Vec::new();
    }
    let parent_old = TaskId::from_segments(prefix, segs[..segs.len() - 1].to_vec());
    match memo.get(parent_old.as_str()) {
        Some(pnew) => pnew.segments().map(|(_, s)| s.to_vec()).unwrap_or_default(),
        None => segs[..segs.len() - 1].to_vec(),
    }
}

/// The counter key for allocating a new `into` number: the prefix plus the
/// rendered new-parent path (empty string for a top-level context).
fn context_key(prefix: &str, parent_new: &[Segment]) -> (String, String) {
    let path = if parent_new.is_empty() {
        String::new()
    } else {
        TaskId::from_segments(prefix, parent_new.to_vec())
            .as_str()
            .to_string()
    };
    (prefix.to_string(), path)
}

/// Build the old→new id map for merging every `from` namespace into `into`
/// (`into = None` means the null namespace, which the CLI disallows as a target).
///
/// Returns only ids that actually change, sorted by old text. Numbers are
/// allocated per `(prefix, new-parent-path)` context — seeded from the max
/// number already present in that context's `into` namespace — so a merged id
/// never collides with an existing target id or with another merged sibling.
///
/// Processing proceeds tier by tier (segment count ascending) so an id's parent
/// is always finalized first; within a tier, preserved `into` segments are
/// visited before substituted ones so their numbers seed the counter.
pub fn build_merge_map(
    all_ids: &[TaskId],
    from: &HashSet<String>,
    into: Option<&Token>,
) -> Vec<(TaskId, TaskId)> {
    let mut ids: Vec<&TaskId> = all_ids
        .iter()
        .filter(|id| id.segments().is_some())
        .collect();
    ids.sort_by(|a, b| a.as_str().cmp(b.as_str()));
    ids.dedup_by(|a, b| a.as_str() == b.as_str());

    let into_width = if into.is_some() { 1 } else { 3 };
    let max_len = ids
        .iter()
        .map(|id| id.segments().unwrap().1.len())
        .max()
        .unwrap_or(0);

    let mut memo: HashMap<String, TaskId> = HashMap::new();
    let mut counters: HashMap<(String, String), u32> = HashMap::new();

    for tier in 1..=max_len {
        let tier_ids = || {
            ids.iter()
                .copied()
                .filter(move |id| id.segments().unwrap().1.len() == tier)
        };
        // Preserved-last first (they seed the target counters), then substituted.
        let mut preserved: Vec<&TaskId> = tier_ids().filter(|id| !last_in_from(id, from)).collect();
        let mut substituted: Vec<&TaskId> =
            tier_ids().filter(|id| last_in_from(id, from)).collect();
        preserved.sort_by(|a, b| a.as_str().cmp(b.as_str()));
        substituted.sort_by(|a, b| a.as_str().cmp(b.as_str()));

        for id in preserved {
            let (prefix, segs) = id.segments().unwrap();
            let parent_new = parent_new_segments(id, &memo);
            let last = segs.last().unwrap().clone();
            if seg_is_into(&last, into) {
                let key = context_key(prefix, &parent_new);
                let e = counters.entry(key).or_insert(0);
                *e = (*e).max(last.number);
            }
            let mut new_segs = parent_new;
            new_segs.push(last);
            memo.insert(
                id.as_str().to_string(),
                TaskId::from_segments(prefix, new_segs),
            );
        }

        for id in substituted {
            let (prefix, _segs) = id.segments().unwrap();
            let parent_new = parent_new_segments(id, &memo);
            let key = context_key(prefix, &parent_new);
            let number = {
                let e = counters.entry(key).or_insert(0);
                *e += 1;
                *e
            };
            let mut new_segs = parent_new;
            new_segs.push(Segment {
                token: into.cloned(),
                number,
                width: into_width,
            });
            memo.insert(
                id.as_str().to_string(),
                TaskId::from_segments(prefix, new_segs),
            );
        }
    }

    let mut out: Vec<(TaskId, TaskId)> = ids
        .iter()
        .filter_map(|id| {
            memo.get(id.as_str())
                .filter(|new| new.as_str() != id.as_str())
                .map(|new| ((*id).clone(), new.clone()))
        })
        .collect();
    out.sort_by(|a, b| a.0.as_str().cmp(b.0.as_str()));
    out
}

// ---------------------------------------------------------------------------
// Collecting ids and applying a map to loaded task trees
// ---------------------------------------------------------------------------

/// Collect every structured task id in a list of tasks (recursing into subtasks).
pub fn collect_ids(tasks: &[Task], out: &mut Vec<TaskId>) {
    for t in tasks {
        if let Some(id) = &t.id {
            out.push(id.clone());
        }
        collect_ids(&t.subtasks, out);
    }
}

/// Collect every structured task id across a track's sections.
pub fn collect_ids_in_track(track: &Track, out: &mut Vec<TaskId>) {
    for node in &track.nodes {
        if let TrackNode::Section { tasks, .. } = node {
            collect_ids(tasks, out);
        }
    }
}

/// A prose (note/spec/ref text) occurrence of a remapped id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProseHit {
    pub old: String,
    pub new: String,
    /// The occurrence looks like a git citation (`fix(ID)` or near a commit sha)
    /// and is therefore never auto-rewritten — only reported.
    pub is_citation: bool,
    /// A short snippet of the surrounding text for the report.
    pub context: String,
}

/// Apply an id map to a task tree: rewrite ids and `dep:` references, and
/// (when `rewrite_notes`) mentions inside note/spec/ref prose — skipping any
/// occurrence that looks like a git citation. Returns whether anything changed
/// and the list of prose occurrences found (for the report, regardless of
/// `rewrite_notes`).
pub fn apply_map_to_tasks(
    tasks: &mut [Task],
    map: &HashMap<String, TaskId>,
    rewrite_notes: bool,
    hits: &mut Vec<ProseHit>,
) -> bool {
    let mut any = false;
    for t in tasks.iter_mut() {
        let mut dirty = false;

        if let Some(id) = &t.id
            && let Some(new) = map.get(id.as_str())
        {
            t.id = Some(new.clone());
            dirty = true;
        }

        for m in &mut t.metadata {
            match m {
                Metadata::Dep(deps) => {
                    for d in deps.iter_mut() {
                        if let Some(new) = map.get(d.as_str()) {
                            *d = new.as_str().to_string();
                            dirty = true;
                        }
                    }
                }
                Metadata::Note(s) | Metadata::Spec(s) => {
                    let (new_text, found) = scan_prose(s, map, rewrite_notes);
                    if !found.is_empty() {
                        hits.extend(found);
                        if rewrite_notes && &new_text != s {
                            *s = new_text;
                            dirty = true;
                        }
                    }
                }
                Metadata::Ref(refs) => {
                    for r in refs.iter_mut() {
                        let (new_text, found) = scan_prose(r, map, rewrite_notes);
                        if !found.is_empty() {
                            hits.extend(found);
                            if rewrite_notes && &new_text != r {
                                *r = new_text;
                                dirty = true;
                            }
                        }
                    }
                }
                Metadata::Added(_) | Metadata::Resolved(_) => {}
            }
        }

        if dirty {
            t.mark_dirty();
            any = true;
        }
        if apply_map_to_tasks(&mut t.subtasks, map, rewrite_notes, hits) {
            any = true;
        }
    }
    any
}

/// Apply an id map across a whole track. Returns whether anything changed.
pub fn apply_map_to_track(
    track: &mut Track,
    map: &HashMap<String, TaskId>,
    rewrite_notes: bool,
    hits: &mut Vec<ProseHit>,
) -> bool {
    let mut any = false;
    for node in &mut track.nodes {
        if let TrackNode::Section { tasks, .. } = node
            && apply_map_to_tasks(tasks, map, rewrite_notes, hits)
        {
            any = true;
        }
    }
    any
}

/// Find remapped ids mentioned in a prose string. Returns the (possibly
/// rewritten) text and the occurrences found. An id is matched only on token
/// boundaries so `BAC-f1` does not match inside `BAC-f10` or `BAC-f1.2`.
/// Occurrences that look like git citations are reported but never rewritten.
fn scan_prose(text: &str, map: &HashMap<String, TaskId>, rewrite: bool) -> (String, Vec<ProseHit>) {
    let mut hits = Vec::new();
    // Try longest old-ids first so a longer id wins over a prefix of it.
    let mut olds: Vec<&String> = map.keys().collect();
    olds.sort_by(|a, b| b.len().cmp(&a.len()).then(a.as_str().cmp(b.as_str())));

    let mut result = text.to_string();
    for old in olds {
        let new = map.get(old).unwrap().as_str();
        let mut search_from = 0;
        while let Some(rel) = result[search_from..].find(old.as_str()) {
            let start = search_from + rel;
            let end = start + old.len();
            let before_ok = start == 0
                || !result.as_bytes()[start - 1].is_ascii_alphanumeric()
                    && result.as_bytes()[start - 1] != b'-';
            let after_ok = end >= result.len() || {
                let bytes = result.as_bytes();
                let c = bytes[end];
                if c.is_ascii_alphanumeric() {
                    false
                } else if c == b'.' {
                    // A `.` continues the id only when followed by a digit (a
                    // subsegment, `BAC-f1.2`). A trailing sentence period
                    // (`BAC-f1.`) is a boundary, so the id still matches.
                    !(end + 1 < bytes.len() && bytes[end + 1].is_ascii_digit())
                } else {
                    true
                }
            };
            if before_ok && after_ok {
                let is_citation = looks_like_citation(&result, start, end);
                hits.push(ProseHit {
                    old: old.clone(),
                    new: new.to_string(),
                    is_citation,
                    context: snippet(&result, start, end),
                });
                if rewrite && !is_citation {
                    result.replace_range(start..end, new);
                    search_from = start + new.len();
                    continue;
                }
            }
            search_from = end;
        }
    }
    (result, hits)
}

/// Heuristic: the id occurrence at `start..end` reads as a git citation and must
/// not be rewritten. Triggers on `fix(ID)`/`feat(ID)`-style wrappers or a commit
/// sha (7+ hex chars) within a short window before the id on the same context.
fn looks_like_citation(text: &str, start: usize, end: usize) -> bool {
    let bytes = text.as_bytes();
    // `(` immediately before and `)` immediately after → `fix(ID)` form.
    if start > 0 && bytes[start - 1] == b'(' && end < bytes.len() && bytes[end] == b')' {
        return true;
    }
    // A hex run of length >= 7 within ~16 chars before the id.
    let window_start = start.saturating_sub(20);
    let window = &text[window_start..start];
    let mut run = 0usize;
    for c in window.chars() {
        if c.is_ascii_hexdigit() {
            run += 1;
            if run >= 7 {
                return true;
            }
        } else {
            run = 0;
        }
    }
    false
}

/// A trimmed one-line snippet around `start..end` for the report.
fn snippet(text: &str, start: usize, end: usize) -> String {
    let from = text[..start]
        .rfind('\n')
        .map(|i| i + 1)
        .unwrap_or(0)
        .max(start.saturating_sub(40));
    let to = text[end..]
        .find('\n')
        .map(|i| end + i)
        .unwrap_or(text.len())
        .min(end + 40);
    text[from..to].trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(list: &[&str]) -> Vec<TaskId> {
        list.iter().map(|s| TaskId::parse(s)).collect()
    }

    fn from(list: &[&str]) -> HashSet<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    fn tok(s: &str) -> Token {
        Token::new(s).unwrap()
    }

    fn map_of(pairs: &[(TaskId, TaskId)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(a, b)| (a.as_str().to_string(), b.as_str().to_string()))
            .collect()
    }

    #[test]
    fn top_level_renumber_continues_target_sequence() {
        // The lace shape: BAC-b1..b3 exist; merging f into b renumbers BAC-f* to
        // continue after the max b.
        let all = ids(&["BAC-b1", "BAC-b2", "BAC-b3", "BAC-f1", "BAC-f2", "BAC-f3"]);
        let out = build_merge_map(&all, &from(&["f"]), Some(&tok("b")));
        assert_eq!(
            map_of(&out),
            vec![
                ("BAC-f1".into(), "BAC-b4".into()),
                ("BAC-f2".into(), "BAC-b5".into()),
                ("BAC-f3".into(), "BAC-b6".into()),
            ]
        );
    }

    #[test]
    fn merges_multiple_sources_into_one_target() {
        let all = ids(&["SEC-b1", "SEC-d1", "SEC-f1"]);
        let out = build_merge_map(&all, &from(&["d", "f"]), Some(&tok("b")));
        // d1 sorts before f1, so d1 -> b2, f1 -> b3.
        assert_eq!(
            map_of(&out),
            vec![
                ("SEC-d1".into(), "SEC-b2".into()),
                ("SEC-f1".into(), "SEC-b3".into()),
            ]
        );
    }

    #[test]
    fn empty_target_namespace_starts_at_one() {
        let all = ids(&["TOO-d1", "TOO-d2"]);
        let out = build_merge_map(&all, &from(&["d"]), Some(&tok("b")));
        assert_eq!(
            map_of(&out),
            vec![
                ("TOO-d1".into(), "TOO-b1".into()),
                ("TOO-d2".into(), "TOO-b2".into()),
            ]
        );
    }

    #[test]
    fn preserves_other_actors_child_segment() {
        // Merging d -> b rewrites only the d segment; actor a's child stays a3.
        let all = ids(&["SEC-b1", "SEC-d1", "SEC-d1.a3"]);
        let out = build_merge_map(&all, &from(&["d"]), Some(&tok("b")));
        assert_eq!(
            map_of(&out),
            vec![
                ("SEC-d1".into(), "SEC-b2".into()),
                ("SEC-d1.a3".into(), "SEC-b2.a3".into()),
            ]
        );
    }

    #[test]
    fn preserves_null_child_under_merged_parent() {
        // A null child (EFF-d1.2) keeps its number; only the parent d1 changes.
        let all = ids(&["EFF-d1", "EFF-d1.1", "EFF-d1.2"]);
        let out = build_merge_map(&all, &from(&["d"]), Some(&tok("b")));
        assert_eq!(
            map_of(&out),
            vec![
                ("EFF-d1".into(), "EFF-b1".into()),
                ("EFF-d1.1".into(), "EFF-b1.1".into()),
                ("EFF-d1.2".into(), "EFF-b1.2".into()),
            ]
        );
    }

    #[test]
    fn substituted_children_under_new_parent_dont_collide() {
        // Under the merged parent, a preserved b child and a substituted d child
        // must not collide: b3 stays b3, the d child becomes b4.
        let all = ids(&["EFF-f1", "EFF-f1.b3", "EFF-f1.d7"]);
        let out = build_merge_map(&all, &from(&["d", "f"]), Some(&tok("b")));
        assert_eq!(
            map_of(&out),
            vec![
                ("EFF-f1".into(), "EFF-b1".into()),
                ("EFF-f1.b3".into(), "EFF-b1.b3".into()),
                ("EFF-f1.d7".into(), "EFF-b1.b4".into()),
            ]
        );
    }

    #[test]
    fn untouched_ids_and_widths_are_preserved() {
        // Null-namespace ids keep their 3-wide padding and are never remapped.
        let all = ids(&["EFF-014", "EFF-014.1", "EFF-d1"]);
        let out = build_merge_map(&all, &from(&["d"]), Some(&tok("b")));
        assert_eq!(map_of(&out), vec![("EFF-d1".into(), "EFF-b1".into())]);
    }

    #[test]
    fn different_prefixes_have_independent_sequences() {
        let all = ids(&["BAC-b5", "BAC-d1", "SEC-d1"]);
        let out = build_merge_map(&all, &from(&["d"]), Some(&tok("b")));
        assert_eq!(
            map_of(&out),
            vec![
                ("BAC-d1".into(), "BAC-b6".into()),
                ("SEC-d1".into(), "SEC-b1".into()),
            ]
        );
    }

    #[test]
    fn validate_rejects_bad_requests() {
        assert!(validate_merge_request(&[], "b").is_err()); // no sources
        assert!(validate_merge_request(&["b".into()], "b").is_err()); // self-merge
        assert!(validate_merge_request(&["null".into()], "b").is_err()); // merge null away
        assert!(validate_merge_request(&["d".into()], "null").is_err()); // into null
        assert!(validate_merge_request(&["d".into(), "d".into()], "b").is_err()); // dup source
        assert!(validate_merge_request(&["d".into(), "f".into()], "b").is_ok());
    }

    #[test]
    fn prose_scan_matches_on_boundaries_only() {
        let mut map: HashMap<String, TaskId> = HashMap::new();
        map.insert("BAC-f1".into(), TaskId::parse("BAC-b17"));
        // BAC-f1 in `BAC-f10` and `BAC-f1.2` must NOT match; the standalone one does.
        let (out, hits) = scan_prose("see BAC-f1, not BAC-f10 nor BAC-f1.2", &map, true);
        assert_eq!(hits.len(), 1);
        assert_eq!(out, "see BAC-b17, not BAC-f10 nor BAC-f1.2");
    }

    #[test]
    fn prose_scan_matches_id_before_a_sentence_period() {
        let mut map: HashMap<String, TaskId> = HashMap::new();
        map.insert("BAC-f1".into(), TaskId::parse("BAC-b6"));
        // A trailing period ends the sentence, not a subsegment: it still matches.
        // But `BAC-f1.2` (period + digit) is a different id and must not match.
        let (out, hits) = scan_prose("captured as BAC-f1. See BAC-f1.2 later", &map, true);
        assert_eq!(hits.len(), 1);
        assert_eq!(out, "captured as BAC-b6. See BAC-f1.2 later");
    }

    #[test]
    fn prose_scan_reports_but_skips_citations() {
        let mut map: HashMap<String, TaskId> = HashMap::new();
        map.insert("SEC-d1".into(), TaskId::parse("SEC-b2"));
        // `fix(SEC-d1)` and a sha-prefixed mention are reported but not rewritten.
        let (out, hits) = scan_prose("added by 77071079 fix(SEC-d1)", &map, true);
        assert_eq!(hits.len(), 1);
        assert!(hits[0].is_citation);
        assert_eq!(out, "added by 77071079 fix(SEC-d1)"); // unchanged
    }
}
