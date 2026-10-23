//! The one projection between a task field and the flat string the detail-view
//! editor holds.
//!
//! # Why this is a module and not three copies
//!
//! Editing a field in the detail view runs three conversions: the field is
//! flattened into `edit_buffer` when the editor opens, parsed back when it
//! confirms, and parsed *again* by [`crate::tui::undo`] when the edit is undone.
//! Those lived in three places and disagreed. `ref:` was flattened with a space,
//! split on comma-**or**-whitespace, and undone by splitting on comma alone —
//! so a single ref containing a space (which the format allows, since `ref:` is
//! comma-separated) came apart into one ref per word on the way out, and came
//! back as one ref containing everything on undo. Opening the field and pressing
//! Enter, changing nothing, was enough:
//!
//! ```text
//! - ref: compiler/src/module_signature.rs:807, tests/…/bac178_… — this ticket's own red
//! - ref: compiler/src/module_signature.rs:807, tests/…/bac178_…, —, this, ticket's, own, red
//! ```
//!
//! The rule this module exists to keep is that `field_to_buffer` and
//! `apply_buffer` are **inverses on every value the format can represent**:
//!
//! ```text
//! apply_buffer(t, r, &field_to_buffer(t, r)) == false     // for all t, r
//! ```
//!
//! which is exactly what makes "open a field and close it again" a no-op, and
//! what makes an undo record faithful. `fields_round_trip_on_realizable_values`
//! asserts it, and the values it can't represent — a ref containing a comma, a
//! tag containing a space — are ones the parser cannot read back either.
//!
//! # Why `apply_buffer` returns whether it changed anything
//!
//! Because the buffer is a lossy view of the field, comparing buffer strings
//! cannot answer "did the user change this". It was the test the editor used,
//! and it is why the corruption above was also **invisible to undo**: the
//! before-and-after buffers were byte-identical, so no `Operation` was pushed,
//! while the field underneath had been rebuilt into something else. The save ran
//! unconditionally; only the undo record was conditional.
//!
//! So the comparison happens here instead, against the parsed field, and the one
//! answer drives both decisions — a field that did not change is not written and
//! not recorded. That also means a task whose file text is merely
//! *non-canonical* keeps its text: nothing marks it dirty, so the serializer
//! re-emits its source lines verbatim.

use crate::model::task::{Metadata, Task};
use crate::ops::task_ops;
use crate::tui::app::DetailRegion;

/// Deduplicate items while preserving first-occurrence order.
pub(crate) fn dedup_preserve_order(iter: impl Iterator<Item = String>) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    iter.filter(|s| seen.insert(s.clone())).collect()
}

/// The name this region carries in [`crate::tui::undo::Operation::FieldEdit`].
/// `Title` is absent on purpose: it has its own operation.
pub(crate) fn field_name(region: DetailRegion) -> Option<&'static str> {
    match region {
        DetailRegion::Tags => Some("tags"),
        DetailRegion::Deps => Some("deps"),
        DetailRegion::Spec => Some("spec"),
        DetailRegion::Refs => Some("refs"),
        DetailRegion::Note => Some("note"),
        _ => None,
    }
}

/// Inverse of [`field_name`].
pub(crate) fn region_from_field_name(name: &str) -> Option<DetailRegion> {
    match name {
        "tags" => Some(DetailRegion::Tags),
        "deps" => Some(DetailRegion::Deps),
        "spec" => Some(DetailRegion::Spec),
        "refs" => Some(DetailRegion::Refs),
        "note" => Some(DetailRegion::Note),
        "title" => Some(DetailRegion::Title),
        _ => None,
    }
}

