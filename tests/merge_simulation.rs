//! Phase 5b — K-actor merge-simulation property test (the capstone).
//!
//! This test directly reproduces the original bug the actor-token namespace
//! feature exists to prevent: multiple independent working copies, each minting
//! task ids while unsynced, whose results are later merged. It asserts that the
//! union of their additions **never contains a duplicate id, every id is
//! well-formed, every (track, namespace) is densely sequenced, and no actor's
//! mint leaked into another namespace.**
//!
//! Where Phases 1–5a pinned each primitive in isolation, this verifies the
//! *emergent* property — that independent namespaced minting composes into a
//! collision-free whole — over many randomized op-sequences.
//!
//! ## What "merge" means here
//!
//! Real divergent clones, when merged, produce a single track file whose task
//! list is the union of what each clone added to its own namespace on top of a
//! common ancestor. We model that without git, entirely in-memory against the
//! real ops layer:
//!
//! 1. Build a shared **ancestor** track (empty, all-null, or mixed namespaces).
//! 2. For each of K actors with **distinct** tokens (one optionally `null`),
//!    clone the ancestor into an **isolated view** and apply a randomized
//!    sequence of mint ops using *that actor's* token namespace. An actor's
//!    max-scan therefore sees only the ancestor + its own additions — never the
//!    other actors' concurrent mints. That isolation is the whole point: if the
//!    harness shared one mutable track it would serialize the actors and miss
//!    the bug class.
//! 3. **Merge** = graft every actor's additions back onto the ancestor.
//! 4. Assert the four invariants on the merged result.
//!
//! Mints go through the real `add_task` / `add_subtask` ops functions, not a
//! reimplementation, so the test exercises the production scan/mint code.

use std::collections::{HashMap, HashSet};

use frame::model::{SectionKind, Task, TaskId, Token, Track};
use frame::ops::task_ops::{InsertPosition, add_subtask, add_task};
use frame::parse::parse_track;
use proptest::prelude::*;
use proptest::sample::{select, subsequence};

/// All actors mint under this single track prefix.
const PREFIX: &str = "EFF";
/// The pool of candidate single-letter tokens. Larger than the max actor count
/// so a distinct subset always exists.
const POOL: [&str; 6] = ["a", "b", "c", "d", "e", "f"];

// ---------------------------------------------------------------------------
// Generated plan
// ---------------------------------------------------------------------------

/// One op applied while building the shared ancestor. The ancestor may use many
/// namespaces (it is shared history), so each op carries its own token.
#[derive(Debug, Clone)]
enum AncestorOp {
    /// Mint a top-level task in the given namespace (`None` = null).
    Top(Option<Token>),
    /// Mint a subtask under the `sel`-th addable parent in the given namespace.
    Sub(usize, Option<Token>),
}

/// One op applied in a single actor's isolated view. The actor's token is fixed
/// for the whole sequence, so ops don't carry it.
#[derive(Debug, Clone)]
enum ActorOp {
    /// Mint a top-level task in the actor's namespace.
    Top,
    /// Mint a subtask under the `sel`-th addable parent in the actor's view.
    Sub(usize),
}

#[derive(Debug, Clone)]
struct Plan {
    ancestor_ops: Vec<AncestorOp>,
    /// `(actor token, that actor's op sequence)`. Tokens are pairwise distinct;
    /// at most one is `null` (`None`).
    actors: Vec<(Option<Token>, Vec<ActorOp>)>,
}

/// A namespace: `None` (null) or one pooled token, drawn with null rarer than
/// tokened so most mints exercise the tokened paths.
fn arb_ns() -> impl Strategy<Value = Option<Token>> {
    prop_oneof![
        1 => Just(None),
        4 => select(POOL.to_vec()).prop_map(|s| Some(Token::new(s).unwrap())),
    ]
}

fn arb_ancestor_op() -> impl Strategy<Value = AncestorOp> {
    prop_oneof![
        arb_ns().prop_map(AncestorOp::Top),
        (0usize..1000, arb_ns()).prop_map(|(sel, ns)| AncestorOp::Sub(sel, ns)),
    ]
}

