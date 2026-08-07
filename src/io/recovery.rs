use std::fmt;
use std::fs::OpenOptions;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::{DateTime, Utc};
use tempfile::NamedTempFile;

use crate::io::lock::FileLock;

/// The lock file guarding recovery-log mutations, for the default location.
/// Never removed — see [`recovery_lock_path`]. Listed in
/// [`crate::io::project_io::LOCAL_ONLY_FRAME_FILES`], which is why it stays a
/// name here even though the lock is now derived from wherever the log resolves
/// to: a lock left in `frame/` must still never be committed.
pub const RECOVERY_LOCK: &str = ".recovery.lock";

/// How long to wait for the recovery-log lock. Short: the critical section is
/// one read plus one rename, and the callers are error paths that must not
/// stall the operation that is already going wrong.
const LOG_LOCK_TIMEOUT: Duration = Duration::from_secs(2);

/// Maximum recovery entries per operation before abort.
pub const BURST_LIMIT: usize = 20;

/// Self-documenting header written at the top of a new recovery log.
const FILE_HEADER: &str = "\
<!-- frame recovery log — append-only error recovery data
     This file captures data that Frame couldn't save normally.
     If something went missing, check here.
     View with: fr recovery
     Prune old entries: fr recovery prune
     Safe to delete if empty or stale. -->

---
";

// ---------------------------------------------------------------------------
// Data types
// ---------------------------------------------------------------------------

/// Category of a recovery entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryCategory {
    Parser,
    Conflict,
    Write,
    Delete,
}

impl fmt::Display for RecoveryCategory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RecoveryCategory::Parser => write!(f, "parser"),
            RecoveryCategory::Conflict => write!(f, "conflict"),
            RecoveryCategory::Write => write!(f, "write"),
            RecoveryCategory::Delete => write!(f, "delete"),
        }
    }
}

impl RecoveryCategory {
    pub fn parse_category(s: &str) -> Option<Self> {
        match s {
            "parser" => Some(RecoveryCategory::Parser),
            "conflict" => Some(RecoveryCategory::Conflict),
            "write" => Some(RecoveryCategory::Write),
            "delete" => Some(RecoveryCategory::Delete),
            _ => None,
        }
    }
}

/// A single entry in the recovery log.
#[derive(Debug, Clone)]
pub struct RecoveryEntry {
    pub timestamp: DateTime<Utc>,
    pub category: RecoveryCategory,
    pub description: String,
    pub fields: Vec<(String, String)>,
    pub body: String,
}

/// Summary info about the recovery log.
#[derive(Debug, Clone)]
pub struct RecoverySummary {
    pub entry_count: usize,
    pub oldest: Option<DateTime<Utc>>,
}

// ---------------------------------------------------------------------------
// Path helper
// ---------------------------------------------------------------------------

/// The environment override for the log's location. Machine-local by nature,
/// which is what an absolute path should be — `project.toml` is committed.
pub const LOG_PATH_ENV: &str = "FRAME_RECOVERY_LOG";

/// Where the recovery log lives and how it is kept.
#[derive(Debug, Clone)]
pub struct RecoverySettings {
    pub log: PathBuf,
    /// Always a sibling of `log` named `<stem>.lock`.
    pub lock: PathBuf,
    pub max_size: u64,
    pub prune_age_days: i64,
}

/// Resolve the log's location and retention for a project.
///
/// Highest precedence first:
///
/// 1. `FRAME_RECOVERY_LOG`
/// 2. `[recovery].path` in `project.toml`, relative paths against the project root
/// 3. `frame/.recovery.log`
///
/// **Reading the config here is deliberately best-effort.** Every caller is on
/// an error path — a failed write, a dropped line, a discarded merge side — and
/// a log that refuses to record anything because `project.toml` is malformed
/// would withhold the evidence exactly when it is most needed. A bad
/// `[recovery]` value is still reported loudly, by `load_project`, on every
/// command that reads the config; this path only has to keep working after that
/// has already gone wrong.
pub fn settings(frame_dir: &Path) -> RecoverySettings {
    let config = read_recovery_config(frame_dir);
    let root = frame_dir.parent().unwrap_or(frame_dir);

    let configured = std::env::var(LOG_PATH_ENV)
        .ok()
        .filter(|v| !v.trim().is_empty())
        .or_else(|| config.path.clone().filter(|v| !v.trim().is_empty()));

    let log = match configured {
        Some(value) => {
            let candidate = PathBuf::from(value.trim());
            if candidate.is_absolute() {
                candidate
            } else {
                root.join(candidate)
            }
        }
        None => default_log_path(frame_dir),
    };

    RecoverySettings {
        lock: lock_beside(&log),
        log,
        max_size: config.max_size.bytes(),
        prune_age_days: config.prune_age_days,
    }
}

/// Where the log lives when nothing overrides it.
fn default_log_path(frame_dir: &Path) -> PathBuf {
    frame_dir.join(".recovery.log")
}

/// The lock guarding mutations of the log.
///
/// A separate, never-removed file, for the reason [`FileLock::acquire_at`]
/// gives and [`crate::io::ids`] already relies on: the log is replaced by
/// `rename(2)`, so locking the log itself would let a waiter hold the lock on
/// an unlinked inode while a newcomer locks the fresh file.
///
/// Derived from the log rather than fixed, because two processes that resolve
/// different logs must not share one lock — that is two writers and one
/// "lock". `.recovery.log` yields `.recovery.lock`, the name this has always
/// used.
fn lock_beside(log: &Path) -> PathBuf {
    log.with_extension("lock")
}

/// The `[recovery]` section, or defaults when it cannot be read.
fn read_recovery_config(frame_dir: &Path) -> crate::model::config::RecoveryConfig {
    #[derive(serde::Deserialize, Default)]
    struct RecoverySection {
        #[serde(default)]
        recovery: crate::model::config::RecoveryConfig,
    }
    std::fs::read_to_string(frame_dir.join("project.toml"))
        .ok()
        .and_then(|text| toml::from_str::<RecoverySection>(&text).ok())
        .unwrap_or_default()
        .recovery
}