/// Flatten a field into the string the editor edits.
pub(crate) fn field_to_buffer(task: &Task, region: DetailRegion) -> String {
    match region {
        DetailRegion::Title => task.title.clone(),
        DetailRegion::Tags => task
            .tags
            .iter()
            .map(|t| format!("#{}", t))
            .collect::<Vec<_>>()
            .join(" "),
        DetailRegion::Deps => collect_list(task, |m| match m {
            Metadata::Dep(d) => Some(d),
            _ => None,
        })
        .join(", "),
        DetailRegion::Spec => collect_list(task, |m| match m {
            Metadata::Spec(s) => Some(s),
            _ => None,
        })
        .join(", "),
        DetailRegion::Refs => collect_list(task, |m| match m {
            Metadata::Ref(r) => Some(r),
            _ => None,
        })
        .join(", "),
        DetailRegion::Note => task
            .metadata
            .iter()
            .find_map(|m| match m {
                Metadata::Note(n) => Some(n.clone()),
                _ => None,
            })
            .unwrap_or_default(),
        DetailRegion::Conflict
        | DetailRegion::Added
        | DetailRegion::Resolved
        | DetailRegion::Subtasks => String::new(),
    }
}

/// Parse `buffer` into `region`'s field and store it. Returns whether the task
/// changed; when it did not, the task is left clean and untouched.
pub(crate) fn apply_buffer(task: &mut Task, region: DetailRegion, buffer: &str) -> bool {
    match region {
        DetailRegion::Title => {
            // A blank title would leave a task nothing can name. Refusing it here
            // means Esc and Enter agree on an emptied title: both keep the old one.
            if buffer.trim().is_empty() {
                return false;
            }
            task_ops::set_title(task, buffer)
        }
        DetailRegion::Tags => {
            let new = parse_tags(buffer);
            if new == task.tags {
                return false;
            }
            task.tags = new;
            task.mark_dirty();
            true
        }
        DetailRegion::Deps => set_list(task, parse_ids(buffer), "dep", Metadata::Dep),
        DetailRegion::Spec => set_list(task, parse_paths(buffer), "spec", Metadata::Spec),
        DetailRegion::Refs => set_list(task, parse_paths(buffer), "ref", Metadata::Ref),
        DetailRegion::Note => {
            let current = field_to_buffer(task, DetailRegion::Note);
            let has_note = task.metadata.iter().any(|m| matches!(m, Metadata::Note(_)));
            if current == buffer && has_note == !buffer.is_empty() {
                return false;
            }
            if buffer.is_empty() {
                task.metadata.retain(|m| !matches!(m, Metadata::Note(_)));
            } else {
                task_ops::set_metadata(task, Metadata::Note(buffer.to_string()));
            }
            task.mark_dirty();
            true
        }
        DetailRegion::Conflict
        | DetailRegion::Added
        | DetailRegion::Resolved
        | DetailRegion::Subtasks => false,
    }
}

/// Tags are whitespace-separated, and can be neither empty nor contain a space,
/// so the flatten/parse pair is lossless without any separator subtlety.
fn parse_tags(buffer: &str) -> Vec<String> {
    dedup_preserve_order(
        buffer
            .split_whitespace()
            .map(|s| s.strip_prefix('#').unwrap_or(s).to_string())
            .filter(|s| !s.is_empty()),
    )
}

/// Task IDs, split leniently on commas **or** whitespace. Safe here and only
/// here: an ID cannot contain whitespace, so the lenient split is still the
/// inverse of `join(", ")`, and typing `EFF-1 EFF-2` keeps working.
fn parse_ids(buffer: &str) -> Vec<String> {
    dedup_preserve_order(
        buffer
            .split(|c: char| c == ',' || c.is_whitespace())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty()),
    )
}

