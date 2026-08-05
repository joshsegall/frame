//! Three-way merge of a track or the inbox against a concurrent write.
//!
//! # Why this exists
//!
//! A TUI save can fail — most often because another `fr` process holds
//! `frame/.lock` past the five-second timeout. The edit stays in memory and
//! reaches no file. That process then *writes* the track, the watcher fires, and
//! the reload finds two versions with no common file between them: ours, which
//! exists only in memory, and theirs, which exists only on disk.
//!
//! `ed273b2` stopped that reload from silently replacing ours, which was
//! destroying work outright. But keeping ours means discarding theirs, and with
//! several sessions writing to one project — the agent workflow in
//! `doc/agent-setup.md` — an agent's write is exactly as real as a human's, and
//! the agent has no way to notice it vanished. Picking a winner is a floor, not
//! an answer.
//!
//! # Why frame's data merges well
//!
//! Tasks carry stable IDs, so the two sides can be matched by identity rather
//! than by position or by diffing text. And `source_text` already holds a task's
//! *own* lines excluding its subtasks (the "selective rewrite" design), so a
//! task is a natural comparison unit and subtasks recurse independently.
//!
//! That makes the common concurrent case merge cleanly rather than conflict. An
//! agent's realistic workload is adding tasks and changing state on tasks the
//! TUI is not touching: additions land on fresh IDs and both sides survive, and
//! a state change to a task we did not edit is taken. A conflict is left only
//! where both sides edited the *same* task differently — which is where no
//! automatic answer is right.
//!
//! # What it does not attempt
//!
//! Ordering. A task's position within its section follows whichever side it came
//! from, and a task only they added is appended to its section. Ordering is the
//! least valuable thing to be precise about here and by far the most expensive:
//! there is no stable anchor to merge positions against once both sides have
//! inserted.
//!
//! Reparenting a subtask. A subtask's ID extends its parent's, so it cannot
//! change parents without being renumbered into a different ID — at which point
//! it is an addition and a deletion, which is handled.
//!
//! # On conflict
//!
//! Ours is kept and theirs is returned in [`Reconciled::conflicts`] for the
//! caller to write to the recovery log. Nothing is dropped on either side; the
//! merge only ever narrows how often that fallback is needed.
//!
//! # The inbox works differently, deliberately
//!
//! Inbox items have no IDs, so identity-matching has nothing to stand on.
//! [`reconcile_inbox`] merges by content as a multiset instead, which fits what
//! the inbox is for — captures and removals — and which resolves a double edit
//! by keeping both versions rather than setting one aside. See its own docs for
//! why that is the right answer there and the wrong one for tracks.

use std::collections::HashMap;

use crate::model::inbox::{Inbox, InboxItem};
use crate::model::task::Task;
use crate::model::track::{SectionKind, Track, TrackNode};

/// A task the merge could not decide, kept as ours with theirs preserved.
#[derive(Debug, Clone)]
pub struct Conflict {
    /// The task's ID, or its title when it has none.
    pub key: String,
    pub reason: ConflictReason,
    /// Their version, as markdown lines, for the recovery log.
    pub theirs: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictReason {
    /// Both sides changed the same task, differently.
    BothEdited,
    /// We changed it; they removed it.
    EditedAndDeleted,
    /// We removed it; they changed it. Theirs is taken — a change is more
    /// specific evidence of intent than an absence, and a delete is trivially
    /// repeatable while an edit is not.
    DeletedAndEdited,
    /// Two tasks share a title and neither has an ID, so identity is ambiguous.
    AmbiguousTitle,
}

impl ConflictReason {
    /// A phrase for a human reading a conflict report, stating what happened and
    /// which side the merge kept — the two things someone has to know before
    /// they can resolve it.
    /// A stable identifier for the reason, for the `conflict:` metadata a merge
    /// leaves in the file. Kept separate from [`Self::describe`] so the wording
    /// of a message can change without rewriting what is committed to files.
    pub fn slug(self) -> &'static str {
        match self {
            ConflictReason::BothEdited => "both-edited",
            ConflictReason::EditedAndDeleted => "edited-and-deleted",
            ConflictReason::DeletedAndEdited => "deleted-and-edited",
            ConflictReason::AmbiguousTitle => "ambiguous-title",
        }
    }

    pub fn describe(self) -> &'static str {
        match self {
            ConflictReason::BothEdited => "both sides edited it differently; kept ours",
            ConflictReason::EditedAndDeleted => "we edited it, they removed it; kept ours",
            ConflictReason::DeletedAndEdited => "we removed it, they edited it; took theirs",
            ConflictReason::AmbiguousTitle => {
                "two untitled-ID tasks share a title, so identity is ambiguous; kept ours"
            }
        }
    }
}

