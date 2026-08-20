use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

/// Configuration from project.toml
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectConfig {
    pub project: ProjectInfo,
    #[serde(default)]
    pub agent: AgentConfig,
    #[serde(default)]
    pub tracks: Vec<TrackConfig>,
    #[serde(default)]
    pub clean: CleanConfig,
    #[serde(default)]
    pub ids: IdConfig,
    #[serde(default)]
    pub ui: UiConfig,
    #[serde(default)]
    pub recovery: RecoveryConfig,
    #[serde(default)]
    pub limits: LimitsConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectInfo {
    pub name: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AgentConfig {
    /// Which track `fr ready --cc` looks at first, or none.
    ///
    /// **Empty means none**, which is what the shipped template writes and
    /// documents, so it is read that way here rather than left as `Some("")`
    /// for every caller to remember — `fr ready --cc` used to put an empty
    /// track id at the head of its list on any project created by `fr init`.
    #[serde(default, deserialize_with = "empty_string_as_none")]
    pub cc_focus: Option<String>,
    #[serde(default = "default_true")]
    pub cc_only: bool,
}

fn empty_string_as_none<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<String>, D::Error> {
    let value = Option::<String>::deserialize(deserializer)?;
    Ok(value.filter(|s| !s.is_empty()))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrackConfig {
    pub id: String,
    pub name: String,
    pub state: String,
    pub file: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CleanConfig {
    /// Default: see src/templates/project.toml
    #[serde(default = "default_true")]
    pub auto_clean: bool,
    /// Default: see src/templates/project.toml
    #[serde(default = "default_done_threshold")]
    pub done_threshold: usize,
    /// Default: see src/templates/project.toml
    #[serde(default = "default_done_retain")]
    pub done_retain: usize,
    /// Bytes of done tasks a track may hold before archiving is considered.
    ///
    /// A parallel trigger to `done_threshold`, not a replacement: the count
    /// trigger cannot see that 173 done tasks averaging 9 KB are a different
    /// proposition from 173 one-liners, and a track whose notes run long
    /// outgrows every practical file size long before it reaches 100 of them.
    ///
    /// The default is deliberately the same statement as `done_threshold` for
    /// a project whose notes are ordinary — at the ~2.7 KB median a real
    /// project produces, 100 done tasks *is* about 256 KB. The two agree on a
    /// healthy track and diverge exactly where the count is blind.
    ///
    /// `"off"` (or `0`) disables the byte trigger, leaving the count alone.
    #[serde(
        default = "default_done_bytes_threshold",
        deserialize_with = "de_limit"
    )]
    pub done_bytes_threshold: Option<ByteSize>,
    /// What a byte-triggered archive drains down to, not the level it stops at.
    ///
    /// The gap between this and `done_bytes_threshold` is hysteresis, and it is
    /// load-bearing: draining only to the trigger would archive one task per
    /// clean forever. It mirrors the count rule, which triggers at 100 and
    /// drains to 10.
    ///
    /// `done_retain` still floors this — the newest handful of done tasks stay
    /// in the track whatever the arithmetic says.
    #[serde(default = "default_done_bytes_retain", deserialize_with = "de_limit")]
    pub done_bytes_retain: Option<ByteSize>,
    /// Default: see src/templates/project.toml
    #[serde(default = "default_true")]
    pub archive_per_track: bool,
}

impl Default for CleanConfig {
    fn default() -> Self {
        CleanConfig {
            auto_clean: true,
            done_threshold: 100,
            done_retain: 10,
            done_bytes_threshold: default_done_bytes_threshold(),
            done_bytes_retain: default_done_bytes_retain(),
            archive_per_track: true,
        }
    }
}

/// Limits on how large frame will let its own commands make things.
///
/// **A guardrail on authoring, not an invariant on the file.** Markdown is the
/// source of truth and stays hand-editable, and `fr import` is exempt because
/// it is how pre-existing content enters frame. Neither is a hole to be closed
/// later: a project that already holds an oversize note is a supported state,
/// and `fr check` does not report one as damage.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LimitsConfig {
    /// The largest note frame's own commands will *grow* a note to.
    ///
    /// Enforcement is non-increasing rather than absolute: a write is refused
    /// only if it would leave the note both over the limit and larger than it
    /// already was. An existing 148 KB note is therefore never touched, never
    /// truncated and never blocks the operations that do not lengthen it — and
    /// it can still be edited down, in as many passes as it takes, because a
    /// shrinking write is always legal. An absolute check would leave no path
    /// off a note that big except one edit landing under the limit in a single
    /// shot.
    ///
    /// `"off"` (or `0`) disables the limit.
    #[serde(default = "default_note_max_bytes", deserialize_with = "de_limit")]
    pub note_max_bytes: Option<ByteSize>,
    /// Live content — `## Backlog` plus `## Parked` — past which `fr check`
    /// warns that a track holds too much open work.
    ///
    /// **Live content, not file size, and the distinction is the design.** Done
    /// is excluded because `[clean]` already bounds it automatically, and by
    /// design it *oscillates*: it triggers at `done_bytes_threshold` and drains
    /// to `done_bytes_retain`, so it swings across that whole band in normal
    /// operation. A whole-file measure inherits the band as slop — the same
    /// track warns at 520 KB just before a clean and sits quiet at 334 KB just
    /// after, with identical open work, and the warning clears itself with no
    /// human action. Measuring only the term nothing auto-remediates keeps the
    /// signal about the one thing a human can act on.
    ///
    /// `"off"` (or `0`) disables the warning.
    #[serde(default = "default_track_warn_bytes", deserialize_with = "de_limit")]
    pub track_warn_bytes: Option<ByteSize>,
    /// Shortest run of lines an append may not repeat verbatim into a note, and
    /// the same threshold `fr check` reports an existing note for holding twice.
    ///
    /// `fr note` appends, and an agent that believes it replaces will write the
    /// whole note out again each time. The note then holds N copies of
    /// everything that did not change that round. Measured on a real project:
    /// one note reached eight copies of itself, 110 KB of its 139 KB, and
    /// across the project 5.4% of all note text was duplication of this kind.
    ///
    /// Compared as **runs of consecutive lines** on exact text. Lines rather
    /// than paragraph blocks because blocks made the guard answer to the
    /// author's punctuation instead of to the duplication: a note whose sections
    /// are separated by blank lines is several blocks and a re-sent section is
    /// caught, while the same sections written as consecutive lines are one
    /// block and re-sending three of four verbatim matched nothing. Both notes
    /// were duplicating the same text.
    ///
    /// Exact matching either way. The repetition it is after is literally
    /// re-pasted, so exact matching finds it, and unlike a similarity score it
    /// cannot refuse a write that was fine — the failure mode of a guess here is
    /// blocking someone's legitimate note.
    ///
    /// `"off"` (or `0`) disables both the guard and the report.
    #[serde(default = "default_note_repeat_bytes", deserialize_with = "de_limit")]
    pub note_repeat_bytes: Option<ByteSize>,
}

impl Default for LimitsConfig {
    fn default() -> Self {
        LimitsConfig {
            note_max_bytes: default_note_max_bytes(),
            track_warn_bytes: default_track_warn_bytes(),
            note_repeat_bytes: default_note_repeat_bytes(),
        }
    }
}

/// 16 KB. Around p93 of the notes in a real long-running project: high enough
/// that a dated investigation record with file:line citations — the thing a
/// note is for, and which runs 2–6 KB — is nowhere near it, low enough to stop
/// the class of note that has stopped being a record and become a document.
fn default_note_max_bytes() -> Option<ByteSize> {
    Some(ByteSize(16 * 1024))
}

/// 512 KB of open work. At a ~2.7 KB median note that is roughly 190 live
/// tasks in one track, which is a defensible "this wants splitting" mark on its
/// own terms rather than a fit to any one project's numbers.
fn default_track_warn_bytes() -> Option<ByteSize> {
    Some(ByteSize(512 * 1024))
}

/// 120 bytes. The sensitivity curve is almost flat either side of it — over a
/// real corpus, 40 bytes flags 19 notes and 120 flags 16, for 95% of the same
/// duplicated text — so the value is chosen for the end that matters: high
/// enough that a repeated one-line code fragment or error string, which a note
/// may legitimately quote twice, is not a refused write. That reasoning is why
/// the unit moved from blocks to line runs and the number did not: it was always
/// about how much text is repeated, and only the measure was wrong.
fn default_note_repeat_bytes() -> Option<ByteSize> {
    Some(ByteSize(120))
}

/// 256 KB. See [`CleanConfig::done_bytes_threshold`].
fn default_done_bytes_threshold() -> Option<ByteSize> {
    Some(ByteSize(256 * 1024))
}

/// 64 KB. See [`CleanConfig::done_bytes_retain`].
fn default_done_bytes_retain() -> Option<ByteSize> {
    Some(ByteSize(64 * 1024))
}

/// A limit that may be switched off, where [`ByteSize`] alone may not be.
///
/// `ByteSize` rejects zero outright, for a reason that belongs to the recovery
/// log: a max size of zero there would trim on every write, so it is always a
/// mistake. For a limit, off is a legitimate thing to ask for — these refuse
/// writes and warn, and there is no `--force`, so the config *is* the escape
/// hatch. `0`, `"0"` and `"off"` all mean off; anything else parses as a size.
fn de_limit<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<ByteSize>, D::Error> {
    use serde::de::Error;
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Raw {
        Int(i64),
        Text(String),
    }
    match Option::<Raw>::deserialize(deserializer)? {
        None => Ok(None),
        Some(Raw::Int(0)) => Ok(None),
        Some(Raw::Int(n)) if n > 0 => Ok(Some(ByteSize(n as u64))),
        Some(Raw::Int(n)) => Err(D::Error::custom(format!(
            "invalid limit {n}: must be a positive number of bytes, or 0 / \"off\" to disable"
        ))),
        Some(Raw::Text(s)) => {
            let t = s.trim();
            if t.eq_ignore_ascii_case("off") || t.eq_ignore_ascii_case("none") || t == "0" {
                return Ok(None);
            }
            ByteSize::parse(t).map(Some).map_err(D::Error::custom)
        }
    }
}

/// Default: see src/templates/project.toml
fn default_true() -> bool {
    true
}

/// Default: see src/templates/project.toml
fn default_done_threshold() -> usize {
    100
}

/// Default: see src/templates/project.toml
fn default_done_retain() -> usize {
    10
}

fn default_board_done_days() -> u32 {
    7
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct IdConfig {
    #[serde(default)]
    pub prefixes: IndexMap<String, String>,
}

/// The recovery log's size, retention and location.
///
/// **The size is a trigger for housekeeping, not a cap.** Exceeding `max_size`
/// is what makes frame *consider* trimming; `prune_age_days` decides what may
/// then go. A log full of recent entries grows past its limit and loses
/// nothing, which is the right way round — the entries most worth keeping are
/// the newest ones. If retention is the thing you care about, `prune_age_days`
/// is the setting to change.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecoveryConfig {
    /// Size past which an append also considers trimming. A bare integer is
    /// bytes; a string may carry a unit (`"5MB"`, `"512KB"`, `"5MiB"`).
    #[serde(default = "default_recovery_max_size")]
    pub max_size: ByteSize,
    /// How old an entry must be before a trim or a bare `fr recovery prune` may
    /// remove it.
    #[serde(default = "default_prune_age_days")]
    pub prune_age_days: i64,
    /// Where the log lives, overriding the default location.
    ///
    /// A **relative** path resolves against the project root and is a
    /// team-wide choice that is correct on every machine — `frame/.recovery.log`
    /// pins the log to each working copy, for anyone who wants that.
    ///
    /// An **absolute** path is accepted but is machine-specific, and
    /// `project.toml` is committed. Prefer the `FRAME_RECOVERY_LOG` environment
    /// variable, which overrides this and belongs to one machine by nature.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

impl Default for RecoveryConfig {
    fn default() -> Self {
        RecoveryConfig {
            max_size: default_recovery_max_size(),
            prune_age_days: default_prune_age_days(),
            path: None,
        }
    }
}

/// 5 MiB. The previous 1 MiB was sized for a file sitting in the working tree,
/// where an accidental commit was conceivable; the log's default home is inside
/// the git directory now, where it cannot be committed at all. At the ~2.8 KB
/// per entry a real project produces, this is roughly 1,900 entries — and it is
/// shared by every worktree of a clone, so the old figure divided by however
/// many working copies were in play.
fn default_recovery_max_size() -> ByteSize {
    ByteSize(5 * 1024 * 1024)
}

fn default_prune_age_days() -> i64 {
    30
}

/// A size in bytes, written as a plain integer or as a string with a unit.
///
/// `KB`/`MB`/`GB` are binary (1024-based) and `KiB`/`MiB`/`GiB` are accepted as
/// synonyms for them. That is not the SI reading, and it is deliberate: nobody
/// setting a log size means 1,000,000 bytes when they write `1MB`, and a config
/// file is a bad place to be pedantic at the user's expense. Stated in the docs
/// rather than left to be discovered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct ByteSize(pub u64);

impl ByteSize {
    pub fn bytes(self) -> u64 {
        self.0
    }

    /// A rounded size for prose — `19.6KB`, `1.5MB`, `512B`.
    ///
    /// Separate from [`Display`](std::fmt::Display), which is exact and is how a
    /// configured value is echoed back: it falls through to raw bytes for
    /// anything not a whole multiple of its unit, so a *measured* size prints as
    /// `20024B` next to a configured `16KB` and the reader has to do the
    /// division themselves. Measurements are almost never round; limits almost
    /// always are. Reporting surfaces want this one.
    pub fn human(self) -> String {
        const K: f64 = 1024.0;
        let b = self.0 as f64;
        let (value, unit) = if b >= K * K * K {
            (b / (K * K * K), "GB")
        } else if b >= K * K {
            (b / (K * K), "MB")
        } else if b >= K {
            (b / K, "KB")
        } else {
            return format!("{}B", self.0);
        };
        // One decimal, but not a bare `.0` — `16KB` reads better than `16.0KB`,
        // and a limit is usually exact.
        if (value.round() - value).abs() < 0.05 {
            format!("{:.0}{}", value, unit)
        } else {
            format!("{:.1}{}", value, unit)
        }
    }

    /// Parse `"5MB"`, `"5 mb"`, `"5MiB"`, or a bare `"5242880"`.
    pub fn parse(text: &str) -> Result<Self, String> {
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return Err("expected a size such as \"5MB\" or a number of bytes".to_string());
        }

        let split = trimmed
            .find(|c: char| !c.is_ascii_digit())
            .unwrap_or(trimmed.len());
        let (digits, unit) = trimmed.split_at(split);
        if digits.is_empty() {
            return Err(format!(
                "invalid size {text:?}: expected a number, optionally followed by KB, MB or GB"
            ));
        }

        let value: u64 = digits
            .parse()
            .map_err(|_| format!("invalid size {text:?}: {digits:?} is not a whole number"))?;

        let multiplier = match unit.trim().to_ascii_lowercase().as_str() {
            "" | "b" => 1u64,
            "k" | "kb" | "kib" => 1024,
            "m" | "mb" | "mib" => 1024 * 1024,
            "g" | "gb" | "gib" => 1024 * 1024 * 1024,
            other => {
                return Err(format!(
                    "invalid size {text:?}: unknown unit {other:?} (use KB, MB or GB)"
                ));
            }
        };

        let bytes = value
            .checked_mul(multiplier)
            .ok_or_else(|| format!("invalid size {text:?}: too large"))?;
        if bytes == 0 {
            return Err(format!(
                "invalid size {text:?}: a size of zero would trim the log on every write"
            ));
        }
        Ok(ByteSize(bytes))
    }
}

impl std::fmt::Display for ByteSize {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        const K: u64 = 1024;
        let (value, unit) = match self.0 {
            b if b >= K * K * K && b % (K * K * K) == 0 => (b / (K * K * K), "GB"),
            b if b >= K * K && b % (K * K) == 0 => (b / (K * K), "MB"),
            b if b >= K && b % K == 0 => (b / K, "KB"),
            b => (b, "B"),
        };
        write!(f, "{value}{unit}")
    }
}