/// Return the path to the recovery log file.
pub fn recovery_log_path(frame_dir: &Path) -> PathBuf {
    settings(frame_dir).log
}

/// The lock guarding mutations of the recovery log.
pub fn recovery_lock_path(frame_dir: &Path) -> PathBuf {
    settings(frame_dir).lock
}

/// Take the recovery-log lock, or `None` if it cannot be had in time.
fn lock_log(lock_path: &Path) -> Option<FileLock> {
    FileLock::acquire_at(lock_path, LOG_LOCK_TIMEOUT).ok()
}

// ---------------------------------------------------------------------------
// Atomic file write
// ---------------------------------------------------------------------------

/// Write `content` to `path` atomically using a temp file + rename.
pub fn atomic_write(path: &Path, content: &[u8]) -> io::Result<()> {
    crate::io::fault::maybe_fail(path)?;
    let dir = path.parent().unwrap_or(Path::new("."));
    let mut tmp = NamedTempFile::new_in(dir)?;
    tmp.write_all(content)?;
    tmp.flush()?;
    tmp.persist(path).map_err(|e| e.error)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Entry formatting
// ---------------------------------------------------------------------------

impl RecoveryEntry {
    /// Format this entry as a markdown block for the recovery log.
    fn to_markdown(&self) -> String {
        let mut out = String::new();

        // Header line
        out.push_str(&format!(
            "## {} — {}: {}\n",
            self.timestamp
                .to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            self.category,
            self.description,
        ));
        out.push('\n');

        // Key: value fields
        for (key, value) in &self.fields {
            out.push_str(&format!("{}: {}\n", key, value));
        }

        // Body as fenced code block
        if !self.body.is_empty() {
            out.push('\n');
            out.push_str("```text\n");
            out.push_str(&self.body);
            if !self.body.ends_with('\n') {
                out.push('\n');
            }
            out.push_str("```\n");
        }

        out.push('\n');
        out.push_str("---\n");
        out
    }
}

// ---------------------------------------------------------------------------
// Logging
// ---------------------------------------------------------------------------

/// Append a recovery entry to the log. Errors are swallowed and printed to stderr.
pub fn log_recovery(frame_dir: &Path, entry: RecoveryEntry) {
    if let Err(e) = log_recovery_inner(frame_dir, entry) {
        eprintln!("warning: could not write to recovery log: {}", e);
    }
}

fn log_recovery_inner(frame_dir: &Path, entry: RecoveryEntry) -> io::Result<()> {
    let settings = settings(frame_dir);
    let path = &settings.log;

    // Held across the trim and the append both. The append needs it as much as
    // the trim does: the log is replaced by rename, and an append racing that
    // rename lands on the unlinked inode and is gone. Falling back to an
    // unlocked append is deliberate — see `lock_log`'s callers below.
    let guard = lock_log(&settings.lock);
    if guard.is_none() {
        eprintln!("warning: recovery log is busy; appending without the lock");
    }

    // A configured path may point somewhere that does not exist yet.
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    // Trim while we already hold the lock, so the trim does not take one of its
    // own — `FileLock` is not re-entrant.
    if let Ok(meta) = std::fs::metadata(path)
        && meta.len() > settings.max_size
    {
        trim_locked(path, settings.prune_age_days);
    }

    let needs_header = !path.exists() || std::fs::metadata(path).map_or(true, |m| m.len() == 0);

    let mut file = OpenOptions::new().create(true).append(true).open(path)?;

    if needs_header {
        file.write_all(FILE_HEADER.as_bytes())?;
    }

    let markdown = entry.to_markdown();
    file.write_all(markdown.as_bytes())?;

    Ok(())
}

/// Trim entries older than the cutoff once the log has outgrown its size.
///
/// The caller must hold the recovery-log lock.
///
/// **Size is the trigger, age is the rule.** Outgrowing the limit is only what
/// makes frame look; nothing younger than `prune_age_days` is removed, so a log
/// full of recent entries grows past its limit and loses nothing. That is the
/// right way round — the newest entries are the ones still worth having — and
/// it means the size setting cannot be turned into a data-loss knob by
/// accident.
///
/// Best-effort: a log that cannot be read or rewritten is left as it is, which
/// is the right failure. Trimming is housekeeping, and the alternative to an
/// oversized log is not a smaller one, it is no log at all.
fn trim_locked(path: &Path, prune_age_days: i64) {
    let Ok(content) = std::fs::read_to_string(path) else {
        return;
    };

    let cutoff = Utc::now() - chrono::Duration::days(prune_age_days);
    let trimmed = prune_entries_before(&content, &cutoff);

    if trimmed.len() < content.len() {
        let _ = atomic_write(path, trimmed.as_bytes());
    }
}

/// Log a task deletion to the recovery log.
pub fn log_task_deletion(frame_dir: &Path, task_id: &str, track_id: &str, task_source: &str) {
    log_recovery(
        frame_dir,
        RecoveryEntry {
            timestamp: Utc::now(),
            category: RecoveryCategory::Delete,
            description: format!("task {} deleted", task_id),
            fields: vec![
                ("Task".to_string(), task_id.to_string()),
                ("Track".to_string(), track_id.to_string()),
            ],
            body: task_source.to_string(),
        },
    );
}

// ---------------------------------------------------------------------------
// Reading entries
// ---------------------------------------------------------------------------

/// Read recovery entries from the log file.
pub fn read_recovery_entries(
    frame_dir: &Path,
    limit: Option<usize>,
    since: Option<DateTime<Utc>>,
) -> Vec<RecoveryEntry> {
    read_recovery_listing(frame_dir, limit, since, None).entries
}

/// A page of the recovery log, and how much of it was left out.
///
/// The count exists because a silent truncation is indistinguishable from an
/// empty tail. `fr check` sends people straight to `fr recovery` to find one
/// specific entry, and with the default limit of 10 against an 18-entry log,
/// "the entry is not there" and "the entry is on the next page" look exactly
/// alike — which is how a real diagnosis concluded frame had never written an
/// entry it had.
#[derive(Debug, Clone)]
pub struct RecoveryListing {
    /// The entries to show, most recent first, after `limit` was applied.
    pub entries: Vec<RecoveryEntry>,
    /// How many entries matched the filters *before* `limit` was applied.
    pub matched: usize,
}

impl RecoveryListing {
    /// How many matching entries `limit` held back.
    pub fn hidden(&self) -> usize {
        self.matched.saturating_sub(self.entries.len())
    }
}

/// Read recovery entries, filtered and paged.
///
/// `for_id` selects the entries that mention one task: either a task ID, or the
/// RFC 3339 timestamp a `conflict:` marker carries (`doc/format.md` documents
/// that timestamp as identifying the entry holding the other side's version —
/// this is the tool that follows it).
pub fn read_recovery_listing(
    frame_dir: &Path,
    limit: Option<usize>,
    since: Option<DateTime<Utc>>,
    for_id: Option<&str>,
) -> RecoveryListing {
    let path = recovery_log_path(frame_dir);
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => {
            return RecoveryListing {
                entries: Vec::new(),
                matched: 0,
            };
        }
    };

    let mut entries = parse_entries(&content);

    // Filter by timestamp
    if let Some(since_dt) = since {
        entries.retain(|e| e.timestamp >= since_dt);
    }

    if let Some(needle) = for_id {
        let stamp = DateTime::parse_from_rfc3339(needle)
            .ok()
            .map(|dt| dt.with_timezone(&Utc));
        entries.retain(|e| match stamp {
            Some(ts) => e.timestamp == ts,
            None => entry_names(e, needle),
        });
    }

    let matched = entries.len();

    // Return most recent entries (entries are parsed oldest-first)
    if let Some(n) = limit {
        let skip = entries.len().saturating_sub(n);
        entries = entries.into_iter().skip(skip).collect();
    }

    // Reverse so most recent is first
    entries.reverse();
    RecoveryListing { entries, matched }
}