/// The result of merging one track.
#[derive(Debug)]
pub struct Reconciled {
    pub track: Track,
    pub conflicts: Vec<Conflict>,
    /// Tasks whose version came from the other writer.
    pub took_theirs: usize,
    /// Tasks removed because both sides agree they are gone.
    pub deleted: usize,
}

impl Reconciled {
    /// Whether the merge took anything from them, which is what makes the result
    /// differ from what we already had.
    pub fn changed_anything(&self) -> bool {
        self.took_theirs > 0 || self.deleted > 0
    }
}

/// Merge `ours` and `theirs` over their common ancestor `base`.
///
/// `base` is the last content known to be on disk — what we loaded, or what we
/// last successfully wrote.
pub fn reconcile_track(base: &Track, ours: &Track, theirs: &Track) -> Reconciled {
    let bi = index(base);
    let oi = index(ours);
    let ti = index(theirs);

    let ambiguous = ambiguous_keys(&[base, ours, theirs]);

    // Every key any side knows about. Ours first, in our order, so a task we
    // already had keeps its position; then theirs, so their additions follow.
    let mut keys: Vec<String> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for k in oi
        .order
        .iter()
        .chain(ti.order.iter())
        .chain(bi.order.iter())
    {
        if seen.insert(k.clone()) {
            keys.push(k.clone());
        }
    }

    let mut conflicts = Vec::new();
    let mut took_theirs = 0usize;
    let mut deleted = 0usize;
    // Resolved tasks, grouped by the section they belong in.
    let mut resolved: Vec<(SectionKind, Task)> = Vec::new();

    for key in &keys {
        let b = bi.entries.get(key);
        let o = oi.entries.get(key);
        let t = ti.entries.get(key);

        // An ambiguous title cannot be matched across sides with any
        // confidence, so the merge declines rather than guesses.
        if ambiguous.contains(key) {
            if let Some(o) = o {
                resolved.push((o.section, o.task.clone()));
                if let Some(t) = t {
                    conflicts.push(Conflict {
                        key: key.clone(),
                        reason: ConflictReason::AmbiguousTitle,
                        theirs: own_lines(&t.task),
                    });
                }
            }
            continue;
        }

        let outcome = decide(b, o, t);

        match outcome {
            Outcome::Delete => {
                deleted += 1;
            }
            Outcome::Ours => {
                let o = o.expect("Outcome::Ours requires our side");
                resolved.push((
                    section_for(b, Some(o), t),
                    merged_task(
                        &o.task,
                        b.map(|e| &e.task),
                        t.map(|e| &e.task),
                        &mut conflicts,
                    ),
                ));
            }
            Outcome::Theirs => {
                let t = t.expect("Outcome::Theirs requires their side");
                took_theirs += 1;
                resolved.push((
                    section_for(b, o, Some(t)),
                    merged_task(
                        &t.task,
                        b.map(|e| &e.task),
                        o.map(|e| &e.task),
                        &mut conflicts,
                    ),
                ));
            }
            Outcome::Conflict(reason) => {
                // Keep ours where we have it; otherwise the conflict is
                // deleted-by-us versus edited-by-them, and theirs is taken.
                match (o, t) {
                    (Some(o), Some(t)) => {
                        conflicts.push(Conflict {
                            key: key.clone(),
                            reason,
                            theirs: own_lines(&t.task),
                        });
                        resolved.push((
                            section_for(b, Some(o), Some(t)),
                            merged_task(&o.task, b.map(|e| &e.task), Some(&t.task), &mut conflicts),
                        ));
                    }
                    (Some(o), None) => {
                        conflicts.push(Conflict {
                            key: key.clone(),
                            reason,
                            theirs: Vec::new(),
                        });
                        resolved.push((o.section, o.task.clone()));
                    }
                    (None, Some(t)) => {
                        took_theirs += 1;
                        resolved.push((t.section, t.task.clone()));
                    }
                    (None, None) => {}
                }
            }
        }
    }

    Reconciled {
        track: rebuild(ours, theirs, resolved),
        conflicts,
        took_theirs,
        deleted,
    }
}

// ---------------------------------------------------------------------------
// Inbox
// ---------------------------------------------------------------------------

/// The result of merging the inbox.
#[derive(Debug)]
pub struct ReconciledInbox {
    pub inbox: Inbox,
    /// Items that came from the other writer.
    pub took_theirs: usize,
    /// Items removed because both sides agree they are gone.
    pub deleted: usize,
}

impl ReconciledInbox {
    pub fn changed_anything(&self) -> bool {
        self.took_theirs > 0 || self.deleted > 0
    }
}

