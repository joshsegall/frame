//! Per-working-copy **actor token** infrastructure.
//!
//! A *working copy* (git clone) is the unit of tokening. Each clone holds one
//! token, recorded in the gitignored `frame/.actor` file. The committed
//! `frame/actors.toml` registry maps every known token to its state and
//! provenance, so a fresh clone can see what's already taken and claims land in
//! git history.
//!
//! `null` is a real token (spelled `null`) meaning the empty-token namespace —
//! today's default. Exactly one working copy holds it (the project creator /
//! primary).
//!
//! This module manages token *lifecycle* (claiming, setting, retiring, listing)
//! and resolves the token a mint operation should use ([`resolve_actor_token`],
//! [`id_scope`]); the ID grammar itself lives in [`crate::model::task_id`].
//!
//! This is **not** the global project registry (`src/io/registry.rs`); that maps
//! `~/.config/frame/projects.toml`. This module is per-project.

use std::path::{Path, PathBuf};

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

/// The safe single-character alphabet: `a–z` minus `{i, l, o}` (letters that
/// read as digits). 23 tokens. Hard rule for auto-picked tokens; also the only
/// permitted single-character tokens for manual `set`.
pub const SAFE_ALPHABET: [&str; 23] = [
    "a", "b", "c", "d", "e", "f", "g", "h", "j", "k", "m", "n", "p", "q", "r", "s", "t", "u", "v",
    "w", "x", "y", "z",
];

/// Auto-pick draws uniformly from the first N never-used tokens (alphabetical),
/// scattering claims to cut racing-collision odds.
pub const FRONTIER_WINDOW: usize = 5;

/// At or below this many never-used tokens, surface a thin-frontier notice.
pub const THIN_FRONTIER: usize = 2;

const STATE_ACTIVE: &str = "active";
const STATE_RETIRED: &str = "retired";

/// A registry row: a token's state and provenance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActorEntry {
    /// Provenance — defaults to the claiming machine's hostname.
    pub name: String,
    /// `active` or `retired`.
    pub state: String,
    /// Date the token was first claimed (`YYYY-MM-DD`). Absent for `null`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub claimed: Option<String>,
    /// Date the token was retired (`YYYY-MM-DD`), if tombstoned.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retired: Option<String>,
}

impl ActorEntry {
    pub fn is_active(&self) -> bool {
        self.state == STATE_ACTIVE
    }

    pub fn is_retired(&self) -> bool {
        self.state == STATE_RETIRED
    }
}

/// The token → entry registry, keyed by token. `IndexMap` preserves insertion
/// order for diff-stable, merge-friendly serialization.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ActorRegistry {
    #[serde(default)]
    pub actors: IndexMap<String, ActorEntry>,
}

/// Outcome of a `claim`/`set` so the caller can phrase the right message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClaimOutcome {
    /// A never-used token became a new active row.
    Created,
    /// A retired (tombstoned) token was flipped back to active.
    Reclaimed,
    /// This clone already owns the token — idempotent (name may be updated).
    AlreadyOwned,
}

impl ActorRegistry {
    /// Tokens never present in the registry (active **or** retired): the safe
    /// alphabet minus everything already recorded. Tombstones stay out of the
    /// frontier. Preserves alphabetical order.
    pub fn never_used_frontier(&self) -> Vec<String> {
        SAFE_ALPHABET
            .iter()
            .filter(|t| !self.actors.contains_key(**t))
            .map(|t| t.to_string())
            .collect()
    }

    /// The first `min(FRONTIER_WINDOW, n)` never-used tokens — the pool auto-pick
    /// draws from.
    pub fn frontier_window(&self) -> Vec<String> {
        let mut frontier = self.never_used_frontier();
        frontier.truncate(FRONTIER_WINDOW);
        frontier
    }

    /// `true` when the never-used frontier is nearly empty and explicit `set` is
    /// recommended (random-from-window narrows toward deterministic).
    pub fn is_thin_frontier(&self) -> bool {
        self.never_used_frontier().len() <= THIN_FRONTIER
    }

    /// Auto-pick a token uniformly at random from the frontier window. Returns
    /// `None` only when the never-used frontier is empty.
    pub fn auto_pick(&self) -> Option<String> {
        let window = self.frontier_window();
        if window.is_empty() {
            return None;
        }
        let idx = random_index(window.len());
        Some(window[idx].clone())
    }

    /// `true` if any retired token exists (so the failure message can mention
    /// reclaiming one).
    pub fn has_retired(&self) -> bool {
        self.actors.values().any(|e| e.is_retired())
    }