/// Whether an entry names `id` anywhere — description, a field value, or the
/// body it preserved.
///
/// Matched on ID boundaries, so `BAC-1` does not answer for `BAC-100`. `.` is a
/// boundary character too, which means `BAC-204` does not match `BAC-204.1`
/// either: a lookup driven by a conflict marker names one exact task, and a
/// prefix match would bury it under its own subtree.
pub fn entry_names(entry: &RecoveryEntry, id: &str) -> bool {
    if id.is_empty() {
        return false;
    }
    if mentions(&entry.description, id) || mentions(&entry.body, id) {
        return true;
    }
    entry.fields.iter().any(|(_, value)| mentions(value, id))
}

fn mentions(haystack: &str, id: &str) -> bool {
    let boundary = |c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.');
    let mut from = 0;
    while let Some(offset) = haystack[from..].find(id) {
        let start = from + offset;
        let end = start + id.len();
        let before_ok = haystack[..start].chars().next_back().is_none_or(boundary);
        let after_ok = haystack[end..].chars().next().is_none_or(boundary);
        if before_ok && after_ok {
            return true;
        }
        from = start + 1;
    }
    false
}

/// Get a summary of the recovery log.
pub fn recovery_summary(frame_dir: &Path) -> Option<RecoverySummary> {
    let path = recovery_log_path(frame_dir);
    let content = std::fs::read_to_string(&path).ok()?;
    let entries = parse_entries(&content);
    if entries.is_empty() {
        return None;
    }
    let oldest = entries.first().map(|e| e.timestamp);
    Some(RecoverySummary {
        entry_count: entries.len(),
        oldest,
    })
}

/// Parse all entries from the log content string.
fn parse_entries(content: &str) -> Vec<RecoveryEntry> {
    let mut entries = Vec::new();
    let mut lines = content.lines().peekable();

    while let Some(line) = lines.next() {
        // Look for entry headers: ## <timestamp> — <category>: <description>
        if !line.starts_with("## ") {
            continue;
        }

        let header = &line[3..];
        let entry = if let Some(parsed) = parse_entry_header(header) {
            parsed
        } else {
            continue;
        };

        let mut fields = Vec::new();
        let mut body = String::new();
        let mut in_code_block = false;

        // Parse fields and body
        for line in lines.by_ref() {
            if line == "---" && !in_code_block {
                break;
            }

            if line.starts_with("## ") && !in_code_block {
                // Next entry — we went too far (missing ---).
                // We won't push this line back, just break
                break;
            }

            if in_code_block {
                if line == "```" || line.starts_with("```\n") {
                    in_code_block = false;
                } else {
                    if !body.is_empty() {
                        body.push('\n');
                    }
                    body.push_str(line);
                }
                continue;
            }

            if line.starts_with("```") {
                in_code_block = true;
                continue;
            }

            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }

            // Try to parse as Key: value field
            if let Some(colon) = trimmed.find(": ") {
                let key = &trimmed[..colon];
                let value = &trimmed[colon + 2..];
                fields.push((key.to_string(), value.to_string()));
            }
        }

        entries.push(RecoveryEntry {
            timestamp: entry.0,
            category: entry.1,
            description: entry.2,
            fields,
            body,
        });
    }

    entries
}

/// Parse an entry header: `<timestamp> — <category>: <description>`
fn parse_entry_header(header: &str) -> Option<(DateTime<Utc>, RecoveryCategory, String)> {
    // Split on " — " (em dash with spaces)
    let dash_pos = header.find(" — ")?;
    let timestamp_str = &header[..dash_pos];
    let rest = &header[dash_pos + " — ".len()..];

    let timestamp = DateTime::parse_from_rfc3339(timestamp_str)
        .ok()?
        .with_timezone(&Utc);

    // Split rest on ": "
    let colon_pos = rest.find(": ")?;
    let category_str = &rest[..colon_pos];
    let description = &rest[colon_pos + 2..];

    let category = RecoveryCategory::parse_category(category_str)?;

    Some((timestamp, category, description.to_string()))
}

// ---------------------------------------------------------------------------
// Pruning
// ---------------------------------------------------------------------------