/// Merge the inbox by content, as a multiset.
///
/// # Why this is not the track algorithm
///
/// Inbox items have **no IDs**. There is nothing stable to match on, so the
/// track merge's central move — pair the two sides up by identity, then ask who
/// changed what — has no foundation here.
///
/// What the inbox actually gets is captures and removals: `fr capture` appends,
/// triage takes an item away. Both are exactly expressible as multiset
/// arithmetic on content, and the standard three-way count
/// (`ours + theirs - base`, floored at zero) handles them without needing
/// identity at all.
///
/// An *edit* then reads as a removal plus a capture, and that turns out to be
/// the right reading rather than a compromise. If we edited an item and they did
/// not, the old text is gone from our side and the new text is new: the old
/// falls out, the new stays. Same in reverse. And if **both** sides edited the
/// same item differently, both versions survive as two items — which for a
/// quick-capture list is the better answer. A duplicate here costs one triage
/// keystroke; a dropped capture is a thought the user cannot get back.
///
/// That is why this reports no conflicts and writes nothing to the recovery log:
/// there is no case where it sets a side's content aside. Tracks cannot work
/// this way — duplicating a task means duplicating an ID, which is damage
/// `fr check` reports as an error.
pub fn reconcile_inbox(base: &Inbox, ours: &Inbox, theirs: &Inbox) -> ReconciledInbox {
    let base_counts = counts(&base.items);
    let our_counts = counts(&ours.items);
    let their_counts = counts(&theirs.items);

    // How many of each item the merged inbox should hold.
    let mut wanted: HashMap<String, usize> = HashMap::new();
    for key in our_counts
        .keys()
        .chain(their_counts.keys())
        .chain(base_counts.keys())
    {
        if wanted.contains_key(key) {
            continue;
        }
        let b = *base_counts.get(key).unwrap_or(&0) as isize;
        let o = *our_counts.get(key).unwrap_or(&0) as isize;
        let t = *their_counts.get(key).unwrap_or(&0) as isize;
        wanted.insert(key.clone(), (o + t - b).max(0) as usize);
    }

    // Ours first, in our order, so what the user is looking at does not jump
    // around; then whatever they added that we have not accounted for.
    let mut remaining = wanted.clone();
    let mut items: Vec<InboxItem> = Vec::new();
    let mut deleted = 0usize;

    for item in &ours.items {
        let key = item_key(item);
        match remaining.get_mut(&key) {
            Some(n) if *n > 0 => {
                *n -= 1;
                items.push(item.clone());
            }
            _ => deleted += 1,
        }
    }

    let mut took_theirs = 0usize;
    for item in &theirs.items {
        let key = item_key(item);
        if let Some(n) = remaining.get_mut(&key)
            && *n > 0
        {
            *n -= 1;
            items.push(item.clone());
            took_theirs += 1;
        }
    }

    // Header follows the same rule as a track's literal content: take theirs
    // only when we did not touch it ourselves.
    let header_lines = if ours.header_lines == base.header_lines {
        theirs.header_lines.clone()
    } else {
        ours.header_lines.clone()
    };

    ReconciledInbox {
        inbox: Inbox {
            header_lines,
            items,
            source_lines: ours.source_lines.clone(),
            // Ours, like `source_lines` above: the merge keeps our file's
            // shape and merges their content into it.
            eol: ours.eol,
        },
        took_theirs,
        deleted,
    }
}

fn counts(items: &[InboxItem]) -> HashMap<String, usize> {
    let mut out: HashMap<String, usize> = HashMap::new();
    for item in items {
        *out.entry(item_key(item)).or_default() += 1;
    }
    out
}

/// An inbox item's content, as its identity.
///
/// Built by hand rather than using the derived `PartialEq`, which also compares
/// `source_text` and `dirty` — two items with identical content would otherwise
/// compare unequal purely because one had been through an edit.
fn item_key(item: &InboxItem) -> String {
    format!(
        "{}\u{1}{}\u{1}{}",
        item.title,
        item.tags.join(","),
        item.body.as_deref().unwrap_or("")
    )
}

// ---------------------------------------------------------------------------
// Decisions
// ---------------------------------------------------------------------------

enum Outcome {
    Ours,
    Theirs,
    Delete,
    Conflict(ConflictReason),
}