fn arb_actor_op() -> impl Strategy<Value = ActorOp> {
    prop_oneof![
        2 => Just(ActorOp::Top),
        3 => (0usize..1000).prop_map(ActorOp::Sub),
    ]
}

fn arb_plan() -> impl Strategy<Value = Plan> {
    (2usize..=5usize)
        .prop_flat_map(|k| {
            (
                // Exactly `k` distinct letters → `k` distinct tokened namespaces.
                subsequence(POOL.to_vec(), k..=k),
                // Whether one actor mints in the null namespace instead.
                any::<bool>(),
                prop::collection::vec(arb_ancestor_op(), 0..=8),
                prop::collection::vec(prop::collection::vec(arb_actor_op(), 0..=10), k..=k),
            )
        })
        .prop_map(|(letters, use_null, ancestor_ops, actor_ops)| {
            let mut tokens: Vec<Option<Token>> = letters
                .into_iter()
                .map(|s| Some(Token::new(s).unwrap()))
                .collect();
            if use_null {
                // Demote the first actor to the null namespace. The rest stay
                // tokened and distinct, so all namespaces remain distinct and at
                // most one is null.
                tokens[0] = None;
            }
            let actors = tokens.into_iter().zip(actor_ops).collect();
            Plan {
                ancestor_ops,
                actors,
            }
        })
}

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

/// A record of one id minted through the real ops layer, with the context
/// needed to verify well-formedness, isolation, and dense sequencing.
#[derive(Debug, Clone)]
struct MintRecord {
    id: String,
    /// `None` for a top-level mint, else the parent id the subtask was added to.
    parent: Option<String>,
}

struct ActorResult {
    token: Option<Token>,
    /// The actor's full isolated track after applying its ops.
    track: Track,
    /// Exactly the ids this actor minted, in order.
    minted: Vec<MintRecord>,
}

/// A fresh, empty track with the standard sections.
fn base_track() -> Track {
    parse_track("# Sim\n\n## Backlog\n\n## Done\n")
}

fn collect_task_ids(task: &Task, out: &mut Vec<String>) {
    if let Some(id) = &task.id {
        out.push(id.to_string());
    }
    for sub in &task.subtasks {
        collect_task_ids(sub, out);
    }
}

/// Every id in a track, at every depth, across all sections.
fn collect_ids(track: &Track) -> Vec<String> {
    let mut out = Vec::new();
    for kind in [SectionKind::Backlog, SectionKind::Parked, SectionKind::Done] {
        for task in track.section_tasks(kind) {
            collect_task_ids(task, &mut out);
        }
    }
    out
}

fn collect_parents(task: &Task, out: &mut Vec<(String, usize)>) {
    if task.depth < 2
        && let Some(id) = &task.id
    {
        out.push((id.to_string(), task.depth));
    }
    for sub in &task.subtasks {
        collect_parents(sub, out);
    }
}

/// Tasks that can still take a subtask (depth < 2, respecting the 3-level cap),
/// as `(id, depth)`.
fn addable_parents(track: &Track) -> Vec<(String, usize)> {
    let mut out = Vec::new();
    for kind in [SectionKind::Backlog, SectionKind::Parked, SectionKind::Done] {
        for task in track.section_tasks(kind) {
            collect_parents(task, &mut out);
        }
    }
    out
}

/// Build the shared ancestor by replaying its op sequence through the real ops
/// functions. May use many namespaces.
fn build_ancestor(ops: &[AncestorOp]) -> Track {
    let mut track = base_track();
    for op in ops {
        match op {
            AncestorOp::Top(ns) => {
                add_task(
                    &mut track,
                    "task".into(),
                    InsertPosition::Bottom,
                    PREFIX,
                    ns.as_ref(),
                )
                .expect("ancestor add_task");
            }
            AncestorOp::Sub(sel, ns) => {
                let parents = addable_parents(&track);
                if parents.is_empty() {
                    add_task(
                        &mut track,
                        "task".into(),
                        InsertPosition::Bottom,
                        PREFIX,
                        ns.as_ref(),
                    )
                    .expect("ancestor add_task (sub fallback)");
                } else {
                    let (pid, _) = &parents[sel % parents.len()];
                    add_subtask(&mut track, pid, "task".into(), ns.as_ref())
                        .expect("ancestor add_subtask");
                }
            }
        }
    }
    track
}