/// `ref:` and `spec:` paths, split on commas **only** — the separator the file
/// format defines for them. Splitting on whitespace as well is what shattered a
/// ref carrying a note about why it is the ref; see this module's header.
///
/// Each is folded to its normal form, exactly as `fr ref add` stores it, so both
/// surfaces write one spelling per file. That also makes the dedup below a dedup
/// by *file* rather than by string: `real.md` and `./real.md` typed into the same
/// list collapse to one entry, which is what `add` does on the CLI side.
///
/// Folding is where the TUI's agreement with the CLI stops. The editor does not
/// refuse a path that leaves the project or that git ignores — there is no
/// `--force` to offer and no good answer for what to do with the text someone
/// just typed — so `render::detail_view` paints those red and `fr check` reports
/// them. `tests/parity.rs` carries that divergence as a stated one.
fn parse_paths(buffer: &str) -> Vec<String> {
    dedup_preserve_order(
        buffer
            .split(',')
            .map(crate::ops::refs::normalize)
            .filter(|s| !s.is_empty()),
    )
}

fn collect_list<'a>(
    task: &'a Task,
    pick: impl Fn(&'a Metadata) -> Option<&'a Vec<String>>,
) -> Vec<String> {
    task.metadata
        .iter()
        .filter_map(pick)
        .flat_map(|v| v.iter().cloned())
        .collect()
}