fn decide(b: Option<&Entry>, o: Option<&Entry>, t: Option<&Entry>) -> Outcome {
    match (b, o, t) {
        // Neither side has it. Only reachable via the base's key list.
        (_, None, None) => Outcome::Delete,

        // An addition by one side only.
        (None, Some(_), None) => Outcome::Ours,
        (None, None, Some(_)) => Outcome::Theirs,

        // Both added it independently.
        (None, Some(o), Some(t)) => {
            if same(o, t) {
                Outcome::Ours
            } else {
                Outcome::Conflict(ConflictReason::BothEdited)
            }
        }

        // They removed something that was there before.
        (Some(b), Some(o), None) => {
            if same(b, o) {
                Outcome::Delete
            } else {
                Outcome::Conflict(ConflictReason::EditedAndDeleted)
            }
        }

        // We removed something that was there before.
        (Some(b), None, Some(t)) => {
            if same(b, t) {
                Outcome::Delete
            } else {
                Outcome::Conflict(ConflictReason::DeletedAndEdited)
            }
        }

        // Present all round: whoever changed it wins, and if both did they must
        // agree.
        (Some(b), Some(o), Some(t)) => match (!same(b, o), !same(b, t)) {
            (false, false) => Outcome::Ours,
            (true, false) => Outcome::Ours,
            (false, true) => Outcome::Theirs,
            (true, true) => {
                if same(o, t) {
                    Outcome::Ours
                } else {
                    Outcome::Conflict(ConflictReason::BothEdited)
                }
            }
        },
    }
}

/// Which section the task ends up in.
///
/// Decided separately from its content, so a state change by them and a title
/// edit by us both survive. A move by them is taken only when we did not move it
/// ourselves.
fn section_for(b: Option<&Entry>, o: Option<&Entry>, t: Option<&Entry>) -> SectionKind {
    match (b, o, t) {
        (Some(b), Some(o), Some(t)) if o.section == b.section && t.section != b.section => {
            t.section
        }
        (_, Some(o), _) => o.section,
        (_, None, Some(t)) => t.section,
        (Some(b), None, None) => b.section,
        (None, None, None) => SectionKind::Backlog,
    }
}

/// Build the winning task, merging its subtasks against the other side.
///
/// The task's own lines come from `winner`; its subtasks are reconciled in their
/// own right, so an edit to a parent on one side and to its child on the other
/// both land.
fn merged_task(
    winner: &Task,
    base: Option<&Task>,
    other: Option<&Task>,
    conflicts: &mut Vec<Conflict>,
) -> Task {
    let mut out = winner.clone();
    let Some(other) = other else {
        return out;
    };

    let empty: Vec<Task> = Vec::new();
    let base_subs = base.map(|t| &t.subtasks).unwrap_or(&empty);

    // `winner` and `other` may be either side; reconcile is symmetric enough
    // here because the winner's own lines are already chosen and only the
    // subtask lists are being merged.
    let (subs, mut sub_conflicts) =
        reconcile_task_lists(base_subs, &winner.subtasks, &other.subtasks);
    // The parent's own lines can stay verbatim: `serialize_task` recurses into
    // subtasks regardless of the dirty flag, so a changed child is re-emitted
    // without the parent needing to be marked dirty.
    out.subtasks = subs;
    conflicts.append(&mut sub_conflicts);
    out
}

/// The same merge over a flat list of sibling tasks, with no sections involved.
fn reconcile_task_lists(
    base: &[Task],
    ours: &[Task],
    theirs: &[Task],
) -> (Vec<Task>, Vec<Conflict>) {
    let bi = index_tasks(base, SectionKind::Backlog);
    let oi = index_tasks(ours, SectionKind::Backlog);
    let ti = index_tasks(theirs, SectionKind::Backlog);

    let mut keys: Vec<String> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for k in oi
        .order
        .iter()
        .chain(ti.order.iter())
        .chain(bi.order.iter())
    {
        if seen.insert(k.clone()) {
            keys.push(k.clone());
        }
    }

    let mut out = Vec::new();
    let mut conflicts = Vec::new();

    for key in &keys {
        let b = bi.entries.get(key);
        let o = oi.entries.get(key);
        let t = ti.entries.get(key);

        match decide(b, o, t) {
            Outcome::Delete => {}
            Outcome::Ours => {
                if let Some(o) = o {
                    out.push(merged_task(
                        &o.task,
                        b.map(|e| &e.task),
                        t.map(|e| &e.task),
                        &mut conflicts,
                    ));
                }
            }
            Outcome::Theirs => {
                if let Some(t) = t {
                    out.push(merged_task(
                        &t.task,
                        b.map(|e| &e.task),
                        o.map(|e| &e.task),
                        &mut conflicts,
                    ));
                }
            }
            Outcome::Conflict(reason) => match (o, t) {
                (Some(o), t) => {
                    conflicts.push(Conflict {
                        key: key.clone(),
                        reason,
                        theirs: t.map(|e| own_lines(&e.task)).unwrap_or_default(),
                    });
                    out.push(merged_task(
                        &o.task,
                        b.map(|e| &e.task),
                        t.map(|e| &e.task),
                        &mut conflicts,
                    ));
                }
                (None, Some(t)) => out.push(t.task.clone()),
                (None, None) => {}
            },
        }
    }

    (out, conflicts)
}