impl Serialize for ByteSize {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for ByteSize {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Int(i64),
            Text(String),
        }
        match Raw::deserialize(deserializer)? {
            Raw::Int(n) if n > 0 => Ok(ByteSize(n as u64)),
            Raw::Int(n) => Err(D::Error::custom(format!(
                "invalid size {n}: must be a positive number of bytes"
            ))),
            Raw::Text(s) => ByteSize::parse(&s).map_err(D::Error::custom),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UiConfig {
    #[serde(default)]
    pub show_key_hints: bool,
    #[serde(default)]
    pub colors: IndexMap<String, String>,
    #[serde(default)]
    pub tag_colors: IndexMap<String, String>,
    /// File extensions to show in ref/spec autocomplete (e.g. ["md", "txt", "pdf"]).
    /// If empty, all files are shown.
    #[serde(default)]
    pub ref_extensions: Vec<String>,
    /// Directories to scope ref/spec autocomplete to (e.g. ["doc", "spec"]).
    /// If empty, the whole project is searched.
    #[serde(default)]
    pub ref_paths: Vec<String>,
    /// Tags always shown in autocomplete (even if no tasks use them yet).
    #[serde(default)]
    pub default_tags: Vec<String>,
    /// Kitty keyboard protocol: true = force on, false = force off, absent = on (default).
    /// Disable if your terminal has issues with enhanced key reporting.
    #[serde(default)]
    pub kitty_keyboard: Option<bool>,
    /// Whether note editing uses soft word wrap (default: true).
    #[serde(default = "default_true")]
    pub note_wrap: bool,
    /// Days of done tasks to show on the board view (default: 7, 0 = hide done column).
    #[serde(default = "default_board_done_days")]
    pub board_done_days: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_config_default() {
        let c = CleanConfig::default();
        assert!(c.auto_clean);
        assert_eq!(c.done_threshold, 100);
        assert_eq!(c.done_retain, 10);
        assert!(c.archive_per_track);
    }

    #[test]
    fn agent_config_default() {
        let a = AgentConfig::default();
        assert!(a.cc_focus.is_none());
        // cc_only default when using Default trait is false (bool default),
        // but serde default_true applies during deserialization
        assert!(!a.cc_only);
    }

    #[test]
    fn agent_config_serde_default_true() {
        // When deserialized from an empty object, cc_only should be true via serde
        let a: AgentConfig = serde_json::from_str("{}").unwrap();
        assert!(a.cc_only);
        assert!(a.cc_focus.is_none());
    }

    #[test]
    fn ui_config_default() {
        let u = UiConfig::default();
        assert!(!u.show_key_hints);
        assert!(u.colors.is_empty());
        assert!(u.tag_colors.is_empty());
        assert!(u.ref_extensions.is_empty());
        assert!(u.ref_paths.is_empty());
        assert!(u.default_tags.is_empty());
        assert!(u.kitty_keyboard.is_none());
        // note_wrap default via Default trait is false (bool default)
        assert!(!u.note_wrap);
    }

    #[test]
    fn ui_config_serde_note_wrap_default_true() {
        // When deserialized from empty object, note_wrap should be true via serde
        let u: UiConfig = serde_json::from_str("{}").unwrap();
        assert!(u.note_wrap);
    }

    // -- [recovery] ---------------------------------------------------------

    const K: u64 = 1024;

    #[test]
    fn a_size_may_be_written_with_or_without_a_unit() {
        for (text, expected) in [
            ("5242880", 5 * K * K),
            ("5MB", 5 * K * K),
            ("5mb", 5 * K * K),
            ("5 MB", 5 * K * K),
            ("5MiB", 5 * K * K),
            ("5M", 5 * K * K),
            ("512KB", 512 * K),
            ("512kib", 512 * K),
            ("2GB", 2 * K * K * K),
            ("900", 900),
            ("900B", 900),
        ] {
            assert_eq!(
                ByteSize::parse(text).map(|b| b.bytes()),
                Ok(expected),
                "parsing {text:?}"
            );
        }
    }

    #[test]
    fn a_size_that_is_not_a_size_is_rejected_by_name() {
        for text in ["", "   ", "MB", "5 megabytes", "-1", "5.5MB", "five"] {
            let err = ByteSize::parse(text).unwrap_err();
            assert!(
                err.contains("size"),
                "the message for {text:?} should say what was wrong: {err}"
            );
        }
    }

    /// Zero would trim on every single write, which is a data-loss setting
    /// wearing a size's clothes.
    #[test]
    fn a_size_of_zero_is_rejected() {
        assert!(ByteSize::parse("0").is_err());
        assert!(ByteSize::parse("0MB").is_err());
    }

    /// `2GB` fits in a u64 but the multiply is where an overflow would hide.
    #[test]
    fn an_enormous_size_is_reported_rather_than_wrapping() {
        let err = ByteSize::parse("99999999999999999999GB").unwrap_err();
        assert!(err.contains("size"), "{err}");
        assert!(ByteSize::parse("17179869184GB").is_err(), "u64 overflow");
    }

    #[test]
    fn a_size_round_trips_through_toml() {
        let config: RecoveryConfig = toml::from_str("max_size = \"512KB\"").unwrap();
        assert_eq!(config.max_size.bytes(), 512 * K);
        let text = toml::to_string(&config).unwrap();
        assert!(text.contains("max_size = \"512KB\""), "{text}");
    }

    #[test]
    fn a_bare_integer_in_toml_is_bytes() {
        let config: RecoveryConfig = toml::from_str("max_size = 4096").unwrap();
        assert_eq!(config.max_size.bytes(), 4096);
    }

    #[test]
    fn a_negative_size_in_toml_is_rejected() {
        assert!(toml::from_str::<RecoveryConfig>("max_size = -1").is_err());
        assert!(toml::from_str::<RecoveryConfig>("max_size = 0").is_err());
    }

    #[test]
    fn the_recovery_defaults_are_five_megabytes_and_thirty_days() {
        let config = RecoveryConfig::default();
        assert_eq!(config.max_size.bytes(), 5 * K * K);
        assert_eq!(config.prune_age_days, 30);
        assert_eq!(config.path, None);

        // And an absent section is the same as the defaults.
        let parsed: RecoveryConfig = toml::from_str("").unwrap();
        assert_eq!(parsed.max_size, config.max_size);
        assert_eq!(parsed.prune_age_days, 30);
    }

    #[test]
    fn a_project_without_a_recovery_section_still_parses() {
        let config: ProjectConfig = toml::from_str("[project]\nname = \"x\"\n").unwrap();
        assert_eq!(config.recovery.max_size.bytes(), 5 * K * K);
    }
}
