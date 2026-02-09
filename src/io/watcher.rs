use std::path::{Path, PathBuf};
use std::sync::mpsc;

use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};

/// Events sent from the file watcher to the TUI event loop.
#[derive(Debug)]
pub enum FileEvent {
    /// One or more tracked files changed on disk.
    Changed(Vec<PathBuf>),
}

/// A file system watcher for the frame/ directory.
pub struct FrameWatcher {
    _watcher: RecommendedWatcher,
    rx: mpsc::Receiver<FileEvent>,
}

impl FrameWatcher {
    /// Start watching the given `frame/` directory.
    /// Returns a `FrameWatcher` whose `poll()` method should be called each tick.
    pub fn start(frame_dir: &Path) -> Result<Self, notify::Error> {
        let (tx, rx) = mpsc::channel();
        let frame_dir_owned = frame_dir.to_path_buf();

        let mut watcher = RecommendedWatcher::new(
            move |result: Result<Event, notify::Error>| {
                let event = match result {
                    Ok(e) => e,
                    Err(_) => return,
                };

                // We only care about creates, modifications, and removes of .md and .toml files
                match event.kind {
                    EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_) => {}
                    _ => return,
                }

                let relevant: Vec<PathBuf> = event
                    .paths
                    .into_iter()
                    .filter(|p| {
                        // Must be inside the frame directory
                        if !p.starts_with(&frame_dir_owned) {
                            return false;
                        }
                        // Skip .lock and .state.json
                        if let Some(name) = p.file_name().and_then(|n| n.to_str())
                            && (name == ".lock" || name == ".state.json")
                        {
                            return false;
                        }
                        // Only care about .md and .toml files
                        matches!(
                            p.extension().and_then(|e| e.to_str()),
                            Some("md") | Some("toml")
                        )
                    })
                    .collect();

                if !relevant.is_empty() {
                    let _ = tx.send(FileEvent::Changed(relevant));
                }
            },
            Config::default(),
        )?;

        watcher.watch(frame_dir, RecursiveMode::Recursive)?;
        Ok(FrameWatcher {
            _watcher: watcher,
            rx,
        })
    }

    /// Non-blocking poll for pending file events.
    /// Returns all queued events (may be empty).
    pub fn poll(&self) -> Vec<FileEvent> {
        let mut events = Vec::new();
        while let Ok(evt) = self.rx.try_recv() {
            events.push(evt);
        }
        events
    }
}