// ---------------------------------------------------------------------------
// Indexing
// ---------------------------------------------------------------------------

struct Entry {
    section: SectionKind,
    task: Task,
}

struct Index {
    entries: HashMap<String, Entry>,
    /// Keys in the order they appear in the file.
    order: Vec<String>,
}

fn index(track: &Track) -> Index {
    let mut entries = HashMap::new();
    let mut order = Vec::new();
    for node in &track.nodes {
        if let TrackNode::Section { kind, tasks, .. } = node {
            for task in tasks {
                let key = task_key(task);
                if entries
                    .insert(
                        key.clone(),
                        Entry {
                            section: *kind,
                            task: task.clone(),
                        },
                    )
                    .is_none()
                {
                    order.push(key);
                }
            }
        }
    }
    Index { entries, order }
}

fn index_tasks(tasks: &[Task], section: SectionKind) -> Index {
    let mut entries = HashMap::new();
    let mut order = Vec::new();
    for task in tasks {
        let key = task_key(task);
        if entries
            .insert(
                key.clone(),
                Entry {
                    section,
                    task: task.clone(),
                },
            )
            .is_none()
        {
            order.push(key);
        }
    }
    Index { entries, order }
}

/// A task's identity across sides: its ID, or its title when it has none.
///
/// The `#` prefix keeps an untitled-but-IDed task from ever colliding with a
/// title that happens to look like an ID.
pub fn task_key(task: &Task) -> String {
    match &task.id {
        Some(id) => format!("#{id}"),
        None => format!("~{}", task.title),
    }
}

/// Title-keyed tasks whose title is not unique on some side.
///
/// Without an ID there is nothing else to match on, so a repeated title makes
/// identity a guess. The merge declines those rather than pairing the wrong two.
fn ambiguous_keys(tracks: &[&Track]) -> std::collections::HashSet<String> {
    let mut ambiguous = std::collections::HashSet::new();
    for track in tracks {
        let mut counts: HashMap<String, usize> = HashMap::new();
        for node in &track.nodes {
            if let TrackNode::Section { tasks, .. } = node {
                for task in tasks {
                    if task.id.is_none() {
                        *counts.entry(task_key(task)).or_default() += 1;
                    }
                }
            }
        }
        for (key, n) in counts {
            if n > 1 {
                ambiguous.insert(key);
            }
        }
    }
    ambiguous
}

/// A task's own markdown lines, excluding subtasks.
fn own_lines(task: &Task) -> Vec<String> {
    let mut bare = task.clone();
    bare.subtasks.clear();
    crate::parse::serialize_tasks(std::slice::from_ref(&bare), 0)
}

/// Whether two entries are the same task in the same place.
///
/// Compares the semantic fields rather than serialized text: a task that has
/// been through a canonical rewrite is not a change, and comparing text would
/// report it as one.
fn same(a: &Entry, b: &Entry) -> bool {
    a.section == b.section && same_content(&a.task, &b.task)
}

fn same_content(a: &Task, b: &Task) -> bool {
    a.state == b.state
        && a.title == b.title
        && a.tags == b.tags
        && a.metadata == b.metadata
        && a.leading_lines == b.leading_lines
        && a.id.as_ref().map(|i| i.to_string()) == b.id.as_ref().map(|i| i.to_string())
}

// ---------------------------------------------------------------------------
// Rebuilding
// ---------------------------------------------------------------------------

/// Put the resolved tasks back into a track, using ours for structure.
///
/// Ours supplies the headers, literal blocks and section order, because it is
/// the version the user is looking at. A section that exists only in theirs is
/// appended, so a task they moved into a section we do not have still lands.
fn rebuild(ours: &Track, theirs: &Track, resolved: Vec<(SectionKind, Task)>) -> Track {
    let mut track = ours.clone();

    let mut by_section: HashMap<SectionKind, Vec<Task>> = HashMap::new();
    for (kind, task) in resolved {
        by_section.entry(kind).or_default().push(task);
    }

    // Any section holding resolved tasks must exist before it can be filled.
    for kind in [SectionKind::Backlog, SectionKind::Parked, SectionKind::Done] {
        if by_section.contains_key(&kind) && track.section_tasks_mut(kind).is_none() {
            // Prefer their header lines when we never had this section.
            track.ensure_section(kind);
            if let Some(their_header) = section_header(theirs, kind)
                && let Some(node) = track
                    .nodes
                    .iter_mut()
                    .find(|n| matches!(n, TrackNode::Section { kind: k, .. } if *k == kind))
                && let TrackNode::Section { header_lines, .. } = node
            {
                *header_lines = their_header;
            }
        }
    }

    for node in &mut track.nodes {
        if let TrackNode::Section { kind, tasks, .. } = node {
            *tasks = by_section.remove(kind).unwrap_or_default();
        }
    }

    track
}