/// Prune entries from the recovery log.
/// Returns the number of entries removed.
pub fn prune_recovery(
    frame_dir: &Path,
    before: Option<DateTime<Utc>>,
    all: bool,
) -> io::Result<usize> {
    let settings = settings(frame_dir);
    let path = settings.log.clone();
    if !path.exists() {
        return Ok(0);
    }

    // Unlike an append, a prune that cannot get the lock simply does not run.
    // The user asked for it and can ask again; there is nothing to lose by
    // waiting, and a great deal to lose by rewriting the log underneath
    // whoever holds it.
    let _guard = lock_log(&settings.lock).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::WouldBlock,
            "recovery log is in use, try again later",
        )
    })?;

    let content = std::fs::read_to_string(&path)?;

    if all {
        let entries = parse_entries(&content);
        let count = entries.len();
        // Header only — but through a rename, so a failure here leaves the log
        // whole rather than truncated.
        atomic_write(&path, FILE_HEADER.as_bytes())?;
        return Ok(count);
    }

    let cutoff =
        before.unwrap_or_else(|| Utc::now() - chrono::Duration::days(settings.prune_age_days));
    let original_entries = parse_entries(&content);
    let original_count = original_entries.len();

    let trimmed = prune_entries_before(&content, &cutoff);
    let new_entries = parse_entries(&trimmed);
    let new_count = new_entries.len();

    atomic_write(&path, trimmed.as_bytes())?;
    Ok(original_count - new_count)

    // Lock released on drop
}

/// Remove entries with timestamps before `cutoff` from the raw content.
/// Preserves the file header.
fn prune_entries_before(content: &str, cutoff: &DateTime<Utc>) -> String {
    let mut result = String::new();
    let mut current_entry = String::new();
    let mut current_timestamp: Option<DateTime<Utc>> = None;
    let mut in_header = true;

    for line in content.lines() {
        // Detect end of file header (first --- after comment block)
        if in_header {
            result.push_str(line);
            result.push('\n');
            if line == "---" {
                in_header = false;
            }
            continue;
        }

        if let Some(stripped) = line.strip_prefix("## ") {
            // Flush previous entry if it passes the cutoff
            if let Some(ts) = current_timestamp
                && ts >= *cutoff
            {
                result.push_str(&current_entry);
            }
            current_entry.clear();
            current_timestamp = parse_entry_header(stripped).map(|(ts, _, _)| ts);
            current_entry.push_str(line);
            current_entry.push('\n');
        } else {
            current_entry.push_str(line);
            current_entry.push('\n');
        }
    }

    // Flush last entry
    if let Some(ts) = current_timestamp
        && ts >= *cutoff
    {
        result.push_str(&current_entry);
    }

    result
}

// ---------------------------------------------------------------------------
// JSON serialization
// ---------------------------------------------------------------------------

impl RecoveryEntry {
    /// Serialize to JSON value for `fr recovery --json`.
    pub fn to_json(&self) -> serde_json::Value {
        let fields: serde_json::Map<String, serde_json::Value> = self
            .fields
            .iter()
            .map(|(k, v)| (k.clone(), serde_json::Value::String(v.clone())))
            .collect();

        serde_json::json!({
            "timestamp": self.timestamp.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            "category": self.category.to_string(),
            "description": self.description,
            "fields": fields,
            "body": self.body,
        })
    }