fn set_list(
    task: &mut Task,
    new: Vec<String>,
    key: &str,
    wrap: impl Fn(Vec<String>) -> Metadata,
) -> bool {
    let current: Vec<String> = task
        .metadata
        .iter()
        .filter(|m| m.key() == key)
        .flat_map(|m| match m {
            Metadata::Dep(v) | Metadata::Ref(v) | Metadata::Spec(v) => v.clone(),
            _ => Vec::new(),
        })
        .collect();
    if current == new {
        return false;
    }
    if new.is_empty() {
        task.metadata.retain(|m| m.key() != key);
    } else {
        task_ops::set_metadata(task, wrap(new));
    }
    task.mark_dirty();
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::task::TaskState;

    const EDITABLE: [DetailRegion; 6] = [
        DetailRegion::Title,
        DetailRegion::Tags,
        DetailRegion::Deps,
        DetailRegion::Spec,
        DetailRegion::Refs,
        DetailRegion::Note,
    ];

    fn task_with(metadata: Vec<Metadata>) -> Task {
        let mut t = Task::new(TaskState::Todo, None, "A task".into());
        t.tags = vec!["design".into(), "urgent".into()];
        t.metadata = metadata;
        t.dirty = false;
        t
    }

    /// The ref from the bug report: two refs, the second carrying prose with
    /// spaces, an em-dash and parentheses.
    fn prose_ref_task() -> Task {
        task_with(vec![Metadata::Ref(vec![
            "compiler/src/module_signature.rs:807".into(),
            "tests/integration/name-resolution/bac178_modqual — this ticket's own red (first fixture)"
                .into(),
        ])])
    }

    #[test]
    fn fields_round_trip_on_realizable_values() {
        let tasks = [
            prose_ref_task(),
            task_with(vec![]),
            task_with(vec![
                Metadata::Dep(vec!["EFF-003".into(), "MOD-007".into()]),
                Metadata::Spec(vec!["doc/spec.md#anchor".into(), "doc/other.md".into()]),
                Metadata::Ref(vec!["a.md".into()]),
                Metadata::Note("line one\nline two".into()),
            ]),
            task_with(vec![Metadata::Spec(vec!["one path with spaces.md".into()])]),
        ];

        for task in tasks {
            for region in EDITABLE {
                let buffer = field_to_buffer(&task, region);
                let mut copy = task.clone();
                let changed = apply_buffer(&mut copy, region, &buffer);
                assert!(
                    !changed,
                    "{:?} reported a change for an untouched buffer {:?}",
                    region, buffer
                );
                assert!(!copy.dirty, "{:?} dirtied an unchanged task", region);
                assert_eq!(
                    copy.metadata, task.metadata,
                    "{:?} rewrote metadata",
                    region
                );
                assert_eq!(copy.tags, task.tags, "{:?} rewrote tags", region);
                assert_eq!(copy.title, task.title, "{:?} rewrote the title", region);
            }
        }
    }

    #[test]
    fn a_ref_carrying_prose_survives_the_buffer() {
        let task = prose_ref_task();
        let buffer = field_to_buffer(&task, DetailRegion::Refs);
        let mut copy = task.clone();
        apply_buffer(&mut copy, DetailRegion::Refs, &buffer);
        assert_eq!(copy.metadata, task.metadata);
    }

    #[test]
    fn editing_one_ref_leaves_the_others_verbatim() {
        let task = prose_ref_task();
        let buffer = field_to_buffer(&task, DetailRegion::Refs);
        let edited = buffer.replace("module_signature.rs:807", "module_signature.rs:900");
        let mut copy = task.clone();
        assert!(apply_buffer(&mut copy, DetailRegion::Refs, &edited));

        let Some(Metadata::Ref(refs)) = copy.metadata.first() else {
            panic!("expected a ref");
        };
        assert_eq!(refs[0], "compiler/src/module_signature.rs:900");
        let Some(Metadata::Ref(original)) = task.metadata.first() else {
            unreachable!()
        };
        assert_eq!(refs[1], original[1], "the untouched ref was reformatted");
    }

    #[test]
    fn a_field_keeps_its_position_when_edited() {
        let mut task = task_with(vec![
            Metadata::Ref(vec!["a.md".into()]),
            Metadata::Note("some note".into()),
        ]);
        assert!(apply_buffer(&mut task, DetailRegion::Refs, "b.md"));
        assert_eq!(task.metadata[0], Metadata::Ref(vec!["b.md".into()]));
        assert_eq!(task.metadata[1], Metadata::Note("some note".into()));
    }

    #[test]
    fn clearing_a_field_removes_it() {
        let mut task = task_with(vec![Metadata::Ref(vec!["a.md".into()])]);
        assert!(apply_buffer(&mut task, DetailRegion::Refs, "  "));
        assert!(task.metadata.is_empty());
        // ...and clearing an absent field is not a change.
        assert!(!apply_buffer(&mut task, DetailRegion::Refs, ""));
    }

    #[test]
    fn an_emptied_title_is_refused() {
        let mut task = task_with(vec![]);
        assert!(!apply_buffer(&mut task, DetailRegion::Title, "   "));
        assert_eq!(task.title, "A task");
        assert!(!task.dirty);
    }

    #[test]
    fn deps_still_accept_whitespace_separated_input() {
        let mut task = task_with(vec![]);
        assert!(apply_buffer(
            &mut task,
            DetailRegion::Deps,
            "EFF-003 MOD-007"
        ));
        assert_eq!(
            task.metadata[0],
            Metadata::Dep(vec!["EFF-003".into(), "MOD-007".into()])
        );
    }

    #[test]
    fn duplicate_entries_collapse() {
        let mut task = task_with(vec![]);
        assert!(apply_buffer(
            &mut task,
            DetailRegion::Refs,
            "a.md, a.md, b.md"
        ));
        assert_eq!(
            task.metadata[0],
            Metadata::Ref(vec!["a.md".into(), "b.md".into()])
        );
    }

    /// The editor stores the same spelling `fr ref add` does, so a path written
    /// in one surface is the string the other one matches against.
    #[test]
    fn paths_are_stored_in_normal_form() {
        for region in [DetailRegion::Refs, DetailRegion::Spec] {
            let mut task = task_with(vec![]);
            assert!(apply_buffer(
                &mut task,
                region,
                "./sub/../real.md, ./doc/../design.md#why"
            ));
            let stored = match &task.metadata[0] {
                Metadata::Ref(v) | Metadata::Spec(v) => v.clone(),
                other => panic!("unexpected metadata: {other:?}"),
            };
            assert_eq!(stored, vec!["real.md", "design.md#why"], "{region:?}");
        }
    }

    /// Folding makes the dedup a dedup by *file*, not by string — the same thing
    /// `fr ref add` does when it declines to append a spelling it already holds.
    #[test]
    fn two_spellings_of_one_file_collapse() {
        let mut task = task_with(vec![]);
        assert!(apply_buffer(
            &mut task,
            DetailRegion::Refs,
            "real.md, ./sub/../real.md, other.md"
        ));
        assert_eq!(
            task.metadata[0],
            Metadata::Ref(vec!["real.md".into(), "other.md".into()])
        );
    }

    /// A name that merely contains `..` or a suffix is not a traversal.
    #[test]
    fn folding_leaves_ordinary_paths_alone() {
        let mut task = task_with(vec![]);
        assert!(apply_buffer(
            &mut task,
            DetailRegion::Refs,
            "doc/..hidden.md, src/parser.rs:807-820, doc/issue#3.md"
        ));
        assert_eq!(
            task.metadata[0],
            Metadata::Ref(vec![
                "doc/..hidden.md".into(),
                "src/parser.rs:807-820".into(),
                "doc/issue#3.md".into()
            ])
        );
    }

    #[test]
    fn field_names_round_trip() {
        for region in EDITABLE {
            if let Some(name) = field_name(region) {
                assert_eq!(region_from_field_name(name), Some(region));
            }
        }
    }
}

