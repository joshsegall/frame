//! A corpus of **damaged projects**, each declaring the complete set of findings
//! `fr check` must report for it.
//!
//! # Why this exists, given every detector already has a test
//!
//! Twenty-two of the twenty-three finding variants have a dedicated test in
//! `src/ops/check.rs`. Every one of them builds a project that triggers exactly
//! one condition, asserts that condition is reported, and says nothing about what
//! *else* was reported — and the project it builds exists only inside that test.
//!
//! So a detector that fires on damage belonging to a **different** detector is
//! invisible to all of them. That is `d0350a1`: a new warning reported
//! "MAI-001 is held by 2 tasks, including an archived one … the number was handed
//! out twice" for a project where nothing had been reissued and no live task
//! existed. It had fired on the duplicated-archive-history condition, which is a
//! different problem with a different repair. Its own test passed. `9e183a8` had
//! to split the warning in two.
//!
//! The value here is the **cross-product**: every detector runs against every
//! damage shape, because every case declares its findings *exhaustively*. A
//! detector that over-fires fails the build, naming the case it broke.
//!
//! # What a case guarantees
//!
//! 1. **Exact set** — findings equal `expect` as a multiset, compared on the tag
//!    *and the identifying payload*. Naming the right condition with the wrong
//!    details is the failure mode this exists to catch, so `total`, `archives`
//!    and the rest are asserted, not just the tag.
//! 2. **Attributable** — every case starts from one shared clean baseline
//!    ([`baseline`], pinned silent by [`the_baseline_is_silent`]) and breaks
//!    exactly one thing, so any finding belongs to that one thing.
//! 3. **Repair** — [`Repair`] declares what `fr check --fix` does with the case.
//!    `Repair::None` is the valuable one: it pins the findings deliberately left
//!    unrepaired, so adding a repair to `fix::plan` means coming here and saying
//!    so.
//!
//! # Staying complete
//!
//! [`every_finding_tag_has_a_case`] reads the finding tags out of
//! `src/ops/check.rs` itself and requires each to appear in [`CASES`]. A new
//! detector fails the build until it has a case. Following `tests/parity.rs`, the
//! guard checks the real source of truth rather than a hand-maintained list, and
//! runs in both directions so a case for a deleted variant is caught too.
//!
//! **Not a fuzzer.** Fixed, named damage with stated provenance, and a
//! known-correct answer to compare against. `tests/parse_properties.rs` generates
//! damage and finds parser bugs; this finds *detector* bugs. The two are
//! complementary and this one should not drift toward generation.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;
use std::process::Command;

use frame::io::project_io;
use frame::ops::check::{CheckResult, check_project};
use frame::ops::fix;
use serde_json::Value;

// ---------------------------------------------------------------------------
// Expectations
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Severity {
    Error,
    Warning,
    Info,
}

impl Severity {
    fn label(self) -> &'static str {
        match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Info => "info",
        }
    }
}

/// How one payload field is compared.
#[derive(Clone, Copy)]
enum Match {
    /// Exactly this string. Arrays are joined with `,`, so a one-element list
    /// reads as the element — which is how the `d0350a1` bug (one file named
    /// twice) shows up as a mismatch rather than passing.
    Eq(&'static str),
    /// Ends with this. For absolute paths, which depend on the temp directory.
    Suffix(&'static str),
    /// Present, value unasserted. For timestamps.
    Any,
}

/// One finding a case must produce. Fields are a *projection*: only what
/// identifies the finding is asserted, so adding an unrelated field to a variant
/// does not churn the table.
struct Expect {
    severity: Severity,
    tag: &'static str,
    fields: &'static [(&'static str, Match)],
}