    /// Claim `token` on behalf of the clone currently holding `current` (the
    /// `.actor` value, `None` if unclaimed). `token` is assumed already
    /// validated. Mutates the registry; returns the outcome or a refusal.
    pub fn claim(
        &mut self,
        token: &str,
        name: &str,
        current: Option<&str>,
        today: &str,
    ) -> Result<ClaimOutcome, String> {
        match self.actors.get(token) {
            None => {
                let claimed = if token == "null" {
                    None
                } else {
                    Some(today.to_string())
                };
                self.actors.insert(
                    token.to_string(),
                    ActorEntry {
                        name: name.to_string(),
                        state: STATE_ACTIVE.to_string(),
                        claimed,
                        retired: None,
                    },
                );
                Ok(ClaimOutcome::Created)
            }
            Some(entry) if entry.is_retired() => {
                let entry = self.actors.get_mut(token).unwrap();
                entry.state = STATE_ACTIVE.to_string();
                entry.retired = None;
                entry.name = name.to_string();
                Ok(ClaimOutcome::Reclaimed)
            }
            Some(entry) => {
                // active
                if current == Some(token) {
                    let entry = self.actors.get_mut(token).unwrap();
                    entry.name = name.to_string();
                    Ok(ClaimOutcome::AlreadyOwned)
                } else {
                    Err(format!(
                        "token '{}' is already claimed by '{}' (active). \
                         Retire it first (`fr actor retire {}`) or choose a different token.",
                        token, entry.name, token
                    ))
                }
            }
        }
    }

    /// Tombstone `token` (active → retired). Errors if absent or already retired.
    pub fn retire(&mut self, token: &str, today: &str) -> Result<(), String> {
        match self.actors.get_mut(token) {
            None => Err(format!("token '{}' is not in the registry", token)),
            Some(entry) if entry.is_retired() => {
                Err(format!("token '{}' is already retired", token))
            }
            Some(entry) => {
                entry.state = STATE_RETIRED.to_string();
                entry.retired = Some(today.to_string());
                Ok(())
            }
        }
    }
}

/// Validate a token. Returns warnings (non-fatal) on success, or an error
/// message on rejection.
///
/// - `null` is always valid.
/// - Uppercase, empty, or non-letter tokens are rejected.
/// - Single-character tokens must be in the 23-token safe alphabet (`i/l/o`
///   excluded — they read as digits).
/// - Multi-character tokens must be all-lowercase; `i/l/o` are permitted but
///   warned about.
pub fn validate_token(token: &str) -> Result<Vec<String>, String> {
    if token == "null" {
        return Ok(Vec::new());
    }
    if token.is_empty() {
        return Err("token cannot be empty".to_string());
    }
    if !token.chars().all(|c| c.is_ascii_alphabetic()) {
        return Err(format!(
            "invalid token '{}' — tokens must be letters only (a–z)",
            token
        ));
    }
    if token.chars().any(|c| c.is_ascii_uppercase()) {
        return Err(format!(
            "invalid token '{}' — tokens must be lowercase",
            token
        ));
    }
    if token.chars().count() == 1 {
        if SAFE_ALPHABET.contains(&token) {
            Ok(Vec::new())
        } else {
            Err(format!(
                "'{}' is not a safe single-char token (i, l, o are excluded because they \
                 read as digits); use a multi-char token like '{}{}' instead",
                token, token, token
            ))
        }
    } else {
        let mut warnings = Vec::new();
        if token.chars().any(|c| matches!(c, 'i' | 'l' | 'o')) {
            warnings.push(format!(
                "token '{}' contains i/l/o, which can be visually confused with digits",
                token
            ));
        }
        Ok(warnings)
    }
}

/// Validate the textual form of a registry (a post-merge-conflict backstop).
/// Reports duplicate token table headers, parse errors, invalid token keys, and
/// invalid states. Returns a (possibly empty) list of human-readable problems.
pub fn validate_registry_text(text: &str) -> Vec<String> {
    let mut issues = Vec::new();

    for dup in duplicate_token_headers(text) {
        issues.push(format!("duplicate token entry: [actors.{}]", dup));
    }

    match toml::from_str::<ActorRegistry>(text) {
        Ok(reg) => {
            for (token, entry) in &reg.actors {
                if validate_token(token).is_err() {
                    issues.push(format!("invalid token key: '{}'", token));
                }
                if entry.state != STATE_ACTIVE && entry.state != STATE_RETIRED {
                    issues.push(format!(
                        "invalid state '{}' for token '{}'",
                        entry.state, token
                    ));
                }
            }
        }
        Err(e) => issues.push(format!("parse error: {}", e)),
    }

    issues
}

