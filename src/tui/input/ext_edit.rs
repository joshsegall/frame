use std::io;

use crossterm::event::{
    DisableBracketedPaste, EnableBracketedPaste, KeyboardEnhancementFlags,
    PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};

use super::common::drain_pending_for_track;
use crate::io::editor::{self, Edited};
use crate::ops::task_ops;
use crate::tui::app::{App, DetailRegion, SaveTarget, View};
use crate::tui::fields::{self, ParsedTemplate};
use crate::tui::undo::Operation;

/// The task under the cursor (or in Detail view) as `(track_id, task_id)`.
fn target_task(app: &App) -> Option<(String, String)> {
    match &app.view {
        View::Detail { track_id, task_id } => {
            let subtask_id = app.detail_state.as_ref().and_then(|ds| {
                if ds.region == DetailRegion::Subtasks {
                    ds.flat_subtask_ids.get(ds.subtask_cursor).cloned()
                } else {
                    None
                }
            });
            Some((
                track_id.clone(),
                subtask_id.unwrap_or_else(|| task_id.clone()),
            ))
        }
        View::Board => app.board_cursor_task_id(),
        _ => {
            let (track_id, task_id, _) = app.cursor_task_id()?;
            Some((track_id, task_id))
        }
    }
}

/// Suspend the TUI terminal state, run `f` (which should spawn the editor),
/// then restore it. Mirrors the restore sequence in `app::run`'s exit path.
fn with_terminal_suspended<T>(f: impl FnOnce() -> T) -> T {
    let _ = disable_raw_mode();
    let _ = execute!(io::stdout(), DisableBracketedPaste);
    let _ = execute!(io::stdout(), PopKeyboardEnhancementFlags);
    let _ = execute!(io::stdout(), LeaveAlternateScreen);
    let result = f();
    let _ = execute!(io::stdout(), EnterAlternateScreen);
    let _ = execute!(io::stdout(), EnableBracketedPaste);
    let _ = execute!(
        io::stdout(),
        PushKeyboardEnhancementFlags(
            KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
                | KeyboardEnhancementFlags::REPORT_EVENT_TYPES
                | KeyboardEnhancementFlags::REPORT_ALL_KEYS_AS_ESCAPE_CODES
        )
    );
    let _ = enable_raw_mode();
    result
}

/// Edit the task under the cursor in an external editor. On save, the template
/// is parsed; invalid markup (unknown attribute, missing field, empty title)
/// reopens the editor on the user's text with the error pinned at the top.
/// A non-zero editor exit, or saving the file unchanged or empty, cancels.
pub(super) fn edit_in_external_editor(app: &mut App) {
    let Some((track_id, task_id)) = target_task(app) else {
        return;
    };
    let hints = app.project.config.ui.editor_hints.unwrap_or(true);
    let tags = if hints {
        app.collect_all_tags()
    } else {
        Vec::new()
    };
    let Some(base) = App::find_track_in_project(&app.project, &track_id)
        .and_then(|track| task_ops::find_task_in_track(track, &task_id))
        .map(|task| fields::template_for_task(task, hints, &tags))
    else {
        return;
    };

    let edited = editor::edit_until_valid(
        &base,
        |run| with_terminal_suspended(run),
        fields::parse_template,
    );
    let (parsed, tmp) = match edited {
        Ok(Edited::Saved(parsed, tmp)) => (parsed, tmp),
        Ok(Edited::Unchanged) => {
            app.status_message = Some("no changes from external editor".to_string());
            return;
        }
        Ok(Edited::Cancelled) => {
            app.status_message = Some("external edit cancelled".to_string());
            return;
        }
        Err(e) => {
            app.status_message = Some(format!("external edit failed: {e}"));
            app.status_is_error = true;
            return;
        }
    };

    if !commit_template(app, &track_id, &task_id, &parsed) {
        // The temp file holds the only copy of the user's text; don't delete it.
        let kept = tmp
            .keep()
            .map(|(_, p)| p.display().to_string())
            .unwrap_or_else(|e| format!("nowhere ({e})"));
        app.status_message = Some(format!(
            "{task_id} is no longer in the project; your edit is kept in {kept}"
        ));
        app.status_is_error = true;
    }
}

/// Apply a parsed template to the task, save, and record one undo step.
/// False if the task is no longer there.
fn commit_template(app: &mut App, track_id: &str, task_id: &str, parsed: &ParsedTemplate) -> bool {
    // The editor was open for as long as the user liked. Start from what is on
    // disk, so another writer's work lands under this edit, not in the recovery
    // log. Unless this session holds content that never reached disk.
    if app.track_changed_on_disk(track_id)
        && !app
            .unsaved
            .contains_key(&SaveTarget::Track(track_id.to_string()))
    {
        app.adopt_track_from_disk(track_id);
    }
    let Some((changed, ops)) = apply_template(app, track_id, task_id, parsed) else {
        return false;
    };
    if !changed {
        app.status_message = Some("no changes from external editor".to_string());
        return true;
    }
    // Before the save, so a save failure's message is the one left showing.
    app.status_message = Some("task updated from external editor".to_string());
    app.save_track_logged(track_id);
    push_undo(app, ops);
    drain_pending_for_track(app, track_id);
    true
}

