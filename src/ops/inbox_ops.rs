use chrono::Local;

use crate::model::inbox::{Inbox, InboxItem};
use crate::model::task::{Metadata, Task, TaskState};
use crate::model::track::{SectionKind, Track, TrackNode};
use crate::ops::task_ops::{InsertPosition, TaskError};

/// Error type for inbox operations
#[derive(Debug, thiserror::Error)]
pub enum InboxError {
    #[error("inbox item index out of range: {0}")]
    IndexOutOfRange(usize),
    #[error("no inbox loaded")]
    NoInbox,
    #[error("task error: {0}")]
    TaskError(#[from] TaskError),
}

/// Add a new item to the inbox.
pub fn add_inbox_item(inbox: &mut Inbox, title: String, tags: Vec<String>, body: Option<String>) {
    let mut item = InboxItem::new(title);
    item.tags = tags;
    item.body = body;
    inbox.items.push(item);
}

/// Remove an inbox item by index (0-based) and triage it into a track.
/// Returns the newly assigned task ID.
pub fn triage(
    inbox: &mut Inbox,
    index: usize,
    track: &mut Track,
    position: InsertPosition,
    prefix: &str,
) -> Result<String, InboxError> {
    if index >= inbox.items.len() {
        return Err(InboxError::IndexOutOfRange(index));
    }

    // Validate destination exists BEFORE removing from inbox
    let has_backlog = track.nodes.iter().any(|n| {
        matches!(
            n,
            TrackNode::Section {
                kind: SectionKind::Backlog,
                ..
            }
        )
    });
    if !has_backlog {
        return Err(InboxError::TaskError(TaskError::InvalidPosition(
            "no backlog section".into(),
        )));
    }
    if let InsertPosition::After(after_id) = &position {
        let found = track
            .section_tasks(SectionKind::Backlog)
            .iter()
            .any(|t| t.id.as_deref() == Some(after_id.as_str()));
        if !found {
            return Err(InboxError::TaskError(TaskError::NotFound(format!(
                "after target {}",
                after_id
            ))));
        }
    }

    // Now safe to remove — destination is validated
    let item = inbox.items.remove(index);

    // Build the task from the inbox item
    let next_num = next_id_for_track(track, prefix);
    let id = format!("{}-{:03}", prefix, next_num);

    let mut task = Task::new(TaskState::Todo, Some(id.clone()), item.title);
    task.tags = item.tags;
    task.metadata.push(Metadata::Added(today_str()));

    // Carry over body as a note
    if let Some(body) = item.body
        && !body.is_empty()
    {
        task.metadata.push(Metadata::Note(body));
    }

    let tasks = track
        .section_tasks_mut(SectionKind::Backlog)
        .expect("backlog section validated above");

    match &position {
        InsertPosition::Bottom => tasks.push(task),
        InsertPosition::Top => tasks.insert(0, task),
        InsertPosition::After(after_id) => {
            let idx = tasks
                .iter()
                .position(|t| t.id.as_deref() == Some(after_id.as_str()))
                .expect("after target validated above");
            tasks.insert(idx + 1, task);
        }
    }

    Ok(id)
}

fn today_str() -> String {
    Local::now().format("%Y-%m-%d").to_string()
}

fn next_id_for_track(track: &Track, prefix: &str) -> usize {
    let mut max = 0usize;
    let prefix_dash = format!("{}-", prefix);
    crate::ops::task_ops::find_max_id_in_track(track, &prefix_dash, &mut max);
    max + 1
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::{parse_inbox, parse_track};

    fn sample_inbox() -> Inbox {
        parse_inbox(
            "\
# Inbox

- Parser crash on empty blocks #bug
  Saw this when testing.

- Think about perform semantics #design

- Quick note
",
        )
        .0
    }

    fn sample_track() -> Track {
        parse_track(
            "\
# Test

## Backlog

- [ ] `T-001` First task
- [ ] `T-002` Second task

## Done
",
        )
    }

    #[test]
    fn test_add_inbox_item() {
        let mut inbox = sample_inbox();
        assert_eq!(inbox.items.len(), 3);
        add_inbox_item(
            &mut inbox,
            "New item".into(),
            vec!["bug".into()],
            Some("Details here.".into()),
        );
        assert_eq!(inbox.items.len(), 4);
        assert_eq!(inbox.items[3].title, "New item");
        assert_eq!(inbox.items[3].tags, vec!["bug"]);
        assert_eq!(inbox.items[3].body.as_deref(), Some("Details here."));
    }

    #[test]
    fn test_triage_bottom() {
        let mut inbox = sample_inbox();
        let mut track = sample_track();
        let id = triage(&mut inbox, 0, &mut track, InsertPosition::Bottom, "T").unwrap();
        assert_eq!(id, "T-003");
        assert_eq!(inbox.items.len(), 2);
        let tasks = track.backlog();
        assert_eq!(tasks.len(), 3);
        assert_eq!(tasks[2].title, "Parser crash on empty blocks");
        assert!(tasks[2].tags.contains(&"bug".to_string()));
        // Body should become a note
        assert!(
            tasks[2]
                .metadata
                .iter()
                .any(|m| matches!(m, Metadata::Note(n) if n.contains("Saw this")))
        );
    }

    #[test]
    fn test_triage_top() {
        let mut inbox = sample_inbox();
        let mut track = sample_track();
        let id = triage(&mut inbox, 1, &mut track, InsertPosition::Top, "T").unwrap();
        assert_eq!(id, "T-003");
        assert_eq!(track.backlog()[0].title, "Think about perform semantics");
    }

    #[test]
    fn test_triage_after() {
        let mut inbox = sample_inbox();
        let mut track = sample_track();
        let id = triage(
            &mut inbox,
            2,
            &mut track,
            InsertPosition::After("T-001".into()),
            "T",
        )
        .unwrap();
        assert_eq!(id, "T-003");
        assert_eq!(track.backlog()[1].title, "Quick note");
    }

    #[test]
    fn test_triage_out_of_range() {
        let mut inbox = sample_inbox();
        let mut track = sample_track();
        let result = triage(&mut inbox, 10, &mut track, InsertPosition::Bottom, "T");
        assert!(result.is_err());
    }

    #[test]
    fn test_triage_no_backlog_preserves_inbox() {
        let mut inbox = sample_inbox();
        let original_len = inbox.items.len();
        // Track with no Backlog section
        let mut track = parse_track(
            "\
# Test

## Done
",
        );
        let result = triage(&mut inbox, 0, &mut track, InsertPosition::Bottom, "T");
        assert!(result.is_err());
        // Inbox must be unchanged
        assert_eq!(inbox.items.len(), original_len);
        assert_eq!(inbox.items[0].title, "Parser crash on empty blocks");
    }

    #[test]
    fn test_triage_invalid_after_target_preserves_inbox() {
        let mut inbox = sample_inbox();
        let original_len = inbox.items.len();
        let mut track = sample_track();
        let result = triage(
            &mut inbox,
            0,
            &mut track,
            InsertPosition::After("NONEXISTENT".into()),
            "T",
        );
        assert!(result.is_err());
        // Inbox must be unchanged
        assert_eq!(inbox.items.len(), original_len);
        assert_eq!(inbox.items[0].title, "Parser crash on empty blocks");
    }

    #[test]
    fn test_triage_out_of_range_preserves_inbox() {
        let mut inbox = sample_inbox();
        let original_len = inbox.items.len();
        let mut track = sample_track();
        let result = triage(&mut inbox, 10, &mut track, InsertPosition::Bottom, "T");
        assert!(result.is_err());
        assert_eq!(inbox.items.len(), original_len);
    }

    #[test]
    fn test_triage_no_body_no_note() {
        let mut inbox = sample_inbox();
        let mut track = sample_track();
        // Item at index 2 ("Quick note") has no body
        let _id = triage(&mut inbox, 2, &mut track, InsertPosition::Bottom, "T").unwrap();
        let tasks = track.backlog();
        let triaged = &tasks[2];
        assert!(!triaged.metadata.iter().any(|m| m.key() == "note"));
    }
}