// ---------------------------------------------------------------------------
// External-editor template
// ---------------------------------------------------------------------------

/// Build the external-editor template for `task`: a header of `key: value`
/// lines, separator lines, and the note body below.
///
/// With `hints`, `#` hint lines follow the header, including all project tags
/// from `available_tags` — they answer "what tags can I use?". Parse strips them.
pub(crate) fn template_for_task(task: &Task, hints: bool, available_tags: &[String]) -> String {
    let mut out = String::new();
    if let Some(id) = &task.id {
        out.push_str(&id.to_string());
        out.push('\n');
    }
    out.push_str("---\n");
    for (key, region) in [
        ("title", DetailRegion::Title),
        ("tags", DetailRegion::Tags),
        ("deps", DetailRegion::Deps),
        ("spec", DetailRegion::Spec),
        ("refs", DetailRegion::Refs),
    ] {
        out.push_str(&format!("{key}: {}\n", field_to_buffer(task, region)));
    }
    if hints {
        out.push_str("# Available attributes — edit the values, keep the keys.\n");
        out.push_str(
            "#   title: <text> <#tag1> <#tag2>   (a #tag in the title is added to tags)\n",
        );
        out.push_str("#   tags: <#tag #tag ...>\n");
        out.push_str("#   deps: <T-001, T-002 ...>\n");
        out.push_str("#   spec: <path#anchor, path ...>\n");
        out.push_str("#   refs: <path:line, path ...>\n");
        if !available_tags.is_empty() {
            let tags = available_tags
                .iter()
                .map(|t| format!("#{t}"))
                .collect::<Vec<_>>()
                .join(" ");
            out.push_str(&format!("# available tags: {tags}\n"));
        }
        out.push_str("# Leave a value empty to remove that attribute.\n");
        out.push_str("# Lines starting with \"#\" are hints — they are removed on save.\n");
    }
    out.push_str("---\n");
    out.push_str(&field_to_buffer(task, DetailRegion::Note));
    out.push('\n');
    out
}

/// Errors from parsing an external-editor template.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TemplateError {
    /// A required field line is missing.
    MissingField(String),
    /// A field key is not a known editable attribute.
    UnknownField(String),
    /// A line is not `key: value`, or a separator is missing.
    BadFormat(String),
    /// The title was emptied.
    EmptyTitle,
}

impl std::fmt::Display for TemplateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TemplateError::MissingField(key) => write!(f, "missing field `{key}`"),
            TemplateError::UnknownField(key) => write!(f, "unknown attribute `{key}`"),
            TemplateError::BadFormat(msg) => write!(f, "{msg}"),
            TemplateError::EmptyTitle => write!(f, "title cannot be empty"),
        }
    }
}