/// Apply every field of `parsed` to the task. `None` if the task is gone;
/// otherwise whether anything changed, and the undo records for it.
fn apply_template(
    app: &mut App,
    track_id: &str,
    task_id: &str,
    parsed: &ParsedTemplate,
) -> Option<(bool, Vec<Operation>)> {
    let task = task_ops::find_task_mut_in_track(app.find_track_mut(track_id)?, task_id)?;
    let before = fields::TEMPLATE_APPLY_ORDER.map(|r| fields::field_to_buffer(task, r));
    let changed = fields::apply_template(task, parsed);

    // Compared after every field is applied, so the tags record includes any
    // tag the title contributed.
    let mut ops = Vec::new();
    for (region, old) in fields::TEMPLATE_APPLY_ORDER.into_iter().zip(before) {
        let new = fields::field_to_buffer(task, region);
        if new == old {
            continue;
        }
        ops.push(match fields::field_name(region) {
            Some(field) => Operation::FieldEdit {
                track_id: track_id.to_string(),
                task_id: task_id.to_string(),
                field: field.to_string(),
                old_value: old,
                new_value: new,
            },
            None => Operation::TitleEdit {
                track_id: track_id.to_string(),
                task_id: task_id.to_string(),
                old_title: old,
                new_title: new,
            },
        });
    }
    Some((changed, ops))
}

/// One editor session is one undo step.
fn push_undo(app: &mut App, mut ops: Vec<Operation>) {
    match ops.len() {
        0 => {}
        1 => app.undo_stack.push(ops.remove(0)),
        _ => app.undo_stack.push(Operation::Bulk(ops)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::app::app_on_disk;
    use crate::tui::input::common::perform_undo;

    const TEMPLATE: &str =
        "---\ntitle: {title}\ntags: {tags}\ndeps: \nspec: \nrefs: \n---\n{note}\n";

    fn parsed(title: &str, tags: &str, note: &str) -> ParsedTemplate {
        let text = TEMPLATE
            .replace("{title}", title)
            .replace("{tags}", tags)
            .replace("{note}", note);
        fields::parse_template(&text).unwrap()
    }

    fn task<'a>(app: &'a App, id: &str) -> Option<&'a crate::model::task::Task> {
        task_ops::find_task_in_track(App::find_track_in_project(&app.project, "a")?, id)
    }

    #[test]
    fn tag_typed_into_title_is_kept() {
        let tmp = tempfile::tempdir().unwrap();
        let mut app = app_on_disk(tmp.path());
        assert!(commit_template(
            &mut app,
            "a",
            "A-001",
            &parsed("One #foo", "", "")
        ));
        let t = task(&app, "A-001").unwrap();
        assert_eq!(t.title, "One");
        assert_eq!(t.tags, vec!["foo".to_string()]);
    }

    #[test]
    fn one_undo_reverts_the_whole_edit() {
        let tmp = tempfile::tempdir().unwrap();
        let mut app = app_on_disk(tmp.path());
        assert!(commit_template(
            &mut app,
            "a",
            "A-001",
            &parsed("Two", "#x", "a note")
        ));
        perform_undo(&mut app);
        let t = task(&app, "A-001").unwrap();
        assert_eq!(t.title, "One");
        assert!(t.tags.is_empty());
        assert_eq!(fields::field_to_buffer(t, DetailRegion::Note), "");
    }

    #[test]
    fn write_made_while_editing_is_kept() {
        let tmp = tempfile::tempdir().unwrap();
        let mut app = app_on_disk(tmp.path());
        let path = tmp.path().join("frame/tracks/a.md");
        std::fs::write(
            &path,
            "# A\n\n## Backlog\n\n- [ ] `A-001` One\n- [ ] `A-002` Other\n\n## Done\n",
        )
        .unwrap();
        // Make the change visible to an mtime check on any filesystem.
        let later = std::time::SystemTime::now() + std::time::Duration::from_secs(5);
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(later)
            .unwrap();

        assert!(commit_template(
            &mut app,
            "a",
            "A-001",
            &parsed("Renamed", "", "")
        ));
        let disk = std::fs::read_to_string(&path).unwrap();
        assert!(disk.contains("`A-001` Renamed"), "{disk}");
        assert!(disk.contains("`A-002` Other"), "{disk}");
    }

    #[test]
    fn missing_task_is_reported() {
        let tmp = tempfile::tempdir().unwrap();
        let mut app = app_on_disk(tmp.path());
        assert!(!commit_template(
            &mut app,
            "a",
            "A-999",
            &parsed("x", "", "")
        ));
    }
}