const fn error(tag: &'static str, fields: &'static [(&'static str, Match)]) -> Expect {
    Expect {
        severity: Severity::Error,
        tag,
        fields,
    }
}

const fn warning(tag: &'static str, fields: &'static [(&'static str, Match)]) -> Expect {
    Expect {
        severity: Severity::Warning,
        tag,
        fields,
    }
}

const fn info(tag: &'static str, fields: &'static [(&'static str, Match)]) -> Expect {
    Expect {
        severity: Severity::Info,
        tag,
        fields,
    }
}

impl Expect {
    fn matches(&self, found: &Finding) -> bool {
        if found.severity != self.severity || found.tag() != self.tag {
            return false;
        }
        self.fields.iter().all(|(key, want)| {
            let Some(have) = found.value.get(key).map(flatten) else {
                return false;
            };
            match want {
                Match::Eq(s) => have == *s,
                Match::Suffix(s) => have.ends_with(s),
                Match::Any => true,
            }
        })
    }

    fn describe(&self) -> String {
        let fields: Vec<String> = self
            .fields
            .iter()
            .map(|(k, m)| match m {
                Match::Eq(s) => format!("{k}={s}"),
                Match::Suffix(s) => format!("{k}=*{s}"),
                Match::Any => format!("{k}=<any>"),
            })
            .collect();
        format!(
            "{} {} {{{}}}",
            self.severity.label(),
            self.tag,
            fields.join(", ")
        )
    }
}

/// A finding as check reported it, flattened to JSON so the table compares
/// against the same shape `--json` consumers see.
struct Finding {
    severity: Severity,
    value: Value,
}

impl Finding {
    fn tag(&self) -> &str {
        self.value
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("?")
    }

    fn describe(&self) -> String {
        format!("{} {}", self.severity.label(), self.value)
    }
}

/// Render a JSON value as the string the table compares against.
fn flatten(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Array(items) => items.iter().map(flatten).collect::<Vec<_>>().join(","),
        other => other.to_string(),
    }
}

fn findings(result: &CheckResult) -> Vec<Finding> {
    let mut out = Vec::new();
    for e in &result.errors {
        out.push(Finding {
            severity: Severity::Error,
            value: serde_json::to_value(e).expect("finding serializes"),
        });
    }
    for w in &result.warnings {
        out.push(Finding {
            severity: Severity::Warning,
            value: serde_json::to_value(w).expect("finding serializes"),
        });
    }
    for i in &result.info {
        out.push(Finding {
            severity: Severity::Info,
            value: serde_json::to_value(i).expect("finding serializes"),
        });
    }
    out
}

/// Compare as a multiset, reporting both directions. Greedy matching is exact
/// here because no two `expect` entries in a case are satisfied by the same
/// finding — if that ever stops holding, the leftovers name it.
fn assert_findings(case: &str, expected: &[Expect], result: &CheckResult) {
    let found = findings(result);
    let mut taken = vec![false; found.len()];
    let mut unmatched_expected = Vec::new();

    for want in expected {
        match found
            .iter()
            .enumerate()
            .find(|(i, f)| !taken[*i] && want.matches(f))
        {
            Some((i, _)) => taken[i] = true,
            None => unmatched_expected.push(want.describe()),
        }
    }

    let unexpected: Vec<String> = found
        .iter()
        .zip(&taken)
        .filter(|(_, t)| !**t)
        .map(|(f, _)| f.describe())
        .collect();

    if unmatched_expected.is_empty() && unexpected.is_empty() {
        return;
    }

    let mut msg = format!("case `{case}`: check did not report what the corpus declares\n");
    if !unmatched_expected.is_empty() {
        msg.push_str(
            "\n  declared but NOT reported (detector regressed, or the payload changed):\n",
        );
        for e in &unmatched_expected {
            msg.push_str(&format!("    {e}\n"));
        }
    }
    if !unexpected.is_empty() {
        msg.push_str(
            "\n  reported but NOT declared (a detector is firing on damage that isn't its own,\n   \
             or this case grew a second finding that should be its own case):\n",
        );
        for u in &unexpected {
            msg.push_str(&format!("    {u}\n"));
        }
    }
    panic!("{msg}");
}

/// Nothing damaging survives a repair.
///
/// Deliberately blind to `info` findings, because a deleting repair writes the
/// removed content to the recovery log before removing it — so `recovery_log`
/// appearing here is the audit trail working, not residue. It has its own case.
fn assert_no_damage(case: &str, result: &CheckResult) {
    let leftover: Vec<String> = findings(result)
        .iter()
        .filter(|f| f.severity != Severity::Info)
        .map(Finding::describe)
        .collect();
    assert!(
        leftover.is_empty(),
        "case `{case}`: `--fix` left damage behind:\n    {}",
        leftover.join("\n    ")
    );
}

// ---------------------------------------------------------------------------
// Cases
// ---------------------------------------------------------------------------

/// What `fr check --fix` must do with a case.
enum Repair {
    /// Applying the plan removes every finding the case declares, and adds none.
    Clears,
    /// No repair exists. The plan must be **empty** — a repair appearing here is
    /// a decision nobody made. This is what pins `fix.rs`'s "deliberately not
    /// repaired" list as something executable rather than prose.
    None,
}

/// Whether a builder could set the case up in this environment.
enum Built {
    Ok,
    Skipped(&'static str),
}

struct Case {
    /// Kebab name, used in failure messages.
    name: &'static str,
    /// How this damage arises in the wild, so a reader can tell a realistic case
    /// from an invented one.
    provenance: &'static str,
    /// The finding tags this case is the corpus entry for. Drives the
    /// completeness guard; a case may declare more than one when a single
    /// condition legitimately produces several.
    covers: &'static [&'static str],
    /// Breaks exactly one thing in an already-written baseline project.
    build: fn(&Path) -> Built,
    /// The complete set of findings. Not "at least".
    expect: &'static [Expect],
    repair: Repair,
}

const CASES: &[Case] = &[
    // --- content damage -------------------------------------------------
    Case {
        name: "dangling-dep",
        provenance: "the blocker was deleted, or its id was renumbered by hand",
        covers: &["dangling_dep"],
        build: |root| {
            append_backlog(
                root,
                "- [ ] `M-004` Blocked on a ghost\n  - added: 2026-01-01\n  - dep: M-999\n",
            );
            Built::Ok
        },
        expect: &[error(
            "dangling_dep",
            &[
                ("task_id", Match::Eq("M-004")),
                ("dep_id", Match::Eq("M-999")),
            ],
        )],
        repair: Repair::None,
    },
    Case {
        name: "broken-ref",
        provenance: "the file was moved or renamed after the task referenced it",
        covers: &["broken_ref"],
        build: |root| {
            append_backlog(
                root,
                "- [ ] `M-004` Points at a missing file\n  - added: 2026-01-01\n  - ref: doc/gone.md\n",
            );
            Built::Ok
        },
        expect: &[error(
            "broken_ref",
            &[
                ("task_id", Match::Eq("M-004")),
                ("path", Match::Eq("doc/gone.md")),
            ],
        )],
        repair: Repair::None,
    },
    Case {
        name: "broken-spec",
        provenance: "same, for a `spec:` — the section fragment is stripped before the check",
        covers: &["broken_spec"],
        build: |root| {
            append_backlog(
                root,
                "- [ ] `M-004` Points at a missing spec\n  - added: 2026-01-01\n  - spec: doc/gone.md#Design\n",
            );
            Built::Ok
        },
        expect: &[error(
            "broken_spec",
            &[
                ("task_id", Match::Eq("M-004")),
                ("path", Match::Eq("doc/gone.md#Design")),
            ],
        )],
        repair: Repair::None,
    },
    Case {
        name: "duplicate-id",
        provenance: "a three-way merge kept both sides of a task added on two branches",
        covers: &["duplicate_id"],
        build: |root| {
            append_backlog(
                root,
                "- [ ] `M-001` The same number, twice\n  - added: 2026-01-01\n",
            );
            Built::Ok
        },
        expect: &[error(
            "duplicate_id",
            &[
                ("task_id", Match::Eq("M-001")),
                ("track_ids", Match::Eq("main,main")),
            ],
        )],
        // Resolved by `fr clean`, not here — see `fix.rs`'s header on why the
        // two commands must not both repair a finding.
        repair: Repair::None,
    },
    Case {
        name: "missing-id",
        provenance: "a task typed straight into the markdown by hand",
        covers: &["missing_id"],
        build: |root| {
            append_backlog(root, "- [ ] Typed in by hand, never minted\n");
            Built::Ok
        },
        expect: &[warning(
            "missing_id",
            &[("title", Match::Eq("Typed in by hand, never minted"))],
        )],
        repair: Repair::None,
    },
    Case {
        name: "missing-added-date",
        provenance: "a task copied from elsewhere, or written before dates were filled in",
        covers: &["missing_added_date"],
        build: |root| {
            append_backlog(root, "- [ ] `M-004` No added date\n");
            Built::Ok
        },
        expect: &[warning(
            "missing_added_date",
            &[("task_id", Match::Eq("M-004"))],
        )],
        repair: Repair::None,
    },
    Case {
        name: "missing-resolved-date",
        provenance: "a checkbox ticked in the editor rather than through `fr`",
        covers: &["missing_resolved_date"],
        build: |root| {
            append_done(root, "- [x] `M-004` Done, undated\n  - added: 2026-01-01\n");
            Built::Ok
        },
        expect: &[warning(
            "missing_resolved_date",
            &[("task_id", Match::Eq("M-004"))],
        )],
        repair: Repair::None,
    },
    Case {
        name: "done-in-backlog",
        provenance: "same — a checkbox ticked in place, leaving the task where it sat",
        covers: &["task_in_wrong_section"],
        build: |root| {
            append_backlog(
                root,
                "- [x] `M-004` Ticked in place\n  - added: 2026-01-01\n  - resolved: 2026-01-02\n",
            );
            Built::Ok
        },
        expect: &[warning(
            "task_in_wrong_section",
            &[
                ("task_id", Match::Eq("M-004")),
                ("expected", Match::Eq("done")),
                ("actual", Match::Eq("backlog")),
            ],
        )],
        // Purely positional, so `--fix` moves it. This was `Repair::None` when
        // the finding covered only done-in-Backlog and shipped without one.
        repair: Repair::Clears,
    },
    Case {
        name: "parked-in-done",
        provenance: "a task parked out of `## Done` before the section policy was total",
        covers: &["task_in_wrong_section"],
        build: |root| {
            append_done(
                root,
                "- [~] `M-004` Parked, left in Done\n  - added: 2026-01-01\n",
            );
            Built::Ok
        },
        // The five misplacements the done-in-Backlog check could not see. This
        // one was produced by frame itself — see `5eb069f`.
        expect: &[warning(
            "task_in_wrong_section",
            &[
                ("task_id", Match::Eq("M-004")),
                ("expected", Match::Eq("parked")),
                ("actual", Match::Eq("done")),
            ],
        )],
        repair: Repair::Clears,
    },
    Case {
        name: "done-subtask-under-a-backlog-parent",
        provenance: "an ordinary project: one subtask finished, its parent not",
        // Covers nothing by design — this case exists to assert *silence*. The
        // two cases above carry the tag.
        covers: &[],
        build: |root| {
            append_backlog(
                root,
                "- [ ] `M-004` Parent\n  - added: 2026-01-01\n  - [x] `M-004.1` Finished sub\n    - added: 2026-01-01\n    - resolved: 2026-01-02\n",
            );
            Built::Ok
        },
        // **Nothing.** A subtask has no section of its own, so a finished one
        // under an unfinished parent is the normal shape — but the old check
        // inherited the parent's section and reported it as done-in-Backlog.
        // Unactionable, too: `fr clean` reconciles top-level tasks only, so the
        // warning never went away. This case exists to keep it gone.
        expect: &[],
        repair: Repair::None,
    },
    Case {
        name: "lost-task",
        provenance: "the recovery system re-attached content it could not place",
        covers: &["lost_task"],
        build: |root| {
            append_backlog(
                root,
                "- [ ] `M-004` Recovered content #lost\n  - added: 2026-01-01\n",
            );
            Built::Ok
        },
        expect: &[warning("lost_task", &[("task_id", Match::Eq("M-004"))])],
        repair: Repair::None,
    },
    Case {
        name: "child-id-not-under-parent",
        provenance: "`fr clean` before acdd4f1 resolved a duplicated subtask with a top-level number",
        covers: &["child_id_not_under_parent"],
        build: |root| {
            append_backlog(
                root,
                "- [ ] `M-004` Parent\n  - added: 2026-01-01\n  - [ ] `M-020` Escaped\n    - added: 2026-01-01\n",
            );
            Built::Ok
        },
        expect: &[warning(
            "child_id_not_under_parent",
            &[
                ("task_id", Match::Eq("M-020")),
                ("parent_id", Match::Eq("M-004")),
            ],
        )],
        repair: Repair::Clears,
    },
    Case {
        name: "unclosed-note-fence",
        provenance: "a note written in the TUI editor and saved mid-example",
        covers: &["unclosed_note_fence"],
        build: |root| {
            append_backlog(
                root,
                "- [ ] `M-004` Note with an open fence\n  - added: 2026-01-01\n  - note:\n    Example:\n    ```rust\n    let x = 1;\n",
            );
            Built::Ok
        },
        expect: &[warning(
            "unclosed_note_fence",
            &[
                ("task_id", Match::Eq("M-004")),
                ("fence", Match::Eq("```rust")),
            ],
        )],
        repair: Repair::Clears,
    },
    Case {
        name: "stranded-line",
        provenance: "prose that lost its indent in an editor, or a fragment left by a merge",
        covers: &["stranded_line"],
        build: |root| {
            // Indented past the metadata it follows, so it is neither metadata
            // nor a task nor part of a note block. Carried on the task below it.
            let path = track_path(root);
            let text = fs::read_to_string(&path).unwrap().replace(
                "- [ ] `M-002` Second task\n",
                "    **Shape.** prose that lost its indent\n- [ ] `M-002` Second task\n",
            );
            fs::write(&path, text).unwrap();
            Built::Ok
        },
        expect: &[warning(
            "stranded_line",
            &[
                ("before_task_id", Match::Eq("M-002")),
                ("line", Match::Eq("**Shape.** prose that lost its indent")),
            ],
        )],
        // Where the line was meant to go is a guess, and guessing rewrites prose.
        repair: Repair::None,
    },
    Case {
        name: "unclosed-inbox-fence",
        provenance: "an inbox item captured mid-paste",
        covers: &["unclosed_inbox_fence"],
        build: |root| {
            fs::write(
                root.join("frame/inbox.md"),
                "# Inbox\n\n- Item with an open body fence\n  ```lace\n  perform Ask()\n",
            )
            .unwrap();
            Built::Ok
        },
        expect: &[warning(
            "unclosed_inbox_fence",
            &[("index", Match::Eq("1")), ("fence", Match::Eq("```lace"))],
        )],
        repair: Repair::Clears,
    },
    // --- project-state damage -------------------------------------------
    Case {
        name: "id-reissued-after-archive",
        provenance: "a number minted while the archived task holding it was invisible to the scan",
        covers: &["id_reissued_after_archive"],
        build: |root| {
            write_archive(
                root,
                "# Archive — main\n\n- [x] `M-001` The original holder\n  - resolved: 2025-12-01\n",
            );
            Built::Ok
        },
        expect: &[warning(
            "id_reissued_after_archive",
            &[
                ("task_id", Match::Eq("M-001")),
                ("tracks", Match::Eq("main")),
                ("archives", Match::Eq("archive/main.md")),
            ],
        )],
        // Which of two legitimate holders should move is a human call.
        repair: Repair::None,
    },
    Case {
        name: "duplicate-archived-id",
        provenance: "a `fr clean` whose archive append landed while its track update was lost",
        covers: &["duplicate_archived_id"],
        build: |root| {
            write_archive(
                root,
                "# Archive — main\n\n- [x] `M-050` Archived twice\n  - resolved: 2025-12-01\n- [x] `M-050` Archived twice\n  - resolved: 2025-12-01\n",
            );
            Built::Ok
        },
        // The `d0350a1` case. `archives` must name the file **once**, and this
        // must not also be reported as a reissued number: nothing was reissued,
        // and no live task holds M-050.
        expect: &[warning(
            "duplicate_archived_id",
            &[
                ("task_id", Match::Eq("M-050")),
                ("total", Match::Eq("2")),
                ("archives", Match::Eq("archive/main.md")),
            ],
        )],
        repair: Repair::Clears,
    },
    Case {
        name: "actor-token-unregistered",
        provenance: "the committed registry lost this clone's claim in a merge",
        covers: &["actor_token_unregistered"],
        build: |root| {
            fs::write(root.join("frame/.actor"), "b\n").unwrap();
            fs::write(root.join("frame/actors.toml"), "").unwrap();
            Built::Ok
        },
        expect: &[warning(
            "actor_token_unregistered",
            &[("token", Match::Eq("b"))],
        )],
        // Self-heals on the next mint.
        repair: Repair::None,
    },
    Case {
        name: "actor-token-retired",
        provenance: "the token was retired by `fr actor merge` in another clone",
        covers: &["actor_token_retired"],
        build: |root| {
            fs::write(root.join("frame/.actor"), "b\n").unwrap();
            fs::write(
                root.join("frame/actors.toml"),
                "[actors.b]\nname = \"host\"\nstate = \"retired\"\nclaimed = \"2026-01-01\"\nretired = \"2026-02-01\"\n",
            )
            .unwrap();
            Built::Ok
        },
        expect: &[warning("actor_token_retired", &[("token", Match::Eq("b"))])],
        // Reactivate, or claim fresh? An identity decision.
        repair: Repair::None,
    },
    Case {
        name: "actor-name-collision",
        provenance: "one machine auto-claimed a token per git worktree",
        covers: &["actor_name_collision"],
        build: |root| {
            fs::write(
                root.join("frame/actors.toml"),
                "[actors.null]\nname = \"host\"\nstate = \"active\"\n\n\
                 [actors.b]\nname = \"host\"\nstate = \"active\"\nclaimed = \"2026-01-01\"\n",
            )
            .unwrap();
            Built::Ok
        },
        expect: &[warning(
            "actor_name_collision",
            &[("name", Match::Eq("host")), ("tokens", Match::Eq("null,b"))],
        )],
        // The repair is `fr actor merge`, already documented as a human call.
        repair: Repair::None,
    },
    Case {
        name: "local-file-not-ignored",
        provenance: "a project created before the `.gitignore` pattern existed",
        covers: &["local_file_committed"],
        build: |root| {
            if !git(root, &["init", "-q"]) {
                return Built::Skipped("git unavailable");
            }
            register_merge_driver(root);
            Built::Ok
        },
        // `.actor` exists and is not ignored. `.inflight` is reported whether or
        // not it exists — it lives only in a window nobody is watching, so an
        // existence check would almost never catch it.
        expect: &[
            warning(
                "local_file_committed",
                &[
                    ("path", Match::Eq("frame/.actor")),
                    ("tracked", Match::Eq("false")),
                ],
            ),
            warning(
                "local_file_committed",
                &[
                    ("path", Match::Eq("frame/.inflight")),
                    ("tracked", Match::Eq("false")),
                ],
            ),
        ],
        // `fr check --fix` deliberately leaves git readiness alone — one command
        // owns that surface, and it is `fr git setup`.
        repair: Repair::None,
    },
    Case {
        name: "local-directory-committed",
        provenance: "a rescue dump from an interrupted session, force-added to git",
        covers: &["local_file_committed"],
        build: |root| {
            if !git(root, &["init", "-q"]) {
                return Built::Skipped("git unavailable");
            }
            register_merge_driver(root);
            // The pattern is in place, so nothing else is reported and this case
            // is attributable to the one thing it breaks.
            std::fs::write(root.join(".gitignore"), "frame/.*\n").unwrap();
            let rescue = root.join("frame/.rescue");
            std::fs::create_dir_all(&rescue).unwrap();
            std::fs::write(rescue.join("main.md"), "- [ ] `M-001` rescued\n").unwrap();
            if !git(root, &["add", "-f", "frame/.rescue/main.md"]) {
                return Built::Skipped("git add failed");
            }
            Built::Ok
        },
        // The reason this case exists: `git ls-files` reports the *file* under a
        // committed directory, never the directory, so an equality test called
        // this untracked and offered the wrong remedy — add a `.gitignore` line
        // that was already there, rather than untrack it.
        expect: &[warning(
            "local_file_committed",
            &[
                ("path", Match::Eq("frame/.rescue")),
                ("tracked", Match::Eq("true")),
            ],
        )],
        // Untracking is `git rm --cached`, deliberately left to a human.
        repair: Repair::None,
    },
    Case {
        name: "merge-driver-unregistered",
        provenance: "a fresh clone: `.gitattributes` arrives with it, but the driver \
                     lives in `.git/config`, which cannot be committed",
        covers: &["merge_driver_unregistered"],
        build: |root| {
            if !git(root, &["init", "-q"]) {
                return Built::Skipped("git unavailable");
            }
            // Everything else about the repo is right, so this case reports one
            // thing: git would merge track files line by line.
            std::fs::write(root.join(".gitignore"), "frame/.*\n").unwrap();
            Built::Ok
        },
        expect: &[warning("merge_driver_unregistered", &[])],
        // The repair writes `.git/config`, which is machine state rather than
        // project content — `fr git setup`, not `--fix`.
        repair: Repair::None,
    },
    Case {
        name: "unresolved-merge-conflict",
        provenance: "`fr merge` could not decide a task and left its marker; the \
                     other version went to the recovery log",
        covers: &["unresolved_merge_conflict"],
        build: |root| {
            append_backlog(
                root,
                "- [ ] `M-004` Both sides touched this\n  - added: 2026-01-01\n  \
                 - conflict: both-edited 2026-08-03T04:08:38Z\n",
            );
            Built::Ok
        },
        expect: &[error(
            "unresolved_merge_conflict",
            &[
                ("task_id", Match::Eq("M-004")),
                ("detail", Match::Eq("both-edited 2026-08-03T04:08:38Z")),
            ],
        )],
        // Which side should win is the judgment a machine cannot make — the same
        // reason `id_reissued_after_archive` has no repair.
        repair: Repair::None,
    },
    Case {
        name: "id-frontier-unreadable",
        provenance: "a half-written or hand-edited frontier store",
        covers: &["id_frontier_unreadable"],
        build: |root| {
            fs::write(root.join("frame/.ids.toml"), "this is not toml {{{").unwrap();
            Built::Ok
        },
        expect: &[warning(
            "id_frontier_unreadable",
            &[("path", Match::Suffix(".ids.toml")), ("detail", Match::Any)],
        )],
        // Check deliberately leaves it in place so the warning names a file still
        // worth inspecting.
        repair: Repair::None,
    },
    Case {
        name: "id-frontier-was-reset",
        provenance: "a mint found the store unreadable and moved it aside",
        covers: &["id_frontier_was_reset"],
        build: |root| {
            fs::write(root.join("frame/.ids.toml.bak"), "old frontier").unwrap();
            Built::Ok
        },
        expect: &[warning(
            "id_frontier_was_reset",
            &[("path", Match::Suffix(".ids.toml.bak"))],
        )],
        repair: Repair::Clears,
    },
    Case {
        name: "interrupted-operation",
        provenance: "a cross-track move killed between its two writes",
        covers: &["interrupted_operation"],
        build: |root| {
            let frame_dir = root.join("frame");
            let marker = frame::io::inflight::InFlight::begin(
                &frame_dir,
                frame::io::inflight::Operation::TrackArchive {
                    track_id: "side".to_string(),
                    file: "tracks/side.md".to_string(),
                },
                "fr track archive side",
            )
            .unwrap();
            // Dropped without `commit`, which is exactly what an interrupted
            // command leaves behind.
            drop(marker);
            Built::Ok
        },
        expect: &[warning(
            "interrupted_operation",
            &[
                ("operation", Match::Eq("track archive")),
                ("command", Match::Eq("fr track archive side")),
                ("started", Match::Any),
            ],
        )],
        repair: Repair::Clears,
    },
    Case {
        name: "recovery-log-present",
        provenance: "any write that had to set content aside for review",
        covers: &["recovery_log"],
        build: |root| {
            frame::io::recovery::log_recovery(
                &root.join("frame"),
                frame::io::recovery::RecoveryEntry {
                    timestamp: chrono::Utc::now(),
                    category: frame::io::recovery::RecoveryCategory::Delete,
                    description: "a task removed by a corpus case".to_string(),
                    fields: vec![("Task".to_string(), "M-999".to_string())],
                    body: "- [ ] `M-999` gone".to_string(),
                },
            );
            Built::Ok
        },
        // Informational, not damage: it reports that a log exists, which is a
        // normal state for a project that has recovered anything.
        expect: &[info(
            "recovery_log",
            &[("entry_count", Match::Eq("1")), ("oldest", Match::Any)],
        )],
        repair: Repair::None,
    },
    // --- the track roster against the files ------------------------------
    Case {
        name: "track-file-missing",
        provenance: "the file was deleted, or a checkout landed project.toml \
                     without it — the track and its tasks then vanish from \
                     every view, because load_project skips a configured track \
                     whose file is absent",
        covers: &["track_file_missing"],
        build: |root| {
            fs::remove_file(track_path(root)).unwrap();
            Built::Ok
        },
        expect: &[error(
            "track_file_missing",
            &[
                ("track_id", Match::Eq("main")),
                ("path", Match::Eq("tracks/main.md")),
                ("state", Match::Eq("active")),
            ],
        )],
        // Dropping the entry discards a track a checkout may restore;
        // recreating the file fabricates content. Neither is a repair.
        repair: Repair::None,
    },
    Case {
        name: "track-file-unreferenced",
        provenance: "a track file arrived from a merge or a copy without its \
                     [[tracks]] entry — the tasks are real and nothing shows them",
        covers: &["track_file_unreferenced"],
        build: |root| {
            fs::write(
                root.join("frame/tracks/stray.md"),
                "# Stray Work\n\n## Backlog\n\n- [ ] `S-001` Invisible task\n  - added: 2026-01-01\n\n## Done\n",
            )
            .unwrap();
            Built::Ok
        },
        expect: &[error(
            "track_file_unreferenced",
            &[
                ("path", Match::Eq("tracks/stray.md")),
                ("title", Match::Eq("Stray Work")),
            ],
        )],
        // Adopting it invents an id, a name and a prefix — and when it is the
        // far half of a rename, the right answer is the original entry back.
        repair: Repair::None,
    },
    Case {
        name: "track-file-renamed-out-from-under-config",
        provenance: "`fr track rename --id` interrupted between the file rename \
                     and the config write, or a manual `mv` of a track file — \
                     the shape that motivated both detectors, and the one where \
                     `fr check` used to answer `✓ project is valid`",
        covers: &["track_file_missing", "track_file_unreferenced"],
        build: |root| {
            fs::rename(track_path(root), root.join("frame/tracks/renamed.md")).unwrap();
            Built::Ok
        },
        // Both halves, from the one rename: the entry points nowhere and the
        // file answers to nobody.
        expect: &[
            error(
                "track_file_missing",
                &[
                    ("track_id", Match::Eq("main")),
                    ("path", Match::Eq("tracks/main.md")),
                    ("state", Match::Eq("active")),
                ],
            ),
            error(
                "track_file_unreferenced",
                &[
                    ("path", Match::Eq("tracks/renamed.md")),
                    ("title", Match::Eq("Main")),
                ],
            ),
        ],
        repair: Repair::None,
    },
];

// ---------------------------------------------------------------------------
// The baseline
// ---------------------------------------------------------------------------

const PROJECT_TOML: &str = r#"[project]
name = "corpus"

[agent]
cc_focus = "main"

[[tracks]]
id = "main"
name = "Main"
state = "active"
file = "tracks/main.md"

[ids.prefixes]
main = "M"
"#;

const TRACK_MD: &str = "\
# Main

## Backlog

- [ ] `M-001` First task
  - added: 2026-01-01
- [ ] `M-002` Second task
  - added: 2026-01-01
  - dep: M-001

## Done

- [x] `M-003` Finished task
  - added: 2026-01-01
  - resolved: 2026-01-02
";

const INBOX_MD: &str = "# Inbox\n\n- An idea to triage\n";

/// The primary working copy's row. Without it the null token in `.actor` has no
/// registry entry and every case would carry an `actor_token_unregistered`
/// warning — a baseline that is *nearly* silent is no baseline at all.
const ACTORS_TOML: &str = "[actors.null]\nname = \"primary\"\nstate = \"active\"\n";

/// A project with nothing wrong with it. Every case starts here, so a finding a
/// case reports is attributable to the one thing that case broke.
fn baseline(root: &Path) {
    let frame_dir = root.join("frame");
    fs::create_dir_all(frame_dir.join("tracks")).unwrap();
    // As `fr init` records the primary working copy: mints stay in the null
    // namespace and nothing auto-claims a token.
    fs::write(frame_dir.join(".actor"), "null\n").unwrap();
    fs::write(frame_dir.join("actors.toml"), ACTORS_TOML).unwrap();
    fs::write(frame_dir.join("project.toml"), PROJECT_TOML).unwrap();
    fs::write(frame_dir.join("tracks/main.md"), TRACK_MD).unwrap();
    fs::write(frame_dir.join("inbox.md"), INBOX_MD).unwrap();
}

fn track_path(root: &Path) -> std::path::PathBuf {
    root.join("frame/tracks/main.md")
}

fn append_backlog(root: &Path, task: &str) {
    let path = track_path(root);
    let text = fs::read_to_string(&path)
        .unwrap()
        .replace("\n## Done", &format!("{task}\n## Done"));
    fs::write(&path, text).unwrap();
}

fn append_done(root: &Path, task: &str) {
    let path = track_path(root);
    let mut text = fs::read_to_string(&path).unwrap();
    text.push_str(task);
    fs::write(&path, text).unwrap();
}

fn write_archive(root: &Path, content: &str) {
    let dir = root.join("frame/archive");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("main.md"), content).unwrap();
}

/// Register frame's merge driver, so a case that needs a git repo is not also
/// reporting an unregistered driver. The corpus rule is that each case breaks
/// exactly one thing; the driver has its own case.
fn register_merge_driver(dir: &Path) -> bool {
    git(
        dir,
        &[
            "config",
            "merge.frame.driver",
            "fr merge --base %O --ours %A --theirs %B --path %P",
        ],
    )
}

fn git(dir: &Path, args: &[&str]) -> bool {
    Command::new("git")
        .current_dir(dir)
        .args(args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn the_baseline_is_silent() {
    let tmp = tempfile::TempDir::new().unwrap();
    baseline(tmp.path());
    let project = project_io::load_project(tmp.path()).unwrap();
    assert_findings("baseline", &[], &check_project(&project));
}

#[test]
fn every_case_reports_exactly_what_it_declares() {
    for case in CASES {
        let tmp = tempfile::TempDir::new().unwrap();
        baseline(tmp.path());
        if let Built::Skipped(why) = (case.build)(tmp.path()) {
            eprintln!("skipping `{}`: {why}", case.name);
            continue;
        }
        let project = project_io::load_project(tmp.path()).unwrap();
        assert_findings(case.name, case.expect, &check_project(&project));
    }
}

/// `Repair::None` is the assertion that matters here: it pins the findings
/// `fix.rs` deliberately leaves alone, so adding a repair means coming here and
/// saying so rather than a repair appearing silently.
#[test]
fn every_case_repairs_as_declared() {
    for case in CASES {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();
        baseline(root);
        if let Built::Skipped(why) = (case.build)(root) {
            eprintln!("skipping `{}`: {why}", case.name);
            continue;
        }

        let mut project = project_io::load_project(root).unwrap();
        let plan = fix::plan(&check_project(&project));

        match case.repair {
            Repair::None => assert!(
                plan.is_empty(),
                "case `{}` declares no repair, but `fix::plan` produced: {:?}\n\
                 If the repair is intended, change the case to Repair::Clears and \
                 record the reasoning in fix.rs's header.",
                case.name,
                plan.iter().map(fix::Repair::describe).collect::<Vec<_>>()
            ),
            Repair::Clears => {
                assert!(
                    !plan.is_empty(),
                    "case `{}` declares Repair::Clears but nothing was planned",
                    case.name
                );
                let result = fix::apply(&mut project, &plan);
                assert!(
                    result.skipped.is_empty(),
                    "case `{}`: repairs were skipped: {:?}",
                    case.name,
                    result
                        .skipped
                        .iter()
                        .map(|s| s.reason.clone())
                        .collect::<Vec<_>>()
                );
                save_touched(&project, &result);

                // Re-read from disk: the assertion is about what landed, not
                // about what we believe we wrote.
                let after = project_io::load_project(root).unwrap();
                assert_no_damage(case.name, &check_project(&after));

                // Idempotent — the property `fr clean`'s archive append had to
                // learn the hard way.
                assert!(
                    fix::plan(&check_project(&after)).is_empty(),
                    "case `{}`: a second --fix still has work to do",
                    case.name
                );
            }
        }
    }
}

/// Mirrors what `cmd_check` does after applying, so the corpus exercises the
/// same save path the CLI takes.
fn save_touched(project: &frame::model::project::Project, result: &fix::FixResult) {
    for track_id in fix::tracks_touched(result) {
        let Some(cfg) = project.config.tracks.iter().find(|t| t.id == track_id) else {
            continue;
        };
        let Some((_, track)) = project.tracks.iter().find(|(id, _)| *id == track_id) else {
            continue;
        };
        project_io::save_track(&project.frame_dir, &cfg.file, track).unwrap();
    }
    if fix::inbox_touched(result)
        && let Some(inbox) = &project.inbox
    {
        project_io::save_inbox(&project.frame_dir, inbox).unwrap();
    }
}

// ---------------------------------------------------------------------------
// The completeness guard
// ---------------------------------------------------------------------------

const CHECK_SRC: &str = include_str!("../src/ops/check.rs");

/// Every finding tag declared by `check.rs`, read out of the source itself
/// rather than a list kept in parallel with it — the same reason
/// `tests/parity.rs` checks against clap's own subcommand list.
fn declared_tags() -> BTreeSet<String> {
    let mut tags = BTreeSet::new();
    for enum_name in ["CheckError", "CheckWarning", "CheckInfo"] {
        let header = format!("pub enum {enum_name} {{");
        let start = CHECK_SRC
            .find(&header)
            .unwrap_or_else(|| panic!("`{header}` not found — did the enums move or get renamed?"))
            + header.len();

        let mut depth = 1usize;
        let mut end = start;
        for (i, c) in CHECK_SRC[start..].char_indices() {
            match c {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        end = start + i;
                        break;
                    }
                }
                _ => {}
            }
        }
        assert!(end > start, "unbalanced braces scanning {enum_name}");

        for line in CHECK_SRC[start..end].lines() {
            let line = line.trim();
            let Some(rest) = line.strip_prefix("#[serde(rename = \"") else {
                continue;
            };
            let Some(tag) = rest.strip_suffix("\")]") else {
                continue;
            };
            tags.insert(tag.to_string());
        }
    }
    tags
}

#[test]
fn the_tag_scrape_finds_a_plausible_set() {
    // A scrape that silently found nothing would make the guard below vacuous.
    let tags = declared_tags();
    assert!(
        tags.len() >= 20,
        "only {} finding tags scraped from check.rs — the scrape is probably \
         broken rather than the enums shrinking: {tags:?}",
        tags.len()
    );
    assert!(tags.contains("duplicate_id"), "sanity: {tags:?}");
    assert!(tags.contains("recovery_log"), "sanity: {tags:?}");
}

#[test]
fn every_finding_tag_has_a_case() {
    let declared = declared_tags();
    let covered: BTreeSet<String> = CASES
        .iter()
        .flat_map(|c| c.covers.iter().map(|t| t.to_string()))
        .collect();

    let missing: Vec<&String> = declared.difference(&covered).collect();
    assert!(
        missing.is_empty(),
        "finding(s) {missing:?} have no case in tests/damaged_corpus.rs.\n\
         Add a Case that builds a project exhibiting the damage, declares the \
         complete set of findings it produces, and states whether `--fix` \
         repairs it. A detector nobody has run against the other damage shapes \
         is how d0350a1 shipped."
    );

    let stale: Vec<&String> = covered.difference(&declared).collect();
    assert!(
        stale.is_empty(),
        "case(s) cover finding(s) {stale:?} that check.rs no longer declares — \
         remove the case, or fix the tag"
    );
}

#[test]
fn every_case_covers_what_it_declares() {
    for case in CASES {
        assert!(
            !case.provenance.is_empty(),
            "case `{}` has no provenance — say how this damage arises in the wild, \
             so a reader can tell a realistic case from an invented one",
            case.name
        );
        for tag in case.covers {
            assert!(
                case.expect.iter().any(|e| e.tag == *tag),
                "case `{}` claims to cover `{tag}` but does not expect it",
                case.name
            );
        }
    }
}