/// Find token names that appear in more than one `[actors.<token>]` header.
fn duplicate_token_headers(text: &str) -> Vec<String> {
    let mut counts: IndexMap<String, usize> = IndexMap::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("[actors.")
            && let Some(token) = rest.strip_suffix(']')
        {
            *counts.entry(token.to_string()).or_insert(0) += 1;
        }
    }
    counts
        .into_iter()
        .filter(|(_, n)| *n > 1)
        .map(|(token, _)| token)
        .collect()
}

// ---------------------------------------------------------------------------
// File I/O
// ---------------------------------------------------------------------------

/// Path to the committed registry.
pub fn actors_path(frame_dir: &Path) -> PathBuf {
    frame_dir.join("actors.toml")
}

/// Path to this working copy's *local* gitignored token file (`frame/.actor`).
/// A local token overrides the clone-wide shared one and is how a single
/// worktree deliberately diverges onto its own token.
pub fn actor_token_path(frame_dir: &Path) -> PathBuf {
    frame_dir.join(".actor")
}

/// Path to the clone-wide *shared* token file, under the git common directory
/// (`<common-dir>/frame-actor`). All git worktrees of one clone resolve the same
/// path, so they share one actor identity by default. `None` when the project is
/// not in a git repository — such projects use only the local `.actor`.
pub fn shared_actor_path(frame_dir: &Path) -> Option<PathBuf> {
    crate::io::git::git_common_dir(frame_dir).map(|d| d.join("frame-actor"))
}

/// Where a token write should land.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenScope {
    /// The clone-wide shared file (inherited by every worktree). Falls back to
    /// the local file when the project is not in a git repo.
    Shared,
    /// This working copy's local `frame/.actor` (a per-worktree override).
    Local,
}

fn read_token_file(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let token = text.trim().to_string();
    if token.is_empty() { None } else { Some(token) }
}

/// This working copy's *local* token (`frame/.actor`), ignoring any shared token.
pub fn read_local_actor_token(frame_dir: &Path) -> Option<String> {
    read_token_file(&actor_token_path(frame_dir))
}

/// The clone-wide *shared* token (`<common-dir>/frame-actor`), if any.
pub fn read_shared_actor_token(frame_dir: &Path) -> Option<String> {
    read_token_file(&shared_actor_path(frame_dir)?)
}

/// Read the registry. A missing file is treated as an empty registry (migration
/// tolerance). A present-but-unparseable file is an error so callers don't
/// silently clobber a merge-broken registry.
pub fn read_actors(frame_dir: &Path) -> Result<ActorRegistry, String> {
    let path = actors_path(frame_dir);
    if !path.exists() {
        return Ok(ActorRegistry::default());
    }
    let text = std::fs::read_to_string(&path)
        .map_err(|e| format!("cannot read {}: {}", path.display(), e))?;
    toml::from_str(&text).map_err(|e| format!("cannot parse {}: {}", path.display(), e))
}

/// Write the registry, preserving key order (diff-stable for a committed file).
pub fn write_actors(frame_dir: &Path, registry: &ActorRegistry) -> std::io::Result<()> {
    let path = actors_path(frame_dir);
    let text = toml::to_string_pretty(registry)
        .map_err(|e| std::io::Error::other(format!("serialize actors.toml: {}", e)))?;
    crate::io::recovery::atomic_write(&path, text.as_bytes())
}

/// The outcome of resolving this clone's minting token (see
/// [`resolve_actor_token`]).
#[derive(Debug, Clone)]
pub struct ResolvedToken {
    /// The token string to mint under (`"null"` for the primary/untokened actor,
    /// or a letter/multi-char token).
    pub token: String,
    /// Set to a one-time, loud message when this call auto-claimed a fresh token
    /// (so the caller can announce it once); `None` when `.actor` already existed.
    pub announcement: Option<String>,
}