/// A parsed external-editor template: per-field buffers in apply order, plus
/// the note body below the second separator.
pub(crate) type ParsedTemplate = (Vec<(DetailRegion, String)>, String);

/// Parse the external-editor template. Lines starting with `#` are stripped;
/// every known key must be present; `note` lives below the second `---`.
pub(crate) fn parse_template(text: &str) -> Result<ParsedTemplate, TemplateError> {
    let lines: Vec<&str> = text.lines().collect();
    let first = lines
        .iter()
        .position(|l| l.trim() == "---")
        .ok_or_else(|| TemplateError::BadFormat("missing first `---` separator".into()))?;
    let after = &lines[first + 1..];
    let second = after
        .iter()
        .position(|l| l.trim() == "---")
        .ok_or_else(|| TemplateError::BadFormat("missing second `---` separator".into()))?;
    let header = &after[..second];
    let note = after[second + 1..].join("\n");

    let mut fields: Vec<(DetailRegion, String)> = Vec::new();
    for raw in header {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once(':') else {
            return Err(TemplateError::BadFormat(format!(
                "line is not `key: value`: {line}"
            )));
        };
        let key = key.trim();
        if key == "note" {
            return Err(TemplateError::BadFormat(
                "the note goes below the second `---`, not as a header field".into(),
            ));
        }
        let region = region_from_field_name(key)
            .ok_or_else(|| TemplateError::UnknownField(key.to_string()))?;
        if !region.is_editable() {
            return Err(TemplateError::UnknownField(key.to_string()));
        }
        let value = value.trim().to_string();
        // Last occurrence wins, so a duplicated key cannot double-apply.
        if let Some(slot) = fields.iter_mut().find(|(r, _)| r == &region) {
            slot.1 = value;
        } else {
            fields.push((region, value));
        }
    }

    for key in ["title", "tags", "deps", "spec", "refs"] {
        let region = region_from_field_name(key).unwrap();
        if !fields.iter().any(|(r, _)| r == &region) {
            return Err(TemplateError::MissingField(key.to_string()));
        }
    }
    if fields
        .iter()
        .any(|(r, v)| *r == DetailRegion::Title && v.trim().is_empty())
    {
        return Err(TemplateError::EmptyTitle);
    }

    Ok((fields, note))
}

/// The fields a template sets, in the order they are applied. Tags before
/// title: a `#tag` typed into the title joins the tags, instead of being
/// overwritten by the `tags:` line.
pub(crate) const TEMPLATE_APPLY_ORDER: [DetailRegion; 6] = [
    DetailRegion::Tags,
    DetailRegion::Title,
    DetailRegion::Deps,
    DetailRegion::Spec,
    DetailRegion::Refs,
    DetailRegion::Note,
];

/// Apply every field of a parsed template to `task`. Returns whether it changed.
pub(crate) fn apply_template(task: &mut Task, (header, note): &ParsedTemplate) -> bool {
    let mut changed = false;
    for region in TEMPLATE_APPLY_ORDER {
        let buffer = match region {
            DetailRegion::Note => note.as_str(),
            // `parse_template` guarantees every header field is present.
            _ => header
                .iter()
                .find(|(r, _)| *r == region)
                .map_or("", |(_, v)| v.as_str()),
        };
        changed |= apply_buffer(task, region, buffer);
    }
    changed
}

#[cfg(test)]
mod ext_template_tests {
    use super::*;
    use crate::model::task::{Task, TaskState};

    fn sample_task() -> Task {
        let mut t = Task::new(
            TaskState::Todo,
            Some(crate::model::task_id::TaskId::parse("T-001")),
            "Fix auth bug".to_string(),
        );
        t.tags = vec!["design".to_string(), "urgent".to_string()];
        t.metadata = vec![Metadata::Dep(vec![
            "T-002".to_string(),
            "T-003".to_string(),
        ])];
        t
    }

