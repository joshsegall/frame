use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use tempfile::NamedTempFile;

/// Maximum size of the recovery log before inline trimming (1 MB).
const MAX_LOG_SIZE: u64 = 1_048_576;

/// Default number of days before entries are prunable.
pub const PRUNE_AGE_DAYS: i64 = 30;

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

/// Return the path to the recovery log file.
pub fn recovery_log_path(frame_dir: &Path) -> PathBuf {
    frame_dir.join(".recovery.log")
}

// ---------------------------------------------------------------------------
// Atomic file write
// ---------------------------------------------------------------------------

/// Write `content` to `path` atomically using a temp file + rename.
pub fn atomic_write(path: &Path, content: &[u8]) -> io::Result<()> {
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
    let path = recovery_log_path(frame_dir);

    // Check size and try inline trim (non-blocking)
    if let Ok(meta) = std::fs::metadata(&path)
        && meta.len() > MAX_LOG_SIZE
    {
        try_inline_trim(&path);
    }

    let needs_header = !path.exists() || std::fs::metadata(&path).map_or(true, |m| m.len() == 0);

    let mut file = OpenOptions::new().create(true).append(true).open(&path)?;

    if needs_header {
        file.write_all(FILE_HEADER.as_bytes())?;
    }

    let markdown = entry.to_markdown();
    file.write_all(markdown.as_bytes())?;

    Ok(())
}

/// Try to trim old entries when the log exceeds MAX_LOG_SIZE.
/// Uses a non-blocking try-lock on the file itself.
fn try_inline_trim(path: &Path) {
    // Try to acquire exclusive lock non-blocking
    let file = match OpenOptions::new().read(true).write(true).open(path) {
        Ok(f) => f,
        Err(_) => return,
    };

    // Non-blocking flock
    let fd = {
        use std::os::unix::io::AsRawFd;
        file.as_raw_fd()
    };
    let ret = unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) };
    if ret != 0 {
        return; // Couldn't get lock — skip trim
    }

    // Read content, trim oldest entries until under limit
    let mut content = String::new();
    let mut reader = io::BufReader::new(&file);
    if reader.read_to_string(&mut content).is_err() {
        return;
    }

    let cutoff = Utc::now() - chrono::Duration::days(PRUNE_AGE_DAYS);
    let trimmed = prune_entries_before(&content, &cutoff);

    if trimmed.len() < content.len() {
        // Rewrite the file
        if let Ok(mut f) = File::create(path) {
            let _ = f.write_all(trimmed.as_bytes());
        }
    }

    // Lock released on drop
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
    let path = recovery_log_path(frame_dir);
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };

    let mut entries = parse_entries(&content);

    // Filter by timestamp
    if let Some(since_dt) = since {
        entries.retain(|e| e.timestamp >= since_dt);
    }

    // Return most recent entries (entries are parsed oldest-first)
    if let Some(n) = limit {
        let skip = entries.len().saturating_sub(n);
        entries = entries.into_iter().skip(skip).collect();
    }

    // Reverse so most recent is first
    entries.reverse();
    entries
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
    let path = recovery_log_path(frame_dir);
    if !path.exists() {
        return Ok(0);
    }

    // Acquire exclusive lock
    let file = OpenOptions::new().read(true).write(true).open(&path)?;
    let fd = {
        use std::os::unix::io::AsRawFd;
        file.as_raw_fd()
    };

    // Blocking lock with ~1s timeout: try non-blocking first, then sleep-retry
    let mut locked = false;
    for _ in 0..10 {
        let ret = unsafe { libc::flock(fd, libc::LOCK_EX | libc::LOCK_NB) };
        if ret == 0 {
            locked = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    if !locked {
        return Err(io::Error::new(
            io::ErrorKind::WouldBlock,
            "recovery log is in use, try again later",
        ));
    }

    let content = std::fs::read_to_string(&path)?;

    if all {
        let entries = parse_entries(&content);
        let count = entries.len();
        // Write header only
        std::fs::write(&path, FILE_HEADER)?;
        return Ok(count);
    }

    let cutoff = before.unwrap_or_else(|| Utc::now() - chrono::Duration::days(PRUNE_AGE_DAYS));
    let original_entries = parse_entries(&content);
    let original_count = original_entries.len();

    let trimmed = prune_entries_before(&content, &cutoff);
    let new_entries = parse_entries(&trimmed);
    let new_count = new_entries.len();

    std::fs::write(&path, &trimmed)?;
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
}