/// Apply one actor's ops to its own isolated clone of the ancestor, minting only
/// in the actor's namespace. Records every minted id.
fn apply_actor(ancestor: &Track, token: &Option<Token>, ops: &[ActorOp]) -> ActorResult {
    let mut track = ancestor.clone();
    let mut addable = addable_parents(&track);
    let mut minted = Vec::new();
    for op in ops {
        match op {
            ActorOp::Top => {
                let id = add_task(
                    &mut track,
                    "task".into(),
                    InsertPosition::Bottom,
                    PREFIX,
                    token.as_ref(),
                )
                .expect("actor add_task");
                addable.push((id.clone(), 0));
                minted.push(MintRecord { id, parent: None });
            }
            ActorOp::Sub(sel) => {
                if addable.is_empty() {
                    // No parent to attach to yet — fall back to a top-level mint
                    // so the op still exercises the mint path.
                    let id = add_task(
                        &mut track,
                        "task".into(),
                        InsertPosition::Bottom,
                        PREFIX,
                        token.as_ref(),
                    )
                    .expect("actor add_task (sub fallback)");
                    addable.push((id.clone(), 0));
                    minted.push(MintRecord { id, parent: None });
                } else {
                    let (pid, pdepth) = addable[sel % addable.len()].clone();
                    let id = add_subtask(&mut track, &pid, "task".into(), token.as_ref())
                        .expect("actor add_subtask");
                    if pdepth + 1 < 2 {
                        addable.push((id.clone(), pdepth + 1));
                    }
                    minted.push(MintRecord {
                        id,
                        parent: Some(pid),
                    });
                }
            }
        }
    }
    ActorResult {
        token: token.clone(),
        track,
        minted,
    }
}

/// Graft an actor's new descendants of a shared task onto the merged copy.
fn graft_children(merged: &mut Task, ancestor: &Task, actor: &Task) {
    for actor_child in &actor.subtasks {
        let cid = actor_child.id.as_deref();
        match ancestor.subtasks.iter().find(|t| t.id.as_deref() == cid) {
            Some(anc_child) => {
                if let Some(merged_child) =
                    merged.subtasks.iter_mut().find(|t| t.id.as_deref() == cid)
                {
                    graft_children(merged_child, anc_child, actor_child);
                }
            }
            // A new subtask this actor added — bring its whole subtree across.
            None => merged.subtasks.push(actor_child.clone()),
        }
    }
}

/// Merge = ancestor + the union of every actor's additions. Because the actors'
/// additions are id-disjoint by construction (the property under test), grafting
/// is unambiguous; if a real bug produced a collision, the duplicate node
/// survives here and the no-duplicate assertion catches it.
fn merge(ancestor: &Track, actors: &[ActorResult]) -> Track {
    let mut merged = ancestor.clone();
    for actor in actors {
        for kind in [SectionKind::Backlog, SectionKind::Parked, SectionKind::Done] {
            let actor_tasks: Vec<Task> = actor.track.section_tasks(kind).to_vec();
            let anc_tasks: Vec<Task> = ancestor.section_tasks(kind).to_vec();
            let Some(merged_tasks) = merged.section_tasks_mut(kind) else {
                continue;
            };
            for at in &actor_tasks {
                let aid = at.id.as_deref();
                match anc_tasks.iter().find(|t| t.id.as_deref() == aid) {
                    Some(anc) => {
                        if let Some(m) = merged_tasks.iter_mut().find(|t| t.id.as_deref() == aid) {
                            graft_children(m, anc, at);
                        }
                    }
                    // A new top-level task — append it (and its subtree).
                    None => merged_tasks.push(at.clone()),
                }
            }
        }
    }
    merged
}