    fn field(parsed: &ParsedTemplate, region: DetailRegion) -> &str {
        parsed
            .0
            .iter()
            .find(|(r, _)| *r == region)
            .map(|(_, v)| v.as_str())
            .unwrap()
    }

    #[test]
    fn round_trip_preserves_every_field() {
        let task = sample_task();
        let parsed = parse_template(&template_for_task(&task, true, &[])).unwrap();
        assert_eq!(field(&parsed, DetailRegion::Title), "Fix auth bug");
        assert_eq!(field(&parsed, DetailRegion::Tags), "#design #urgent");
        assert_eq!(field(&parsed, DetailRegion::Deps), "T-002, T-003");
        assert_eq!(field(&parsed, DetailRegion::Spec), "");
        assert_eq!(field(&parsed, DetailRegion::Refs), "");
        assert_eq!(parsed.1, "");
    }

    #[test]
    fn hint_lines_are_stripped() {
        let task = sample_task();
        let template = template_for_task(&task, true, &["urgent".to_string()]);
        assert!(template.contains("# available tags: #urgent"));
        let parsed = parse_template(&template).unwrap();
        assert_eq!(parsed.0.len(), 5, "hints must not become fields");
    }

    #[test]
    fn hints_can_be_turned_off() {
        let task = sample_task();
        let with = template_for_task(&task, true, &["urgent".to_string()]);
        assert!(with.contains("#   title: <text> <#tag1> <#tag2>"));
        let without = template_for_task(&task, false, &["urgent".to_string()]);
        assert!(!without.lines().any(|l| l.starts_with('#')), "{without}");
        assert_eq!(
            parse_template(&with).unwrap(),
            parse_template(&without).unwrap()
        );
    }

    #[test]
    fn note_body_preserves_hash_lines() {
        let text = "T-001\n---\ntitle: x\ntags: \ndeps: \nspec: \nrefs: \n---\n# heading\nbody\n# another\n";
        let parsed = parse_template(text).unwrap();
        assert_eq!(parsed.1, "# heading\nbody\n# another");
    }

    #[test]
    fn unknown_field_is_rejected() {
        let text = "---\ntitle: x\ntags: \ndeps: \nspec: \nrefs: \nbogus: y\n---\n";
        assert!(matches!(
            parse_template(text),
            Err(TemplateError::UnknownField(k)) if k == "bogus"
        ));
    }

    #[test]
    fn missing_field_is_rejected() {
        let text = "---\ntitle: x\ntags: \ndeps: \nspec: \n---\n";
        assert!(matches!(
            parse_template(text),
            Err(TemplateError::MissingField(k)) if k == "refs"
        ));
    }

    #[test]
    fn note_as_header_field_is_rejected() {
        let text = "---\ntitle: x\ntags: \ndeps: \nspec: \nrefs: \nnote: y\n---\n";
        assert!(matches!(
            parse_template(text),
            Err(TemplateError::BadFormat(_))
        ));
    }

    #[test]
    fn empty_title_is_rejected() {
        let text = "---\ntitle:  \ntags: \ndeps: \nspec: \nrefs: \n---\n";
        assert_eq!(parse_template(text), Err(TemplateError::EmptyTitle));
    }

    #[test]
    fn duplicate_key_last_wins() {
        let text = "---\ntitle: first\ntitle: second\ntags: \ndeps: \nspec: \nrefs: \n---\n";
        let parsed = parse_template(text).unwrap();
        assert_eq!(field(&parsed, DetailRegion::Title), "second");
        assert_eq!(parsed.0.len(), 5);
    }

    #[test]
    fn empty_value_clears_a_field() {
        let text = "---\ntitle: x\ntags: \ndeps: \nspec: \nrefs: \n---\n";
        let parsed = parse_template(text).unwrap();
        assert_eq!(field(&parsed, DetailRegion::Tags), "");
    }
}