/// Resolve "my token" for a mint operation, the single hook every minting site
/// calls. The caller must hold the project lock.
///
/// 1. If `frame/.actor` exists, return its token (including `null`).
/// 2. If absent, **auto-claim**: draw from the frontier, write `.actor`, add the
///    registry row, and return a one-time announcement (the "default to a new
///    token" behavior for a fresh clone of an existing project).
/// 3. If absent **and the frontier is empty**, fail with the routing message so
///    the mint can abort without creating anything.
pub fn resolve_actor_token(frame_dir: &Path) -> Result<ResolvedToken, String> {
    if let Some(token) = read_actor_token(frame_dir) {
        // Self-heal a drifted registry. This clone's identity lives in the
        // gitignored `.actor`, but the shared, committed `actors.toml` can lose
        // our row out from under us — a concurrent clone may overwrite the
        // registry without it, or a `git reset`/`restore` can revert an
        // as-yet-uncommitted claim. If our token has no row at all, re-register
        // it so the committed registry reflects reality again. (A *retired* row
        // is left untouched: `fr actor retire` deliberately leaves `.actor`
        // pointing at the tombstone, so resurrecting it here would fight intent.
        // `fr check` reports that case instead.)
        let mut reg = read_actors(frame_dir)?;
        if !reg.actors.contains_key(&token) {
            let name = default_name();
            reg.claim(&token, &name, Some(token.as_str()), &today())?;
            write_actors(frame_dir, &reg)
                .map_err(|e| format!("cannot write actors.toml: {}", e))?;
            return Ok(ResolvedToken {
                token: token.clone(),
                announcement: Some(format!(
                    "re-registered actor token '{}' in actors.toml (registry had drifted)",
                    token
                )),
            });
        }
        return Ok(ResolvedToken {
            token,
            announcement: None,
        });
    }

    // Unclaimed working copy — auto-claim a token on first mint.
    let mut reg = read_actors(frame_dir)?;
    let token = match reg.auto_pick() {
        Some(t) => t,
        None => {
            let hint = if reg.has_retired() {
                "no unused actor tokens remain. Reclaim a retired token with `fr actor set <retired-token>` (see `fr actor list`), or claim a custom multi-char token with `fr actor set <aa|foo|…>`."
            } else {
                "no unused actor tokens remain. Claim a custom multi-char token with `fr actor set <aa|foo|…>`."
            };
            return Err(hint.to_string());
        }
    };

    let name = default_name();
    reg.claim(&token, &name, None, &today())?;
    write_actors(frame_dir, &reg).map_err(|e| format!("cannot write actors.toml: {}", e))?;
    // Auto-claim writes the clone-wide *shared* token (never null), so every
    // other git worktree of this clone inherits it instead of each auto-claiming
    // its own. Falls back to the local file for non-git projects.
    write_actor_token_scoped(frame_dir, &token, TokenScope::Shared)
        .map_err(|e| format!("cannot write actor token: {}", e))?;

    let shared = shared_actor_path(frame_dir).is_some();
    let announcement = Some(if shared {
        format!(
            "Claimed actor token '{}' for this clone (shared across its worktrees)",
            token
        )
    } else {
        format!("Claimed actor token '{}' for this working copy", token)
    });
    Ok(ResolvedToken {
        token,
        announcement,
    })
}

/// What namespace an ID-assigning path should mint in, honoring the strict null
/// policy: the null namespace belongs only to a clone that deliberately took it
/// (`fr init` or an explicit `fr actor set null`), never to a merely-unclaimed
/// clone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdScope {
    /// The clone holds a real namespace (`None` = null) and may mint in it.
    Mint(Option<crate::model::task_id::Token>),
    /// The clone is unclaimed (no `.actor`). Passive paths must **not** mint —
    /// they leave tasks ID-less until an explicit action resolves a token.
    Unclaimed,
}

/// This clone's ID scope **without** claiming anything, read from `.actor`: a
/// present token (including `null`) → [`IdScope::Mint`]; an absent `.actor` →
/// [`IdScope::Unclaimed`]. Used by passive paths (TUI load-time auto-clean,
/// clean previews) that must neither auto-claim a token nor mint null from an
/// unclaimed clone.
pub fn id_scope(frame_dir: &Path) -> IdScope {
    match read_actor_token(frame_dir) {
        Some(t) => IdScope::Mint(crate::model::task_id::actor_namespace(&t)),
        None => IdScope::Unclaimed,
    }
}

/// This working copy's effective token, or `None` if unclaimed. A local
/// `frame/.actor` (a deliberate per-worktree override) wins; otherwise the
/// clone-wide shared token, inherited by every worktree, applies.
pub fn read_actor_token(frame_dir: &Path) -> Option<String> {
    read_local_actor_token(frame_dir).or_else(|| read_shared_actor_token(frame_dir))
}

/// Human-facing label for this clone's actor token, as read by
/// [`read_actor_token`]: `None` (no `.actor`) → `"unclaimed"`, `Some("null")` →
/// `"primary"`, and any other token → the literal token. Used by passive
/// surfaces (`fr info`, the TUI overview header) to show which clone you are on
/// without claiming anything.
pub fn actor_label(token: Option<&str>) -> &str {
    match token {
        None => "unclaimed",
        Some("null") => "primary",
        Some(t) => t,
    }
}

/// Write this clone's token to the local `frame/.actor` file.
pub fn write_actor_token(frame_dir: &Path, token: &str) -> std::io::Result<()> {
    write_actor_token_scoped(frame_dir, token, TokenScope::Local)
}