fn section_header(track: &Track, kind: SectionKind) -> Option<Vec<String>> {
    track.nodes.iter().find_map(|n| match n {
        TrackNode::Section {
            kind: k,
            header_lines,
            ..
        } if *k == kind => Some(header_lines.clone()),
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::{parse_track, serialize_track};

    fn t(text: &str) -> Track {
        parse_track(text)
    }

    const BASE: &str = "\
# A

## Backlog

- [ ] `A-001` One
- [ ] `A-002` Two

## Done
";

    /// The case the whole merge exists for: an agent adds a task while the TUI
    /// edits a different one. Neither side should have to lose.
    #[test]
    fn independent_additions_both_survive() {
        let base = t(BASE);
        let ours = t(
            "# A\n\n## Backlog\n\n- [ ] `A-001` One, edited here\n- [ ] `A-002` Two\n\n## Done\n",
        );
        let theirs = t(
            "# A\n\n## Backlog\n\n- [ ] `A-001` One\n- [ ] `A-002` Two\n- [ ] `A-003` Three\n\n## Done\n",
        );

        let r = reconcile_track(&base, &ours, &theirs);
        let out = serialize_track(&r.track);

        assert!(out.contains("One, edited here"), "our edit: {out}");
        assert!(out.contains("A-003` Three"), "their addition: {out}");
        assert!(r.conflicts.is_empty(), "{:?}", r.conflicts);
    }

    /// A state change by them to a task we did not touch is taken, including the
    /// section move that goes with marking something done.
    #[test]
    fn their_state_change_to_an_untouched_task_is_taken() {
        let base = t(BASE);
        let ours = t(
            "# A\n\n## Backlog\n\n- [ ] `A-001` One, edited here\n- [ ] `A-002` Two\n\n## Done\n",
        );
        let theirs = t("# A\n\n## Backlog\n\n- [ ] `A-001` One\n\n## Done\n\n- [x] `A-002` Two\n");

        let r = reconcile_track(&base, &ours, &theirs);
        let out = serialize_track(&r.track);

        assert!(out.contains("One, edited here"), "{out}");
        let done = r.track.done();
        assert_eq!(done.len(), 1, "their move to Done should land: {out}");
        assert_eq!(done[0].title, "Two");
        assert!(r.conflicts.is_empty(), "{:?}", r.conflicts);
    }

    #[test]
    fn our_edit_wins_over_an_untouched_task_on_their_side() {
        let base = t(BASE);
        let ours = t("# A\n\n## Backlog\n\n- [ ] `A-001` Ours\n- [ ] `A-002` Two\n\n## Done\n");
        let theirs = t(BASE);

        let r = reconcile_track(&base, &ours, &theirs);
        assert!(serialize_track(&r.track).contains("Ours"));
        assert_eq!(r.took_theirs, 0);
    }

    /// Both edited the same task differently — the one case with no right
    /// answer. Ours is kept and theirs is handed back for the recovery log.
    #[test]
    fn a_genuine_conflict_keeps_ours_and_reports_theirs() {
        let base = t(BASE);
        let ours = t("# A\n\n## Backlog\n\n- [ ] `A-001` Ours\n- [ ] `A-002` Two\n\n## Done\n");
        let theirs = t("# A\n\n## Backlog\n\n- [ ] `A-001` Theirs\n- [ ] `A-002` Two\n\n## Done\n");

        let r = reconcile_track(&base, &ours, &theirs);
        assert!(serialize_track(&r.track).contains("Ours"));
        assert_eq!(r.conflicts.len(), 1);
        assert_eq!(r.conflicts[0].reason, ConflictReason::BothEdited);
        assert!(
            r.conflicts[0].theirs.join("\n").contains("Theirs"),
            "their version must be preserved for the log: {:?}",
            r.conflicts[0].theirs
        );
    }

    #[test]
    fn a_deletion_both_sides_agree_on_is_applied() {
        let base = t(BASE);
        let ours = t(BASE);
        let theirs = t("# A\n\n## Backlog\n\n- [ ] `A-001` One\n\n## Done\n");

        let r = reconcile_track(&base, &ours, &theirs);
        let out = serialize_track(&r.track);
        assert!(!out.contains("A-002"), "their deletion should apply: {out}");
        assert_eq!(r.deleted, 1);
    }

    /// A delete is trivially repeatable; an edit is not. So an edit beats a
    /// delete, in both directions — kept as ours here.
    #[test]
    fn our_edit_beats_their_delete() {
        let base = t(BASE);
        let ours =
            t("# A\n\n## Backlog\n\n- [ ] `A-001` One\n- [ ] `A-002` Two, edited\n\n## Done\n");
        let theirs = t("# A\n\n## Backlog\n\n- [ ] `A-001` One\n\n## Done\n");

        let r = reconcile_track(&base, &ours, &theirs);
        let out = serialize_track(&r.track);
        assert!(out.contains("Two, edited"), "{out}");
        assert_eq!(r.conflicts[0].reason, ConflictReason::EditedAndDeleted);
    }

    #[test]
    fn their_edit_beats_our_delete() {
        let base = t(BASE);
        let ours = t("# A\n\n## Backlog\n\n- [ ] `A-001` One\n\n## Done\n");
        let theirs =
            t("# A\n\n## Backlog\n\n- [ ] `A-001` One\n- [ ] `A-002` Two, edited\n\n## Done\n");

        let r = reconcile_track(&base, &ours, &theirs);
        let out = serialize_track(&r.track);
        assert!(out.contains("Two, edited"), "{out}");
    }

    /// Subtasks merge in their own right, so a parent edit on one side and a
    /// child edit on the other both land.
    #[test]
    fn subtasks_merge_independently_of_their_parent() {
        let base =
            t("# A\n\n## Backlog\n\n- [ ] `A-001` Parent\n  - [ ] `A-001.1` Child\n\n## Done\n");
        let ours = t(
            "# A\n\n## Backlog\n\n- [ ] `A-001` Parent, ours\n  - [ ] `A-001.1` Child\n\n## Done\n",
        );
        let theirs = t(
            "# A\n\n## Backlog\n\n- [ ] `A-001` Parent\n  - [ ] `A-001.1` Child\n  - [ ] `A-001.2` Second child\n\n## Done\n",
        );

        let r = reconcile_track(&base, &ours, &theirs);
        let out = serialize_track(&r.track);
        assert!(out.contains("Parent, ours"), "{out}");
        assert!(out.contains("A-001.2` Second child"), "{out}");
        assert!(r.conflicts.is_empty(), "{:?}", r.conflicts);
    }

    /// Merging a side against itself must be a no-op, whatever is in it —
    /// otherwise every reload would rewrite the file.
    #[test]
    fn merging_identical_sides_changes_nothing() {
        let base = t(BASE);
        let r = reconcile_track(&base, &base.clone(), &base.clone());
        assert_eq!(serialize_track(&r.track), serialize_track(&base));
        assert!(!r.changed_anything());
        assert!(r.conflicts.is_empty());
    }

    /// Taking their whole file when we changed nothing must reproduce it
    /// exactly — the ordinary reload case, expressed as a merge.
    #[test]
    fn taking_their_side_wholesale_reproduces_it() {
        let base = t(BASE);
        let theirs = t(
            "# A\n\n## Backlog\n\n- [ ] `A-001` One\n- [ ] `A-002` Two\n- [ ] `A-003` Three\n\n## Done\n",
        );
        let r = reconcile_track(&base, &base.clone(), &theirs);
        assert_eq!(serialize_track(&r.track), serialize_track(&theirs));
    }

    /// Without an ID there is nothing to match on but the title, so a repeated
    /// title is declined rather than paired up wrongly.
    #[test]
    fn a_repeated_untitled_task_is_not_guessed_at() {
        let base = t("# A\n\n## Backlog\n\n- [ ] Same\n- [ ] Same\n\n## Done\n");
        let ours = t("# A\n\n## Backlog\n\n- [ ] Same\n- [ ] Same\n\n## Done\n");
        let theirs = t("# A\n\n## Backlog\n\n- [ ] Same\n- [ ] Different\n\n## Done\n");

        let r = reconcile_track(&base, &ours, &theirs);
        // Ours is kept intact rather than half-merged into an unrecognisable
        // shape.
        assert!(serialize_track(&r.track).contains("Same"));
    }

    // ---- Inbox -----------------------------------------------------------

    fn ib(text: &str) -> Inbox {
        crate::parse::parse_inbox(text).0
    }

    fn titles(inbox: &Inbox) -> Vec<&str> {
        inbox.items.iter().map(|i| i.title.as_str()).collect()
    }

    const INBOX_BASE: &str = "# Inbox\n\n- one\n- two\n";

    /// The common case by far: two writers capturing into the inbox at once.
    #[test]
    fn captures_on_both_sides_survive() {
        let base = ib(INBOX_BASE);
        let ours = ib("# Inbox\n\n- one\n- two\n- ours\n");
        let theirs = ib("# Inbox\n\n- one\n- two\n- theirs\n");

        let r = reconcile_inbox(&base, &ours, &theirs);
        assert_eq!(titles(&r.inbox), vec!["one", "two", "ours", "theirs"]);
        assert_eq!(r.took_theirs, 1);
    }

    /// Triage on the other side removes an item; we should not resurrect it.
    #[test]
    fn their_removal_is_applied() {
        let base = ib(INBOX_BASE);
        let ours = ib(INBOX_BASE);
        let theirs = ib("# Inbox\n\n- one\n");

        let r = reconcile_inbox(&base, &ours, &theirs);
        assert_eq!(titles(&r.inbox), vec!["one"]);
        assert_eq!(r.deleted, 1);
    }

    #[test]
    fn our_removal_is_kept_when_they_did_not_touch_it() {
        let base = ib(INBOX_BASE);
        let ours = ib("# Inbox\n\n- one\n");
        let theirs = ib(INBOX_BASE);

        let r = reconcile_inbox(&base, &ours, &theirs);
        assert_eq!(titles(&r.inbox), vec!["one"]);
    }

    /// An edit is a removal plus a capture, and reads correctly as one: the old
    /// text falls out on our side, the new text is new, and their untouched copy
    /// does not drag the original back.
    #[test]
    fn our_edit_does_not_resurrect_the_original() {
        let base = ib(INBOX_BASE);
        let ours = ib("# Inbox\n\n- one, edited\n- two\n");
        let theirs = ib(INBOX_BASE);

        let r = reconcile_inbox(&base, &ours, &theirs);
        assert_eq!(titles(&r.inbox), vec!["one, edited", "two"]);
    }

    #[test]
    fn their_edit_is_taken() {
        let base = ib(INBOX_BASE);
        let ours = ib(INBOX_BASE);
        let theirs = ib("# Inbox\n\n- one, theirs\n- two\n");

        let r = reconcile_inbox(&base, &ours, &theirs);
        assert_eq!(titles(&r.inbox), vec!["two", "one, theirs"]);
    }

    /// Both edited the same item. Keeping both is the deliberate answer: a
    /// duplicate in a capture list costs one triage keystroke, a dropped capture
    /// is a thought the user cannot get back.
    #[test]
    fn a_double_edit_keeps_both_rather_than_choosing() {
        let base = ib(INBOX_BASE);
        let ours = ib("# Inbox\n\n- one, ours\n- two\n");
        let theirs = ib("# Inbox\n\n- one, theirs\n- two\n");

        let r = reconcile_inbox(&base, &ours, &theirs);
        let t = titles(&r.inbox);
        assert!(t.contains(&"one, ours"), "{t:?}");
        assert!(t.contains(&"one, theirs"), "{t:?}");
    }

    /// Genuinely duplicated captures are counted, not collapsed — otherwise
    /// merging would silently delete one of two identical notes.
    #[test]
    fn identical_items_are_counted_not_deduplicated() {
        let base = ib("# Inbox\n\n- same\n");
        let ours = ib("# Inbox\n\n- same\n");
        let theirs = ib("# Inbox\n\n- same\n- same\n");

        let r = reconcile_inbox(&base, &ours, &theirs);
        assert_eq!(
            titles(&r.inbox),
            vec!["same", "same"],
            "their capture lands"
        );
    }

    /// Tags and body are part of an item's identity, so changing either is an
    /// edit rather than a coincidental match.
    #[test]
    fn tags_distinguish_two_items_with_the_same_title() {
        let base = ib("# Inbox\n\n- note #a\n");
        let ours = ib("# Inbox\n\n- note #a\n");
        let theirs = ib("# Inbox\n\n- note #b\n");

        let r = reconcile_inbox(&base, &ours, &theirs);
        assert_eq!(r.inbox.items.len(), 1);
        assert_eq!(r.inbox.items[0].tags, vec!["b".to_string()]);
    }

    #[test]
    fn merging_identical_inboxes_changes_nothing() {
        let base = ib(INBOX_BASE);
        let r = reconcile_inbox(&base, &base.clone(), &base.clone());
        assert_eq!(titles(&r.inbox), vec!["one", "two"]);
        assert!(!r.changed_anything());
    }

    #[test]
    fn an_inbox_we_did_not_touch_takes_their_side_wholesale() {
        let base = ib(INBOX_BASE);
        let theirs = ib("# Inbox\n\n- one\n- two\n- three\n");
        let r = reconcile_inbox(&base, &base.clone(), &theirs);
        assert_eq!(titles(&r.inbox), vec!["one", "two", "three"]);
    }

    #[test]
    fn a_task_only_we_have_is_kept() {
        let base = t(BASE);
        let ours = t(
            "# A\n\n## Backlog\n\n- [ ] `A-001` One\n- [ ] `A-002` Two\n- [ ] `A-009` Ours only\n\n## Done\n",
        );
        let theirs = t(BASE);

        let r = reconcile_track(&base, &ours, &theirs);
        assert!(serialize_track(&r.track).contains("Ours only"));
    }
}