/// The largest top-level number recorded in the ancestor for `token`'s
/// namespace (0 if the namespace is fresh).
fn ancestor_top_base(ancestor: &Track, token: Option<&Token>) -> u32 {
    let mut max = 0;
    for kind in [SectionKind::Backlog, SectionKind::Parked, SectionKind::Done] {
        let mut stack: Vec<&Task> = ancestor.section_tasks(kind).iter().collect();
        while let Some(t) = stack.pop() {
            if let Some(id) = &t.id
                && let Some(n) = id.top_level_number(PREFIX, token)
            {
                max = max.max(n);
            }
            stack.extend(t.subtasks.iter());
        }
    }
    max
}

/// The largest direct-child number recorded in the ancestor under `parent` for
/// `token`'s namespace (0 if none — including when the parent is not an ancestor
/// task).
fn ancestor_child_base(ancestor: &Track, parent: &str, token: Option<&Token>) -> u32 {
    let parent_id = TaskId::parse(parent);
    let mut max = 0;
    for kind in [SectionKind::Backlog, SectionKind::Parked, SectionKind::Done] {
        let mut stack: Vec<&Task> = ancestor.section_tasks(kind).iter().collect();
        while let Some(t) = stack.pop() {
            if let Some(id) = &t.id
                && let Some(n) = id.child_number_of(&parent_id, token)
            {
                max = max.max(n);
            }
            stack.extend(t.subtasks.iter());
        }
    }
    max
}

// ---------------------------------------------------------------------------
// The property
// ---------------------------------------------------------------------------