    /// Format as human-readable raw markdown for display.
    pub fn to_display_markdown(&self) -> String {
        self.to_markdown()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Datelike;
    use tempfile::TempDir;

    fn make_entry(category: RecoveryCategory, desc: &str, body: &str) -> RecoveryEntry {
        RecoveryEntry {
            timestamp: Utc::now(),
            category,
            description: desc.to_string(),
            fields: vec![
                ("Source".to_string(), "tracks/test.md".to_string()),
                ("Context".to_string(), "section \"Backlog\"".to_string()),
            ],
            body: body.to_string(),
        }
    }

    /// An entry with a chosen timestamp, so the `--for <timestamp>` lookup and
    /// the ordering can both be pinned without sleeping.
    fn stamped_entry(secs: i64, desc: &str, body: &str) -> RecoveryEntry {
        RecoveryEntry {
            timestamp: DateTime::from_timestamp(1_700_000_000 + secs, 0).unwrap(),
            category: RecoveryCategory::Delete,
            description: desc.to_string(),
            fields: vec![("Track".to_string(), "main".to_string())],
            body: body.to_string(),
        }
    }

    /// A frame dir with a `project.toml` carrying the given `[recovery]` body.
    fn project_with_recovery(section: &str) -> (TempDir, PathBuf) {
        let tmp = TempDir::new().unwrap();
        let frame_dir = tmp.path().join("frame");
        std::fs::create_dir_all(&frame_dir).unwrap();
        std::fs::write(
            frame_dir.join("project.toml"),
            format!("[project]\nname = \"t\"\n\n[recovery]\n{section}"),
        )
        .unwrap();
        (tmp, frame_dir)
    }

    // -- Where the log lives ------------------------------------------------

    #[test]
    fn without_configuration_the_log_sits_in_the_frame_directory() {
        let (_tmp, frame_dir) = project_with_recovery("");
        let s = settings(&frame_dir);
        assert_eq!(s.log, frame_dir.join(".recovery.log"));
    }

    /// The lock is derived from the log, and the derivation has to reproduce
    /// the name this has always used or existing locks stop serialising.
    #[test]
    fn the_lock_is_named_beside_the_log_it_guards() {
        let (_tmp, frame_dir) = project_with_recovery("");
        let s = settings(&frame_dir);
        assert_eq!(s.lock, frame_dir.join(".recovery.lock"));
        assert_eq!(s.lock.file_name().unwrap(), RECOVERY_LOCK);
        assert_ne!(s.lock, s.log);
    }

    #[test]
    fn a_relative_configured_path_resolves_against_the_project_root() {
        let (tmp, frame_dir) = project_with_recovery("path = \"logs/frame.log\"\n");
        let s = settings(&frame_dir);
        assert_eq!(s.log, tmp.path().join("logs/frame.log"));
        assert_eq!(s.lock, tmp.path().join("logs/frame.lock"));
    }

    /// The escape hatch back to per-working-copy behaviour, spelled the way the
    /// docs spell it.
    #[test]
    fn a_configured_path_can_pin_the_log_to_the_working_copy() {
        let (_tmp, frame_dir) = project_with_recovery("path = \"frame/.recovery.log\"\n");
        let s = settings(&frame_dir);
        assert_eq!(s.log, frame_dir.join(".recovery.log"));
    }

    #[test]
    fn an_absolute_configured_path_is_taken_as_written() {
        let (_tmp, frame_dir) = project_with_recovery("path = \"/tmp/frame-abs.log\"\n");
        assert_eq!(
            settings(&frame_dir).log,
            PathBuf::from("/tmp/frame-abs.log")
        );
    }

    #[test]
    fn an_empty_configured_path_falls_back_rather_than_writing_to_the_root() {
        let (_tmp, frame_dir) = project_with_recovery("path = \"  \"\n");
        assert_eq!(settings(&frame_dir).log, frame_dir.join(".recovery.log"));
    }

    #[test]
    fn a_configured_size_and_age_are_read() {
        let (_tmp, frame_dir) = project_with_recovery("max_size = \"64KB\"\nprune_age_days = 7\n");
        let s = settings(&frame_dir);
        assert_eq!(s.max_size, 64 * 1024);
        assert_eq!(s.prune_age_days, 7);
    }

    /// The log is written from error paths. A `project.toml` too broken to
    /// parse must not also cost us the record of what went wrong.
    #[test]
    fn a_broken_project_toml_falls_back_to_defaults_rather_than_losing_the_log() {
        let tmp = TempDir::new().unwrap();
        let frame_dir = tmp.path().join("frame");
        std::fs::create_dir_all(&frame_dir).unwrap();
        std::fs::write(frame_dir.join("project.toml"), "this is not [ valid toml").unwrap();

        let s = settings(&frame_dir);
        assert_eq!(s.log, frame_dir.join(".recovery.log"));
        assert_eq!(s.max_size, 5 * 1024 * 1024);
        assert_eq!(s.prune_age_days, 30);

        log_recovery(&frame_dir, stamped_entry(1, "still recorded", "body"));
        assert_eq!(read_recovery_entries(&frame_dir, None, None).len(), 1);
    }

    #[test]
    fn a_configured_path_is_where_entries_actually_land() {
        let (tmp, frame_dir) = project_with_recovery("path = \"logs/frame.log\"\n");
        log_recovery(&frame_dir, stamped_entry(1, "written", "body"));

        assert!(tmp.path().join("logs/frame.log").exists(), "parent created");
        assert!(!frame_dir.join(".recovery.log").exists(), "not the default");
        let entries = read_recovery_entries(&frame_dir, None, None);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].description, "written");
    }

    // -- Retention: size triggers, age decides ------------------------------

    /// Fill the log past `max_size` with entries of a chosen age.
    fn fill_log(frame_dir: &Path, count: usize, days_old: i64) {
        for i in 0..count {
            log_recovery(
                frame_dir,
                RecoveryEntry {
                    timestamp: Utc::now() - chrono::Duration::days(days_old),
                    category: RecoveryCategory::Delete,
                    description: format!("entry {i}"),
                    fields: vec![],
                    body: "x".repeat(2000),
                },
            );
        }
    }

    /// The claim that matters most: outgrowing the limit is not itself a reason
    /// to delete anything. A busy week must not cost this week's entries.
    #[test]
    fn an_oversized_log_of_recent_entries_loses_nothing() {
        let (_tmp, frame_dir) = project_with_recovery("max_size = \"8KB\"\n");
        fill_log(&frame_dir, 20, 0);

        let entries = read_recovery_entries(&frame_dir, None, None);
        assert_eq!(
            entries.len(),
            20,
            "size is a trigger for housekeeping, not a cap on recent data"
        );
        assert!(
            std::fs::metadata(&settings(&frame_dir).log).unwrap().len() > 8 * 1024,
            "and the file really did outgrow its limit"
        );
    }

    #[test]
    fn an_oversized_log_drops_only_what_is_older_than_the_age_setting() {
        let (_tmp, frame_dir) = project_with_recovery("max_size = \"8KB\"\nprune_age_days = 30\n");
        fill_log(&frame_dir, 6, 90); // older than the cutoff
        fill_log(&frame_dir, 6, 1); // newer than it

        let descriptions: Vec<String> = read_recovery_entries(&frame_dir, None, None)
            .into_iter()
            .map(|e| e.description)
            .collect();
        assert!(
            descriptions.len() < 12,
            "the trim should have run: {descriptions:?}"
        );
        // Everything that survived is recent; the trim never touches those.
        let recent = read_recovery_entries(&frame_dir, None, None)
            .into_iter()
            .filter(|e| e.timestamp > Utc::now() - chrono::Duration::days(30))
            .count();
        assert_eq!(recent, 6, "every recent entry survives");
    }

    /// Under the limit, age alone changes nothing — the trim never runs.
    #[test]
    fn an_undersized_log_keeps_entries_older_than_the_age_setting() {
        let (_tmp, frame_dir) = project_with_recovery("max_size = \"5MB\"\n");
        fill_log(&frame_dir, 3, 400);
        assert_eq!(read_recovery_entries(&frame_dir, None, None).len(), 3);
    }

    #[test]
    fn a_custom_age_moves_the_cutoff() {
        let (_tmp, frame_dir) = project_with_recovery("max_size = \"8KB\"\nprune_age_days = 365\n");
        fill_log(&frame_dir, 12, 90); // old by the default rule, young by this one

        assert_eq!(
            read_recovery_entries(&frame_dir, None, None).len(),
            12,
            "a 365-day retention keeps 90-day-old entries"
        );
    }

    // -- `--for <ID>`: the lookup a conflict marker sends you to ------------

    #[test]
    fn for_id_matches_the_description_a_field_and_the_body() {
        let tmp = TempDir::new().unwrap();
        let frame_dir = tmp.path().join("frame");
        std::fs::create_dir_all(&frame_dir).unwrap();

        log_recovery(&frame_dir, stamped_entry(1, "task M-001 deleted", "body"));
        log_recovery(&frame_dir, stamped_entry(2, "unrelated", "- [ ] `M-002` x"));
        log_recovery(&frame_dir, {
            let mut e = stamped_entry(3, "unrelated", "body");
            e.fields = vec![("Task".to_string(), "M-003".to_string())];
            e
        });

        for id in ["M-001", "M-002", "M-003"] {
            let listing = read_recovery_listing(&frame_dir, None, None, Some(id));
            assert_eq!(listing.matched, 1, "{id} should match exactly one entry");
        }
    }

    /// The reason the match is boundary-aware: a project long enough to have
    /// `M-100` still has `M-1`, and a lookup for the short one must not drag in
    /// every entry about the long one.
    #[test]
    fn for_id_does_not_match_a_longer_id_that_starts_the_same() {
        let tmp = TempDir::new().unwrap();
        let frame_dir = tmp.path().join("frame");
        std::fs::create_dir_all(&frame_dir).unwrap();

        log_recovery(&frame_dir, stamped_entry(1, "task M-100 deleted", "body"));
        log_recovery(&frame_dir, stamped_entry(2, "task M-1 deleted", "body"));

        let listing = read_recovery_listing(&frame_dir, None, None, Some("M-1"));
        assert_eq!(listing.matched, 1);
        assert_eq!(listing.entries[0].description, "task M-1 deleted");
    }

    /// `.` is a boundary character, so a parent does not answer for its
    /// subtree. A marker names one exact task; a prefix match would bury it.
    #[test]
    fn for_id_does_not_match_a_subtask_of_the_id() {
        let tmp = TempDir::new().unwrap();
        let frame_dir = tmp.path().join("frame");
        std::fs::create_dir_all(&frame_dir).unwrap();

        log_recovery(&frame_dir, stamped_entry(1, "task M-004.1 deleted", "body"));

        assert_eq!(
            read_recovery_listing(&frame_dir, None, None, Some("M-004")).matched,
            0
        );
        assert_eq!(
            read_recovery_listing(&frame_dir, None, None, Some("M-004.1")).matched,
            1
        );
    }

    /// The other half of `--for`: a `conflict:` marker carries a timestamp, and
    /// `doc/format.md` says it identifies the entry. This is what follows it.
    #[test]
    fn for_a_timestamp_selects_the_entry_the_marker_names() {
        let tmp = TempDir::new().unwrap();
        let frame_dir = tmp.path().join("frame");
        std::fs::create_dir_all(&frame_dir).unwrap();

        log_recovery(&frame_dir, stamped_entry(0, "first", "body"));
        log_recovery(&frame_dir, stamped_entry(60, "second", "body"));

        let stamp = DateTime::from_timestamp(1_700_000_060, 0)
            .unwrap()
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let listing = read_recovery_listing(&frame_dir, None, None, Some(&stamp));
        assert_eq!(listing.matched, 1);
        assert_eq!(listing.entries[0].description, "second");
    }

    #[test]
    fn for_an_id_nothing_mentions_matches_nothing() {
        let tmp = TempDir::new().unwrap();
        let frame_dir = tmp.path().join("frame");
        std::fs::create_dir_all(&frame_dir).unwrap();
        log_recovery(&frame_dir, stamped_entry(1, "task M-001 deleted", "body"));

        assert_eq!(
            read_recovery_listing(&frame_dir, None, None, Some("M-999")).matched,
            0
        );
    }

    // -- The count behind the truncation notice -----------------------------

    /// `matched` counts what passed the filters, not what survived the limit —
    /// otherwise the notice could never say how much it was hiding.
    #[test]
    fn matched_counts_before_the_limit_and_hidden_is_the_difference() {
        let tmp = TempDir::new().unwrap();
        let frame_dir = tmp.path().join("frame");
        std::fs::create_dir_all(&frame_dir).unwrap();

        for i in 0..18 {
            log_recovery(&frame_dir, stamped_entry(i, &format!("entry {i}"), "body"));
        }

        let listing = read_recovery_listing(&frame_dir, Some(10), None, None);
        assert_eq!(listing.entries.len(), 10);
        assert_eq!(listing.matched, 18);
        assert_eq!(listing.hidden(), 8);

        // Newest first, and it is the newest ten that are kept.
        assert_eq!(listing.entries[0].description, "entry 17");
        assert_eq!(listing.entries[9].description, "entry 8");
    }

    #[test]
    fn nothing_is_hidden_when_the_limit_exceeds_the_matches() {
        let tmp = TempDir::new().unwrap();
        let frame_dir = tmp.path().join("frame");
        std::fs::create_dir_all(&frame_dir).unwrap();
        for i in 0..3 {
            log_recovery(&frame_dir, stamped_entry(i, &format!("entry {i}"), "body"));
        }

        let listing = read_recovery_listing(&frame_dir, Some(10), None, None);
        assert_eq!(listing.matched, 3);
        assert_eq!(listing.hidden(), 0);
    }

    #[test]
    fn an_absent_log_lists_nothing_rather_than_failing() {
        let tmp = TempDir::new().unwrap();
        let frame_dir = tmp.path().join("frame");
        std::fs::create_dir_all(&frame_dir).unwrap();

        let listing = read_recovery_listing(&frame_dir, Some(10), None, None);
        assert!(listing.entries.is_empty());
        assert_eq!(listing.matched, 0);
        assert_eq!(listing.hidden(), 0);
    }

    #[test]
    fn test_entry_formatting() {
        let entry = make_entry(RecoveryCategory::Parser, "dropped lines", "some content");
        let md = entry.to_markdown();
        assert!(md.contains("## "));
        assert!(md.contains("parser: dropped lines"));
        assert!(md.contains("Source: tracks/test.md"));
        assert!(md.contains("```text"));
        assert!(md.contains("some content"));
        assert!(md.ends_with("---\n"));
    }

    #[test]
    fn test_log_and_read() {
        let tmp = TempDir::new().unwrap();
        let frame_dir = tmp.path().join("frame");
        std::fs::create_dir_all(&frame_dir).unwrap();

        log_recovery(
            &frame_dir,
            make_entry(RecoveryCategory::Parser, "test1", "body1"),
        );
        log_recovery(
            &frame_dir,
            make_entry(RecoveryCategory::Write, "test2", "body2"),
        );

        let entries = read_recovery_entries(&frame_dir, None, None);
        assert_eq!(entries.len(), 2);
        // Most recent first
        assert_eq!(entries[0].description, "test2");
        assert_eq!(entries[1].description, "test1");
    }

    #[test]
    fn test_read_with_limit() {
        let tmp = TempDir::new().unwrap();
        let frame_dir = tmp.path().join("frame");
        std::fs::create_dir_all(&frame_dir).unwrap();

        for i in 0..5 {
            log_recovery(
                &frame_dir,
                make_entry(
                    RecoveryCategory::Parser,
                    &format!("entry{}", i),
                    &format!("body{}", i),
                ),
            );
        }

        let entries = read_recovery_entries(&frame_dir, Some(2), None);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].description, "entry4");
        assert_eq!(entries[1].description, "entry3");
    }

    #[test]
    fn test_prune_all() {
        let tmp = TempDir::new().unwrap();
        let frame_dir = tmp.path().join("frame");
        std::fs::create_dir_all(&frame_dir).unwrap();

        log_recovery(
            &frame_dir,
            make_entry(RecoveryCategory::Parser, "test", "body"),
        );

        let count = prune_recovery(&frame_dir, None, true).unwrap();
        assert_eq!(count, 1);

        let entries = read_recovery_entries(&frame_dir, None, None);
        assert!(entries.is_empty());

        // File should still exist with header
        let path = recovery_log_path(&frame_dir);
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(content.contains("frame recovery log"));
    }

    #[test]
    fn test_recovery_summary() {
        let tmp = TempDir::new().unwrap();
        let frame_dir = tmp.path().join("frame");
        std::fs::create_dir_all(&frame_dir).unwrap();

        assert!(recovery_summary(&frame_dir).is_none());

        log_recovery(
            &frame_dir,
            make_entry(RecoveryCategory::Write, "test", "body"),
        );

        let summary = recovery_summary(&frame_dir).unwrap();
        assert_eq!(summary.entry_count, 1);
        assert!(summary.oldest.is_some());
    }

    #[test]
    fn test_atomic_write() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("test.txt");

        atomic_write(&path, b"hello world").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "hello world");

        // Overwrite
        atomic_write(&path, b"goodbye").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "goodbye");
    }

    #[test]
    fn test_entry_to_json() {
        let entry = make_entry(RecoveryCategory::Parser, "dropped lines", "content");
        let json = entry.to_json();
        assert_eq!(json["category"], "parser");
        assert_eq!(json["description"], "dropped lines");
        assert_eq!(json["body"], "content");
        assert!(json["fields"]["Source"].as_str().is_some());
    }

    #[test]
    fn test_parse_entry_header() {
        let result = parse_entry_header("2026-02-10T14:32:05Z — parser: dropped lines");
        assert!(result.is_some());
        let (ts, cat, desc) = result.unwrap();
        assert_eq!(cat, RecoveryCategory::Parser);
        assert_eq!(desc, "dropped lines");
        assert_eq!(ts.year(), 2026);
    }

    #[test]
    fn test_parse_entry_header_invalid() {
        assert!(parse_entry_header("not a valid header").is_none());
        assert!(parse_entry_header("2026-02-10T14:32:05Z — unknown: desc").is_none());
    }

    #[test]
    fn test_recovery_log_path() {
        let path = recovery_log_path(Path::new("/tmp/frame"));
        assert_eq!(path, PathBuf::from("/tmp/frame/.recovery.log"));
    }

    #[test]
    fn test_empty_body_entry() {
        let entry = RecoveryEntry {
            timestamp: Utc::now(),
            category: RecoveryCategory::Conflict,
            description: "orphaned edit".to_string(),
            fields: vec![("Task".to_string(), "EFF-014".to_string())],
            body: String::new(),
        };
        let md = entry.to_markdown();
        assert!(!md.contains("```"));
        assert!(md.contains("conflict: orphaned edit"));
    }

    #[test]
    fn test_round_trip_parse() {
        let tmp = TempDir::new().unwrap();
        let frame_dir = tmp.path().join("frame");
        std::fs::create_dir_all(&frame_dir).unwrap();

        let original = RecoveryEntry {
            timestamp: Utc::now(),
            category: RecoveryCategory::Write,
            description: "rename failed".to_string(),
            fields: vec![
                ("Target".to_string(), "tracks/effects.md".to_string()),
                ("Error".to_string(), "Permission denied".to_string()),
            ],
            body: "# Effect System\n\n## Backlog\n".to_string(),
        };

        log_recovery(&frame_dir, original.clone());

        let entries = read_recovery_entries(&frame_dir, None, None);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].category, RecoveryCategory::Write);
        assert_eq!(entries[0].description, "rename failed");
        assert_eq!(entries[0].fields.len(), 2);
        assert_eq!(entries[0].body, "# Effect System\n\n## Backlog");
    }

    #[test]
    fn test_file_header_created_on_first_write() {
        let tmp = TempDir::new().unwrap();
        let frame_dir = tmp.path().join("frame");
        std::fs::create_dir_all(&frame_dir).unwrap();

        log_recovery(
            &frame_dir,
            make_entry(RecoveryCategory::Parser, "test", "body"),
        );

        let content = std::fs::read_to_string(recovery_log_path(&frame_dir)).unwrap();
        assert!(content.starts_with("<!-- frame recovery log"));
        assert!(content.contains("---\n"));
    }

    #[test]
    fn test_category_display() {
        assert_eq!(RecoveryCategory::Parser.to_string(), "parser");
        assert_eq!(RecoveryCategory::Conflict.to_string(), "conflict");
        assert_eq!(RecoveryCategory::Write.to_string(), "write");
    }

    #[test]
    fn test_prune_before_cutoff() {
        let tmp = TempDir::new().unwrap();
        let frame_dir = tmp.path().join("frame");
        std::fs::create_dir_all(&frame_dir).unwrap();

        // Create an entry with a timestamp in the past
        let old_entry = RecoveryEntry {
            timestamp: Utc::now() - chrono::Duration::days(60),
            category: RecoveryCategory::Parser,
            description: "old entry".to_string(),
            fields: vec![],
            body: "old content".to_string(),
        };
        log_recovery(&frame_dir, old_entry);

        // Create a recent entry
        let new_entry = RecoveryEntry {
            timestamp: Utc::now(),
            category: RecoveryCategory::Write,
            description: "new entry".to_string(),
            fields: vec![],
            body: "new content".to_string(),
        };
        log_recovery(&frame_dir, new_entry);

        // Prune entries older than 30 days
        let cutoff = Utc::now() - chrono::Duration::days(30);
        let removed = prune_recovery(&frame_dir, Some(cutoff), false).unwrap();
        assert_eq!(removed, 1);

        let entries = read_recovery_entries(&frame_dir, None, None);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].description, "new entry");
    }

    #[test]
    fn test_prune_no_log_file() {
        let tmp = TempDir::new().unwrap();
        let frame_dir = tmp.path().join("frame");
        std::fs::create_dir_all(&frame_dir).unwrap();

        // Prune when no log file exists should return 0
        let removed = prune_recovery(&frame_dir, None, true).unwrap();
        assert_eq!(removed, 0);
    }

    #[test]
    fn test_read_since_filter() {
        let tmp = TempDir::new().unwrap();
        let frame_dir = tmp.path().join("frame");
        std::fs::create_dir_all(&frame_dir).unwrap();

        let old_entry = RecoveryEntry {
            timestamp: Utc::now() - chrono::Duration::days(10),
            category: RecoveryCategory::Parser,
            description: "older".to_string(),
            fields: vec![],
            body: String::new(),
        };
        log_recovery(&frame_dir, old_entry);

        let new_entry = RecoveryEntry {
            timestamp: Utc::now(),
            category: RecoveryCategory::Write,
            description: "newer".to_string(),
            fields: vec![],
            body: String::new(),
        };
        log_recovery(&frame_dir, new_entry);

        // Read entries since 5 days ago — should only get the newer one
        let since = Utc::now() - chrono::Duration::days(5);
        let entries = read_recovery_entries(&frame_dir, None, Some(since));
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].description, "newer");
    }

    #[test]
    fn test_prune_entries_before_preserves_header() {
        let content = format!(
            "{}\n## {} — parser: old\n\nBody\n\n---\n## {} — write: new\n\nBody2\n\n---\n",
            FILE_HEADER,
            (Utc::now() - chrono::Duration::days(60))
                .to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        );

        let cutoff = Utc::now() - chrono::Duration::days(30);
        let trimmed = prune_entries_before(&content, &cutoff);

        // Header should still be present
        assert!(trimmed.contains("frame recovery log"));
        // Old entry should be removed
        assert!(!trimmed.contains("parser: old"));
        // New entry should remain
        assert!(trimmed.contains("write: new"));
    }

    #[test]
    fn test_multiple_fields_round_trip() {
        let tmp = TempDir::new().unwrap();
        let frame_dir = tmp.path().join("frame");
        std::fs::create_dir_all(&frame_dir).unwrap();

        let entry = RecoveryEntry {
            timestamp: Utc::now(),
            category: RecoveryCategory::Write,
            description: "multi-field test".to_string(),
            fields: vec![
                ("Source".to_string(), "tracks/main.md".to_string()),
                ("Target".to_string(), "tracks/side.md".to_string()),
                ("Error".to_string(), "Permission denied".to_string()),
            ],
            body: String::new(),
        };
        log_recovery(&frame_dir, entry);

        let entries = read_recovery_entries(&frame_dir, None, None);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].fields.len(), 3);
        assert_eq!(entries[0].fields[0].0, "Source");
        assert_eq!(entries[0].fields[1].0, "Target");
        assert_eq!(entries[0].fields[2].0, "Error");
        assert_eq!(entries[0].fields[2].1, "Permission denied");
    }

    #[test]
    fn test_read_nonexistent_returns_empty() {
        let tmp = TempDir::new().unwrap();
        let frame_dir = tmp.path().join("nonexistent");
        let entries = read_recovery_entries(&frame_dir, None, None);
        assert!(entries.is_empty());
    }

    #[test]
    fn test_category_from_str() {
        assert_eq!(
            RecoveryCategory::parse_category("parser"),
            Some(RecoveryCategory::Parser)
        );
        assert_eq!(
            RecoveryCategory::parse_category("conflict"),
            Some(RecoveryCategory::Conflict)
        );
        assert_eq!(
            RecoveryCategory::parse_category("write"),
            Some(RecoveryCategory::Write)
        );
        assert_eq!(
            RecoveryCategory::parse_category("delete"),
            Some(RecoveryCategory::Delete)
        );
        assert_eq!(RecoveryCategory::parse_category("unknown"), None);
    }

    /// The lock guarding the log is a separate file that outlives the lock, for
    /// the reason `FileLock::acquire_at` documents: the log is replaced by
    /// rename, so locking the log itself would let a waiter hold a lock on an
    /// unlinked inode while a newcomer locks the fresh file.
    #[test]
    fn the_log_lock_is_a_separate_file_that_survives_release() {
        let tmp = TempDir::new().unwrap();
        let frame_dir = tmp.path().join("frame");
        std::fs::create_dir_all(&frame_dir).unwrap();

        log_recovery(
            &frame_dir,
            make_entry(RecoveryCategory::Parser, "test", "body"),
        );

        let lock_path = recovery_lock_path(&frame_dir);
        assert!(lock_path.exists(), "the lock file is left in place");
        assert_ne!(lock_path, recovery_log_path(&frame_dir));
    }

    /// Pruning replaces the log by rename. The lock is a different file, so it
    /// is still there afterwards and still guards the new inode.
    #[test]
    fn the_log_lock_outlives_a_prune() {
        let tmp = TempDir::new().unwrap();
        let frame_dir = tmp.path().join("frame");
        std::fs::create_dir_all(&frame_dir).unwrap();

        log_recovery(
            &frame_dir,
            make_entry(RecoveryCategory::Parser, "test", "body"),
        );
        prune_recovery(&frame_dir, None, true).unwrap();

        assert!(recovery_lock_path(&frame_dir).exists());
        // And the log is still usable: a later entry appends cleanly.
        log_recovery(
            &frame_dir,
            make_entry(RecoveryCategory::Write, "after", "body"),
        );
        let entries = read_recovery_entries(&frame_dir, None, None);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].description, "after");
    }
}