/// Write a token to the chosen scope. [`TokenScope::Shared`] targets the
/// clone-wide file (so every worktree inherits it) but falls back to the local
/// file when the project is not in a git repo. The null (primary) token must
/// only ever be written [`TokenScope::Local`] — the shared token is always a
/// real letter — but that policy is enforced by callers, not here.
pub fn write_actor_token_scoped(
    frame_dir: &Path,
    token: &str,
    scope: TokenScope,
) -> std::io::Result<()> {
    let path = match scope {
        TokenScope::Shared => {
            shared_actor_path(frame_dir).unwrap_or_else(|| actor_token_path(frame_dir))
        }
        TokenScope::Local => actor_token_path(frame_dir),
    };
    crate::io::recovery::atomic_write(&path, format!("{}\n", token).as_bytes())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// The machine's hostname, the default provenance `name`. Falls back to
/// `"unknown"`.
pub fn default_name() -> String {
    const LEN: usize = 256;
    let mut buf = vec![0u8; LEN];
    let res = unsafe { libc::gethostname(buf.as_mut_ptr() as *mut libc::c_char, LEN) };
    if res != 0 {
        return "unknown".to_string();
    }
    let end = buf.iter().position(|&b| b == 0).unwrap_or(LEN);
    let name = String::from_utf8_lossy(&buf[..end]).trim().to_string();
    if name.is_empty() {
        "unknown".to_string()
    } else {
        name
    }
}

/// Today as `YYYY-MM-DD` (local time), matching task metadata dates.
pub fn today() -> String {
    chrono::Local::now().format("%Y-%m-%d").to_string()
}

/// A process-unique random index in `0..n` (`n` must be > 0). Uses OS-seeded
/// `RandomState` mixed with the current time — enough entropy to scatter
/// concurrent claims without a new dependency.
fn random_index(n: usize) -> usize {
    use std::hash::{BuildHasher, Hasher};
    use std::time::{SystemTime, UNIX_EPOCH};
    let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    hasher.write_u64(nanos);
    (hasher.finish() % n as u64) as usize
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn entry(name: &str, state: &str) -> ActorEntry {
        ActorEntry {
            name: name.to_string(),
            state: state.to_string(),
            claimed: Some("2026-06-20".to_string()),
            retired: if state == STATE_RETIRED {
                Some("2026-06-25".to_string())
            } else {
                None
            },
        }
    }

    fn reg_with(tokens: &[(&str, &str)]) -> ActorRegistry {
        let mut reg = ActorRegistry::default();
        for (tok, state) in tokens {
            reg.actors.insert(tok.to_string(), entry("host", state));
        }
        reg
    }

    // --- Frontier ---

    #[test]
    fn frontier_empty_registry_starts_at_a() {
        let reg = ActorRegistry::default();
        assert_eq!(reg.never_used_frontier().len(), 23);
        assert_eq!(reg.frontier_window(), vec!["a", "b", "c", "d", "e"]);
    }

    #[test]
    fn frontier_excludes_active_and_retired() {
        let reg = reg_with(&[("a", "active"), ("b", "retired")]);
        let frontier = reg.never_used_frontier();
        assert!(!frontier.contains(&"a".to_string()));
        assert!(!frontier.contains(&"b".to_string())); // tombstones stay out
        assert_eq!(reg.frontier_window(), vec!["c", "d", "e", "f", "g"]);
    }

    #[test]
    fn frontier_window_is_min_five() {
        let reg = ActorRegistry::default();
        assert_eq!(reg.frontier_window().len(), 5);
    }

    #[test]
    fn frontier_window_fewer_than_five() {
        // Use up all but two safe tokens.
        let used: Vec<(&str, &str)> = SAFE_ALPHABET[..21].iter().map(|t| (*t, "active")).collect();
        let reg = reg_with(&used);
        let window = reg.frontier_window();
        assert_eq!(window, vec!["y", "z"]);
        assert!(reg.is_thin_frontier());
    }

    #[test]
    fn auto_pick_returns_frontier_member() {
        let reg = reg_with(&[("a", "active")]);
        let frontier = reg.never_used_frontier();
        for _ in 0..50 {
            let pick = reg.auto_pick().unwrap();
            assert!(frontier.contains(&pick), "{pick} not in frontier");
            assert!(reg.frontier_window().contains(&pick));
        }
    }

    #[test]
    fn auto_pick_empty_frontier_with_retired_fails() {
        // All 23 present; some retired → frontier empty, retired exist.
        let mut tokens: Vec<(&str, &str)> = SAFE_ALPHABET.iter().map(|t| (*t, "active")).collect();
        tokens[0].1 = "retired";
        let reg = reg_with(&tokens);
        assert!(reg.never_used_frontier().is_empty());
        assert!(reg.auto_pick().is_none());
        assert!(reg.has_retired());
    }

    #[test]
    fn auto_pick_empty_frontier_all_active_fails() {
        let tokens: Vec<(&str, &str)> = SAFE_ALPHABET.iter().map(|t| (*t, "active")).collect();
        let reg = reg_with(&tokens);
        assert!(reg.auto_pick().is_none());
        assert!(!reg.has_retired());
    }

    // --- Claim / set ---

    #[test]
    fn claim_never_used_creates_active_row() {
        let mut reg = ActorRegistry::default();
        let outcome = reg.claim("a", "laptop", None, "2026-06-27").unwrap();
        assert_eq!(outcome, ClaimOutcome::Created);
        let e = reg.actors.get("a").unwrap();
        assert!(e.is_active());
        assert_eq!(e.name, "laptop");
        assert_eq!(e.claimed.as_deref(), Some("2026-06-27"));
    }

    #[test]
    fn claim_null_has_no_claimed_date() {
        let mut reg = ActorRegistry::default();
        reg.claim("null", "origin", None, "2026-06-27").unwrap();
        let e = reg.actors.get("null").unwrap();
        assert!(e.is_active());
        assert!(e.claimed.is_none());
    }

    #[test]
    fn claim_active_owned_by_another_refused() {
        let mut reg = reg_with(&[("a", "active")]);
        let err = reg.claim("a", "me", None, "2026-06-27").unwrap_err();
        assert!(err.contains("already claimed"));
    }

    #[test]
    fn claim_own_token_idempotent() {
        let mut reg = reg_with(&[("a", "active")]);
        let outcome = reg.claim("a", "newname", Some("a"), "2026-06-27").unwrap();
        assert_eq!(outcome, ClaimOutcome::AlreadyOwned);
        assert_eq!(reg.actors.get("a").unwrap().name, "newname");
    }

    // --- Retire / reclaim ---

    #[test]
    fn retire_tombstones_and_leaves_frontier() {
        let mut reg = reg_with(&[("a", "active")]);
        reg.retire("a", "2026-06-27").unwrap();
        let e = reg.actors.get("a").unwrap();
        assert!(e.is_retired());
        assert_eq!(e.retired.as_deref(), Some("2026-06-27"));
        assert!(!reg.never_used_frontier().contains(&"a".to_string()));
    }

    #[test]
    fn retire_absent_or_already_retired_errors() {
        let mut reg = reg_with(&[("a", "retired")]);
        assert!(reg.retire("z", "2026-06-27").is_err());
        assert!(reg.retire("a", "2026-06-27").is_err());
    }

    #[test]
    fn reclaim_retired_flips_to_active() {
        let mut reg = reg_with(&[("b", "retired")]);
        let outcome = reg.claim("b", "desktop", None, "2026-06-27").unwrap();
        assert_eq!(outcome, ClaimOutcome::Reclaimed);
        let e = reg.actors.get("b").unwrap();
        assert!(e.is_active());
        assert!(e.retired.is_none());
    }

    // --- Validation ---

    #[test]
    fn validate_rejects_uppercase_empty_nonletter() {
        assert!(validate_token("A").is_err());
        assert!(validate_token("").is_err());
        assert!(validate_token("1").is_err());
        assert!(validate_token("a1").is_err());
        assert!(validate_token("a-b").is_err());
    }

    #[test]
    fn validate_single_char_must_be_safe() {
        assert!(validate_token("a").unwrap().is_empty());
        assert!(validate_token("i").is_err());
        assert!(validate_token("l").is_err());
        assert!(validate_token("o").is_err());
    }

    #[test]
    fn validate_multichar_lowercase_ok_with_ilo_warning() {
        // Plain multi-char lowercase with no i/l/o → accepted, no warnings.
        assert!(validate_token("aa").unwrap().is_empty());
        assert!(validate_token("team").unwrap().is_empty());
        // Multi-char containing i/l/o → accepted, but warned.
        assert!(!validate_token("foo").unwrap().is_empty());
        assert!(!validate_token("oil").unwrap().is_empty());
    }

    #[test]
    fn validate_null_ok() {
        assert!(validate_token("null").unwrap().is_empty());
    }

    #[test]
    fn validate_registry_text_reports_duplicates() {
        let text = "\
[actors.null]
name = \"origin\"
state = \"active\"

[actors.a]
name = \"x\"
state = \"active\"

[actors.a]
name = \"y\"
state = \"active\"
";
        let issues = validate_registry_text(text);
        assert!(
            issues
                .iter()
                .any(|i| i.contains("duplicate token entry: [actors.a]")),
            "issues: {issues:?}"
        );
    }

    // --- Registry I/O ---

    #[test]
    fn registry_round_trip_preserves_key_order() {
        let mut reg = ActorRegistry::default();
        reg.claim("null", "origin", None, "2026-06-01").unwrap();
        reg.claim("c", "host-c", None, "2026-06-02").unwrap();
        reg.claim("a", "host-a", None, "2026-06-03").unwrap();

        let tmp = TempDir::new().unwrap();
        let frame_dir = tmp.path().join("frame");
        std::fs::create_dir_all(&frame_dir).unwrap();
        write_actors(&frame_dir, &reg).unwrap();

        let loaded = read_actors(&frame_dir).unwrap();
        let keys: Vec<&String> = loaded.actors.keys().collect();
        assert_eq!(keys, vec!["null", "c", "a"], "key order must be stable");
        assert_eq!(loaded.actors.get("c").unwrap().name, "host-c");
    }

    #[test]
    fn read_missing_registry_is_empty() {
        let tmp = TempDir::new().unwrap();
        let frame_dir = tmp.path().join("frame");
        std::fs::create_dir_all(&frame_dir).unwrap();
        let reg = read_actors(&frame_dir).unwrap();
        assert!(reg.actors.is_empty());
        assert!(!actors_path(&frame_dir).exists());
    }

    #[test]
    fn actor_token_file_round_trip() {
        let tmp = TempDir::new().unwrap();
        let frame_dir = tmp.path().join("frame");
        std::fs::create_dir_all(&frame_dir).unwrap();
        assert!(read_actor_token(&frame_dir).is_none());
        write_actor_token(&frame_dir, "a").unwrap();
        assert_eq!(read_actor_token(&frame_dir).as_deref(), Some("a"));
        write_actor_token(&frame_dir, "null").unwrap();
        assert_eq!(read_actor_token(&frame_dir).as_deref(), Some("null"));
    }

    #[test]
    fn shared_write_falls_back_to_local_without_git() {
        let tmp = TempDir::new().unwrap();
        let frame_dir = tmp.path().join("frame");
        std::fs::create_dir_all(&frame_dir).unwrap();
        // Not a git repo: there is no shared path, so a shared write lands local.
        assert!(shared_actor_path(&frame_dir).is_none());
        write_actor_token_scoped(&frame_dir, "a", TokenScope::Shared).unwrap();
        assert_eq!(read_local_actor_token(&frame_dir).as_deref(), Some("a"));
        assert_eq!(read_actor_token(&frame_dir).as_deref(), Some("a"));
    }

    #[test]
    fn local_token_overrides_shared_in_git_repo() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let git_ok = std::process::Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["init", "-q"])
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if !git_ok {
            return; // git unavailable — skip the git-backed assertions
        }
        let frame_dir = root.join("frame");
        std::fs::create_dir_all(&frame_dir).unwrap();
        let Some(shared) = shared_actor_path(&frame_dir) else {
            return;
        };
        // A shared token is inherited when there is no local override.
        write_actor_token_scoped(&frame_dir, "d", TokenScope::Shared).unwrap();
        assert!(shared.exists());
        assert_eq!(read_shared_actor_token(&frame_dir).as_deref(), Some("d"));
        assert_eq!(read_actor_token(&frame_dir).as_deref(), Some("d"));
        // A local token wins over the shared one.
        write_actor_token_scoped(&frame_dir, "k", TokenScope::Local).unwrap();
        assert_eq!(read_local_actor_token(&frame_dir).as_deref(), Some("k"));
        assert_eq!(read_actor_token(&frame_dir).as_deref(), Some("k"));
    }

    #[test]
    fn default_name_nonempty() {
        assert!(!default_name().is_empty());
    }

    // --- resolve_actor_token (Phase 3 mint hook) ---

    #[test]
    fn resolve_returns_existing_actor_without_claiming() {
        let tmp = TempDir::new().unwrap();
        let frame_dir = tmp.path().join("frame");
        std::fs::create_dir_all(&frame_dir).unwrap();
        write_actor_token(&frame_dir, "null").unwrap();
        // Registry already lists our token — the happy path is a pure no-op.
        write_actors(&frame_dir, &reg_with(&[("null", "active")])).unwrap();

        let resolved = resolve_actor_token(&frame_dir).unwrap();
        assert_eq!(resolved.token, "null");
        assert!(resolved.announcement.is_none());
    }

    #[test]
    fn resolve_self_heals_token_missing_from_registry() {
        let tmp = TempDir::new().unwrap();
        let frame_dir = tmp.path().join("frame");
        std::fs::create_dir_all(&frame_dir).unwrap();
        // This clone holds `null`, but a concurrent clone overwrote the committed
        // registry with only its own token — our row was lost (the real-world
        // drift: a gitignored `.actor` outliving a clobbered `actors.toml`).
        write_actor_token(&frame_dir, "null").unwrap();
        write_actors(&frame_dir, &reg_with(&[("b", "active")])).unwrap();

        let resolved = resolve_actor_token(&frame_dir).unwrap();
        assert_eq!(resolved.token, "null");
        // Healed: announced once, our row is back, and `b` is left intact.
        assert!(resolved.announcement.unwrap().contains("re-registered"));
        let reg = read_actors(&frame_dir).unwrap();
        assert!(reg.actors.get("null").unwrap().is_active());
        assert!(reg.actors.contains_key("b"));
        // `.actor` itself is never disturbed.
        assert_eq!(read_actor_token(&frame_dir).as_deref(), Some("null"));

        // A second resolve is now a no-op — the row is present again.
        let again = resolve_actor_token(&frame_dir).unwrap();
        assert!(again.announcement.is_none());
    }

    #[test]
    fn resolve_does_not_resurrect_a_retired_held_token() {
        let tmp = TempDir::new().unwrap();
        let frame_dir = tmp.path().join("frame");
        std::fs::create_dir_all(&frame_dir).unwrap();
        // `.actor` still points at a token the registry has tombstoned (e.g. the
        // user ran `fr actor retire` on their own token). A mint must NOT silently
        // flip it back to active — that case is `fr check`'s to report.
        write_actor_token(&frame_dir, "a").unwrap();
        write_actors(&frame_dir, &reg_with(&[("a", "retired")])).unwrap();

        let resolved = resolve_actor_token(&frame_dir).unwrap();
        assert_eq!(resolved.token, "a");
        assert!(resolved.announcement.is_none());
        assert!(
            read_actors(&frame_dir)
                .unwrap()
                .actors
                .get("a")
                .unwrap()
                .is_retired()
        );
    }

    #[test]
    fn resolve_auto_claims_when_unclaimed() {
        let tmp = TempDir::new().unwrap();
        let frame_dir = tmp.path().join("frame");
        std::fs::create_dir_all(&frame_dir).unwrap();

        let resolved = resolve_actor_token(&frame_dir).unwrap();
        // A letter token was claimed and announced once.
        assert_ne!(resolved.token, "null");
        assert!(SAFE_ALPHABET.contains(&resolved.token.as_str()));
        assert!(resolved.announcement.unwrap().contains(&resolved.token));
        // `.actor` and the registry row were persisted.
        assert_eq!(
            read_actor_token(&frame_dir).as_deref(),
            Some(resolved.token.as_str())
        );
        let reg = read_actors(&frame_dir).unwrap();
        assert!(reg.actors.get(&resolved.token).unwrap().is_active());

        // A second resolve is now a no-op (no re-announcement).
        let again = resolve_actor_token(&frame_dir).unwrap();
        assert_eq!(again.token, resolved.token);
        assert!(again.announcement.is_none());
    }

    #[test]
    fn resolve_errors_when_frontier_empty_and_unclaimed() {
        let tmp = TempDir::new().unwrap();
        let frame_dir = tmp.path().join("frame");
        std::fs::create_dir_all(&frame_dir).unwrap();
        // Fill the whole safe alphabet (some retired) so the frontier is empty.
        let mut tokens: Vec<(&str, &str)> = SAFE_ALPHABET.iter().map(|t| (*t, "active")).collect();
        tokens[0].1 = "retired";
        write_actors(&frame_dir, &reg_with(&tokens)).unwrap();

        let err = resolve_actor_token(&frame_dir).unwrap_err();
        assert!(err.contains("fr actor set"));
        // Nothing was claimed.
        assert!(read_actor_token(&frame_dir).is_none());
    }

    #[test]
    fn id_scope_distinguishes_unclaimed_from_null() {
        use crate::model::task_id::Token;
        let tmp = TempDir::new().unwrap();
        let frame_dir = tmp.path().join("frame");
        std::fs::create_dir_all(&frame_dir).unwrap();

        // Unclaimed (no `.actor`) is its own state — NOT null.
        assert_eq!(id_scope(&frame_dir), IdScope::Unclaimed);
        // A read-only scope check claims nothing.
        assert!(read_actor_token(&frame_dir).is_none());
        assert!(!actors_path(&frame_dir).exists());

        // The `fr init` creator deliberately holds null.
        write_actor_token(&frame_dir, "null").unwrap();
        assert_eq!(id_scope(&frame_dir), IdScope::Mint(None));
        // Tokened → that token's namespace.
        write_actor_token(&frame_dir, "a").unwrap();
        assert_eq!(id_scope(&frame_dir), IdScope::Mint(Token::new("a")));
    }
}