fn check(plan: Plan) -> Result<(), TestCaseError> {
    let ancestor = build_ancestor(&plan.ancestor_ops);
    let actors: Vec<ActorResult> = plan
        .actors
        .iter()
        .map(|(token, ops)| apply_actor(&ancestor, token, ops))
        .collect();

    // --- Teeth / coverage: the case genuinely exercises K >= 2 actors with
    // distinct namespaces minting in isolation. (Without distinct namespaces
    // or without isolated views this property would not hold — see the
    // module-level note and the `same_namespace_isolated_views_collide` test.)
    prop_assert!(actors.len() >= 2, "expected K >= 2 actors");
    let namespaces: HashSet<Option<String>> = actors
        .iter()
        .map(|a| a.token.as_ref().map(|t| t.as_str().to_string()))
        .collect();
    prop_assert_eq!(
        namespaces.len(),
        actors.len(),
        "actor namespaces must be pairwise distinct"
    );

    let ancestor_ids: HashSet<String> = collect_ids(&ancestor).into_iter().collect();

    // --- Harness self-check: the recorded mints are exactly the additions
    // present in each actor's real track (no addition missed, none invented).
    for actor in &actors {
        let additions: HashSet<String> = collect_ids(&actor.track)
            .into_iter()
            .filter(|id| !ancestor_ids.contains(id))
            .collect();
        let recorded: HashSet<String> = actor.minted.iter().map(|m| m.id.clone()).collect();
        prop_assert_eq!(
            additions,
            recorded,
            "recorded mints must equal the actor's track additions"
        );
    }

    let merged = merge(&ancestor, &actors);
    let merged_ids = collect_ids(&merged);

    // --- Invariant 1: no duplicate ids in the merged union (the headline).
    let unique: HashSet<&String> = merged_ids.iter().collect();
    if unique.len() != merged_ids.len() {
        let mut seen = HashSet::new();
        let dups: Vec<&String> = merged_ids.iter().filter(|id| !seen.insert(*id)).collect();
        return Err(TestCaseError::fail(format!(
            "duplicate id(s) in merged result: {dups:?}"
        )));
    }

    // The merged ids are exactly the ancestor ids plus every minted id.
    let mut expected: HashSet<String> = ancestor_ids.clone();
    for actor in &actors {
        for m in &actor.minted {
            expected.insert(m.id.clone());
        }
    }
    prop_assert_eq!(
        &unique
            .iter()
            .map(|s| (*s).clone())
            .collect::<HashSet<String>>(),
        &expected,
        "merged ids must be ancestor ids ∪ minted ids"
    );

    // --- Invariants 2 (well-formed) & 4 (isolation), and gather for 3.
    // Group minted numbers by (parent, namespace). Distinct actor tokens (and at
    // most one null) mean each group belongs to a single actor.
    let mut groups: HashMap<(Option<String>, Option<String>), Vec<u32>> = HashMap::new();
    for actor in &actors {
        let token = actor.token.as_ref();
        let ns_key = actor.token.as_ref().map(|t| t.as_str().to_string());
        for m in &actor.minted {
            let id = TaskId::parse(&m.id);

            // Isolation: the minted id's leaf segment carries this actor's token
            // and nothing leaked into another namespace.
            prop_assert_eq!(
                id.leaf_token(),
                token,
                "namespace leak: {} not in actor namespace",
                m.id
            );

            // Well-formed + correct namespace: a structured id in this namespace
            // yields Some(number); a Raw id, wrong prefix, or wrong namespace
            // would yield None.
            let number = match &m.parent {
                None => {
                    let n = id.top_level_number(PREFIX, token);
                    prop_assert!(
                        n.is_some(),
                        "top-level id not well-formed / out of namespace: {}",
                        m.id
                    );
                    n.unwrap()
                }
                Some(parent) => {
                    let parent_id = TaskId::parse(parent);
                    let n = id.child_number_of(&parent_id, token);
                    prop_assert!(
                        n.is_some(),
                        "child id not well-formed / out of namespace: {} under {}",
                        m.id,
                        parent
                    );
                    n.unwrap()
                }
            };
            prop_assert!(number >= 1, "minted number must be positive: {}", m.id);

            groups
                .entry((m.parent.clone(), ns_key.clone()))
                .or_default()
                .push(number);
        }
    }

    // --- Invariant 3: per-namespace dense sequencing. Within each
    // (parent, namespace) the minted numbers form a gap-free run continuing from
    // the ancestor's max in that namespace. Density is per-namespace only —
    // across actors the merged numbers interleave with no global density, and
    // that is intentionally NOT asserted.
    for ((parent, ns_key), numbers) in &groups {
        let token = ns_key.as_ref().map(|s| Token::new(s.as_str()).unwrap());
        let base = match parent {
            None => ancestor_top_base(&ancestor, token.as_ref()),
            Some(p) => ancestor_child_base(&ancestor, p, token.as_ref()),
        };
        let mut sorted = numbers.clone();
        sorted.sort_unstable();
        let expected: Vec<u32> = (base + 1..=base + numbers.len() as u32).collect();
        prop_assert_eq!(
            sorted,
            expected,
            "non-dense sequence for parent={:?} namespace={:?}",
            parent,
            ns_key
        );
    }

    Ok(())
}

proptest! {
    /// K independent actors mint in isolated namespaced views; their merged
    /// union is collision-free, well-formed, namespace-isolated, and densely
    /// sequenced per namespace.
    #[test]
    fn merged_minting_preserves_invariants(plan in arb_plan()) {
        check(plan)?;
    }
}

/// Teeth check, kept as documentation of *why* the namespacing matters: two
/// actors with **isolated views** but the **same** namespace (the pre-fix
/// behaviour) DO collide. This is the exact bug the per-actor namespace prevents
/// and that `merged_minting_preserves_invariants` guards against. If this ever
/// stops colliding, the property test above has lost its teeth.
#[test]
fn same_namespace_isolated_views_collide() {
    let ancestor = base_track();
    let ops = vec![ActorOp::Top, ActorOp::Top];
    // Both actors mint in the null namespace from isolated clones of the ancestor.
    let a = apply_actor(&ancestor, &None, &ops);
    let b = apply_actor(&ancestor, &None, &ops);

    let a_ids: HashSet<&String> = a.minted.iter().map(|m| &m.id).collect();
    let collision = b.minted.iter().any(|m| a_ids.contains(&m.id));
    assert!(
        collision,
        "two isolated actors in the same namespace must collide — \
         this is the bug the per-actor namespace prevents"
    );
}
