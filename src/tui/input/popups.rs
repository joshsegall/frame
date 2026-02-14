use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::tui::app::{App, DepPopupEntry, View};

pub(super) fn open_dep_popup_from_track_view(app: &mut App) {
    if let Some((track_id, task_id, _section)) = app.cursor_task_id() {
        app.open_dep_popup(&track_id, &task_id);
    }
}

pub(super) fn open_dep_popup_from_detail_view(app: &mut App) {
    if let View::Detail {
        ref track_id,
        ref task_id,
    } = app.view
    {
        let track_id = track_id.clone();
        let task_id = task_id.clone();
        app.open_dep_popup(&track_id, &task_id);
    }
}

pub(super) fn handle_dep_popup_key(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Esc => {
            app.dep_popup = None;
        }
        KeyCode::Char('j') | KeyCode::Down => {
            dep_popup_move_cursor(app, 1);
        }
        KeyCode::Char('k') | KeyCode::Up => {
            dep_popup_move_cursor(app, -1);
        }
        KeyCode::Char('g') => {
            dep_popup_jump_top(app);
        }
        KeyCode::Char('G') => {
            dep_popup_jump_bottom(app);
        }
        KeyCode::Char('l') | KeyCode::Right => {
            dep_popup_expand(app);
        }
        KeyCode::Char('h') | KeyCode::Left => {
            dep_popup_collapse(app);
        }
        KeyCode::Enter => {
            dep_popup_jump_to_task(app);
        }
        _ => {}
    }
}

pub(super) fn dep_popup_move_cursor(app: &mut App, direction: i32) {
    let dp = match &mut app.dep_popup {
        Some(dp) => dp,
        None => return,
    };

    let len = dp.entries.len();
    if len == 0 {
        return;
    }

    let mut new_cursor = dp.cursor;
    loop {
        if direction > 0 {
            if new_cursor + 1 >= len {
                break;
            }
            new_cursor += 1;
        } else {
            if new_cursor == 0 {
                break;
            }
            new_cursor -= 1;
        }
        // Skip section headers and nothing entries
        if matches!(dp.entries.get(new_cursor), Some(DepPopupEntry::Task { .. })) {
            dp.cursor = new_cursor;
            break;
        }
    }

    // Adjust scroll to keep cursor visible
    // The visible entry lines start at line 1 (after top blank line)
    // but the entries map 1:1 to line indices + 1 (blank line offset)
    dep_popup_adjust_scroll(dp);
}

pub(super) fn dep_popup_adjust_scroll(dp: &mut crate::tui::app::DepPopupState) {
    // We don't know the exact popup height here, but we'll use a reasonable estimate.
    // The actual scroll adjustment happens based on cursor position relative to visible window.
    // Use a max visible estimate of 15 entries.
    let visible_entries = 15usize;
    if dp.cursor < dp.scroll_offset {
        dp.scroll_offset = dp.cursor;
    }
    if dp.cursor >= dp.scroll_offset + visible_entries {
        dp.scroll_offset = dp.cursor - visible_entries + 1;
    }
}

pub(super) fn dep_popup_jump_top(app: &mut App) {
    let dp = match &mut app.dep_popup {
        Some(dp) => dp,
        None => return,
    };
    // Find first selectable entry
    if let Some(idx) = dp
        .entries
        .iter()
        .position(|e| matches!(e, DepPopupEntry::Task { .. }))
    {
        dp.cursor = idx;
        dep_popup_adjust_scroll(dp);
    }
}

pub(super) fn dep_popup_jump_bottom(app: &mut App) {
    let dp = match &mut app.dep_popup {
        Some(dp) => dp,
        None => return,
    };
    // Find last selectable entry
    if let Some(idx) = dp
        .entries
        .iter()
        .rposition(|e| matches!(e, DepPopupEntry::Task { .. }))
    {
        dp.cursor = idx;
        dep_popup_adjust_scroll(dp);
    }
}

pub(super) fn dep_popup_expand(app: &mut App) {
    // Get the cursor entry info, then modify state
    let (expand_key, should_rebuild) = {
        let dp = match &app.dep_popup {
            Some(dp) => dp,
            None => return,
        };
        let entry = match dp.entries.get(dp.cursor) {
            Some(DepPopupEntry::Task {
                task_id,
                has_children,
                is_expanded,
                is_circular,
                is_dangling,
                is_upstream,
                ..
            }) => {
                if *is_circular || *is_dangling || !*has_children || *is_expanded {
                    return;
                }
                let prefix = if *is_upstream { "up" } else { "down" };
                format!("{}:{}", prefix, task_id)
            }
            _ => return,
        };
        (entry, true)
    };

    if should_rebuild {
        if let Some(dp) = &mut app.dep_popup {
            dp.expanded.insert(expand_key);
        }
        // Rebuild entries
        let mut dp = app.dep_popup.take().unwrap();
        app.rebuild_dep_popup_entries(&mut dp);
        app.dep_popup = Some(dp);
    }
}

pub(super) fn dep_popup_collapse(app: &mut App) {
    // Get the cursor entry info, then modify state
    let collapse_key = {
        let dp = match &app.dep_popup {
            Some(dp) => dp,
            None => return,
        };
        match dp.entries.get(dp.cursor) {
            Some(DepPopupEntry::Task {
                task_id,
                is_expanded,
                is_upstream,
                depth,
                ..
            }) => {
                if *is_expanded {
                    // Collapse this entry
                    let prefix = if *is_upstream { "up" } else { "down" };
                    Some(format!("{}:{}", prefix, task_id))
                } else if *depth > 0 {
                    // Move cursor to parent — find the previous entry with depth-1
                    let cursor = dp.cursor;
                    let target_depth = depth - 1;
                    let is_up = *is_upstream;
                    let mut parent_idx = None;
                    for i in (0..cursor).rev() {
                        if let DepPopupEntry::Task {
                            depth: d,
                            is_upstream: u,
                            ..
                        } = &dp.entries[i]
                            && *d == target_depth
                            && *u == is_up
                        {
                            parent_idx = Some(i);
                            break;
                        }
                    }
                    if let Some(idx) = parent_idx {
                        // Just move cursor to parent, don't collapse
                        let dp = app.dep_popup.as_mut().unwrap();
                        dp.cursor = idx;
                        dep_popup_adjust_scroll(dp);
                    }
                    return;
                } else {
                    return;
                }
            }
            _ => return,
        }
    };

    if let Some(key) = collapse_key {
        if let Some(dp) = &mut app.dep_popup {
            dp.expanded.remove(&key);
        }
        let mut dp = app.dep_popup.take().unwrap();
        // Remember cursor task id to restore cursor position
        let cursor_task_id = match dp.entries.get(dp.cursor) {
            Some(DepPopupEntry::Task { task_id, .. }) => Some(task_id.clone()),
            _ => None,
        };
        app.rebuild_dep_popup_entries(&mut dp);
        // Restore cursor position to the same task
        if let Some(tid) = cursor_task_id
            && let Some(idx) = dp
                .entries
                .iter()
                .position(|e| matches!(e, DepPopupEntry::Task { task_id, .. } if task_id == &tid))
        {
            dp.cursor = idx;
        }
        dep_popup_adjust_scroll(&mut dp);
        app.dep_popup = Some(dp);
    }
}

pub(super) fn dep_popup_jump_to_task(app: &mut App) {
    let (task_id, entry_track_id) = {
        let dp = match &app.dep_popup {
            Some(dp) => dp,
            None => return,
        };
        match dp.entries.get(dp.cursor) {
            Some(DepPopupEntry::Task {
                task_id,
                track_id,
                is_dangling,
                is_circular,
                ..
            }) => {
                if *is_dangling || *is_circular {
                    return;
                }
                (task_id.clone(), track_id.clone())
            }
            _ => return,
        }
    };

    // Close popup and jump to the task
    app.dep_popup = None;
    if !app.jump_to_task(&task_id) {
        // jump_to_task fails for Done-section tasks (not in flat items).
        // Fall back to opening detail view if the task exists in a track.
        if let Some(track_id) = entry_track_id {
            app.open_detail(track_id, task_id);
        } else {
            app.status_message = Some(format!("task {} not found", task_id));
            app.status_is_error = true;
        }
    }
}

// ---------------------------------------------------------------------------
// Tag color popup

pub(super) fn handle_tag_color_popup_key(app: &mut App, key: KeyEvent) {
    let is_picker_open = app
        .tag_color_popup
        .as_ref()
        .map(|tcp| tcp.picker_open)
        .unwrap_or(false);

    if is_picker_open {
        handle_tag_color_picker_key(app, key);
    } else {
        handle_tag_color_list_key(app, key);
    }
}

pub(super) fn handle_tag_color_list_key(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Esc => {
            app.tag_color_popup = None;
        }
        KeyCode::Char('j') | KeyCode::Down => {
            tag_color_move_cursor(app, 1);
        }
        KeyCode::Char('k') | KeyCode::Up => {
            tag_color_move_cursor(app, -1);
        }
        KeyCode::Enter => {
            tag_color_open_picker(app);
        }
        KeyCode::Backspace => {
            tag_color_clear(app);
        }
        _ => {}
    }
}

pub(super) fn handle_tag_color_picker_key(app: &mut App, key: KeyEvent) {
    use crate::tui::app::TAG_COLOR_PALETTE;

    let palette_count = TAG_COLOR_PALETTE.len();

    match key.code {
        KeyCode::Esc => {
            // Cancel picker, return to list
            if let Some(tcp) = &mut app.tag_color_popup {
                tcp.picker_open = false;
            }
        }
        KeyCode::Char('h') | KeyCode::Left => {
            if let Some(tcp) = &mut app.tag_color_popup
                && tcp.picker_cursor > 0
            {
                tcp.picker_cursor -= 1;
            }
        }
        KeyCode::Char('l') | KeyCode::Right => {
            if let Some(tcp) = &mut app.tag_color_popup
                && tcp.picker_cursor < palette_count
            {
                tcp.picker_cursor += 1;
            }
        }
        KeyCode::Enter => {
            // Assign selected color or clear if on "×"
            let (tag, picker_idx) = match &app.tag_color_popup {
                Some(tcp) if !tcp.tags.is_empty() => {
                    let tag = tcp.tags[tcp.cursor].0.clone();
                    (tag, tcp.picker_cursor)
                }
                _ => return,
            };

            if picker_idx < palette_count {
                let hex = TAG_COLOR_PALETTE[picker_idx].1;
                tag_color_assign(app, &tag, hex);
            } else {
                // "×" position — clear
                tag_color_clear(app);
            }

            if let Some(tcp) = &mut app.tag_color_popup {
                tcp.picker_open = false;
            }
        }
        KeyCode::Backspace => {
            tag_color_clear(app);
            if let Some(tcp) = &mut app.tag_color_popup {
                tcp.picker_open = false;
            }
        }
        _ => {}
    }
}

pub(super) fn tag_color_move_cursor(app: &mut App, direction: i32) {
    let tcp = match &mut app.tag_color_popup {
        Some(tcp) => tcp,
        None => return,
    };
    let len = tcp.tags.len();
    if len == 0 {
        return;
    }
    if direction > 0 {
        if tcp.cursor + 1 < len {
            tcp.cursor += 1;
        }
    } else if tcp.cursor > 0 {
        tcp.cursor -= 1;
    }
    tag_color_adjust_scroll(tcp);
}

pub(super) fn tag_color_adjust_scroll(tcp: &mut crate::tui::app::TagColorPopupState) {
    let visible = 15usize; // approximate visible entries
    if tcp.cursor < tcp.scroll_offset {
        tcp.scroll_offset = tcp.cursor;
    }
    if tcp.cursor >= tcp.scroll_offset + visible {
        tcp.scroll_offset = tcp.cursor - visible + 1;
    }
}

pub(super) fn tag_color_open_picker(app: &mut App) {
    use crate::tui::app::TAG_COLOR_PALETTE;

    let tcp = match &mut app.tag_color_popup {
        Some(tcp) => tcp,
        None => return,
    };
    if tcp.tags.is_empty() {
        return;
    }

    tcp.picker_open = true;

    // Pre-select the current color's swatch (if it matches a palette entry)
    let current_hex = tcp.tags[tcp.cursor].1.as_deref();
    tcp.picker_cursor = match current_hex {
        Some(hex) => {
            let hex_upper = hex.to_uppercase();
            TAG_COLOR_PALETTE
                .iter()
                .position(|(_, ph)| ph.to_uppercase() == hex_upper)
                .unwrap_or(0) // custom hex: no pre-selection, start at first
        }
        None => 0,
    };
}

/// Assign a palette color to the current tag and write to config
pub(super) fn tag_color_assign(app: &mut App, tag: &str, hex: &str) {
    use crate::io::config_io;

    // Write to disk via toml_edit (round-trip safe)
    let frame_dir = app.project.frame_dir.clone();
    if let Ok((_config, mut doc)) = config_io::read_config(&frame_dir) {
        config_io::set_tag_color(&mut doc, tag, hex);
        let _ = config_io::write_config(&frame_dir, &doc);
    }

    // Update in-memory config
    app.project
        .config
        .ui
        .tag_colors
        .insert(tag.to_string(), hex.to_string());

    // Update theme
    if let Some(color) = crate::tui::theme::parse_hex_color_pub(hex) {
        app.theme.tag_colors.insert(tag.to_string(), color);
    }

    // Update popup state
    if let Some(tcp) = &mut app.tag_color_popup
        && let Some(entry) = tcp.tags.iter_mut().find(|(t, _)| t == tag)
    {
        entry.1 = Some(hex.to_string());
    }

    app.last_save_at = Some(std::time::Instant::now());
}

/// Clear the color for the current tag and write to config
pub(super) fn tag_color_clear(app: &mut App) {
    use crate::io::config_io;

    let tag = match &app.tag_color_popup {
        Some(tcp) if !tcp.tags.is_empty() => tcp.tags[tcp.cursor].0.clone(),
        _ => return,
    };

    // Write to disk via toml_edit (round-trip safe)
    let frame_dir = app.project.frame_dir.clone();
    if let Ok((_config, mut doc)) = config_io::read_config(&frame_dir) {
        config_io::clear_tag_color(&mut doc, &tag);
        let _ = config_io::write_config(&frame_dir, &doc);
    }

    // Update in-memory config
    app.project.config.ui.tag_colors.shift_remove(&tag);

    // Update theme: remove the explicit mapping so it falls back to hardcoded defaults
    // But we need to check if there's a hardcoded default; if so, keep it
    let default_theme = crate::tui::theme::Theme::default();
    if let Some(default_color) = default_theme.tag_colors.get(&tag) {
        app.theme.tag_colors.insert(tag.clone(), *default_color);
    } else {
        app.theme.tag_colors.remove(&tag);
    }

    // Update popup state
    if let Some(tcp) = &mut app.tag_color_popup
        && let Some(entry) = tcp.tags.iter_mut().find(|(t, _)| t == &tag)
    {
        entry.1 = None;
    }

    app.last_save_at = Some(std::time::Instant::now());
}

// ---------------------------------------------------------------------------
// Project picker

pub(super) fn open_project_picker(app: &mut App) {
    let reg = crate::io::registry::read_registry();
    let current_path = Some(app.project.root.to_string_lossy().to_string());
    app.project_picker = Some(crate::tui::app::ProjectPickerState::new(
        reg.projects,
        current_path,
    ));
}

pub(super) fn handle_project_picker_key(app: &mut App, key: KeyEvent) {
    let picker = match &mut app.project_picker {
        Some(p) => p,
        None => return,
    };

    match (key.modifiers, key.code) {
        (_, KeyCode::Esc) => {
            app.project_picker = None;
        }
        (_, KeyCode::Up) | (_, KeyCode::Char('k')) => {
            picker.move_up();
        }
        (_, KeyCode::Down) | (_, KeyCode::Char('j')) => {
            picker.move_down();
        }
        (_, KeyCode::Enter) => {
            if let Some(entry) = picker.selected_entry() {
                let path = entry.path.clone();
                let root = std::path::PathBuf::from(&path);
                if !root.join("frame").exists() {
                    app.status_message = Some(format!("project not found at {}", path));
                    app.project_picker = None;
                    return;
                }
                // Switch project: load the new project
                match crate::io::project_io::load_project(&root) {
                    Ok(mut project) => {
                        // Ensure IDs and dates
                        let modified = crate::ops::clean::ensure_ids_and_dates(&mut project);
                        if !modified.is_empty() {
                            let _lock =
                                crate::io::lock::FileLock::acquire_default(&project.frame_dir).ok();
                            for track_id in &modified {
                                if let Some(tc) =
                                    project.config.tracks.iter().find(|tc| tc.id == *track_id)
                                {
                                    let file = &tc.file;
                                    if let Some(track) = project
                                        .tracks
                                        .iter()
                                        .find(|(id, _)| id == track_id)
                                        .map(|(_, t)| t)
                                    {
                                        let _ = crate::io::project_io::save_track(
                                            &project.frame_dir,
                                            file,
                                            track,
                                        );
                                    }
                                }
                            }
                        }

                        // Touch TUI timestamp
                        crate::io::registry::register_project(
                            &project.config.project.name,
                            &project.root,
                        );
                        crate::io::registry::touch_tui(&project.root);

                        // Save old UI state before switching
                        crate::tui::app::save_ui_state(app);

                        // Replace app with a fresh App for the new project
                        *app = App::new(project);
                        app.watcher_needs_restart = true;

                        // Update terminal window title
                        crate::tui::app::set_window_title(&app.project.config.project.name);

                        // Restore UI state for the new project
                        crate::tui::app::restore_ui_state(app);
                    }
                    Err(e) => {
                        app.status_message = Some(format!("error loading project: {}", e));
                        app.project_picker = None;
                    }
                }
            }
        }
        (KeyModifiers::SHIFT, KeyCode::Char('X')) | (KeyModifiers::NONE, KeyCode::Char('X')) => {
            picker.remove_selected();
        }
        (_, KeyCode::Char('s')) => {
            picker.toggle_sort();
        }
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// Recovery log overlay

/// Open the recovery log overlay by loading entries and caching lines.
pub(super) fn open_recovery_overlay(app: &mut App) {
    let entries =
        crate::io::recovery::read_recovery_entries(&app.project.frame_dir, Some(10), None);
    let mut lines = Vec::new();
    if entries.is_empty() {
        // Will be handled by the renderer
    } else {
        for entry in &entries {
            let md = entry.to_display_markdown();
            for line in md.lines() {
                lines.push(line.to_string());
            }
        }
    }
    app.recovery_log_lines = lines;
    app.recovery_log_scroll = 0;
    app.show_recovery_log = true;
}

/// Handle input when the recovery log overlay is showing.
pub(super) fn handle_recovery_overlay(app: &mut App, key: KeyEvent) {
    match (key.modifiers, key.code) {
        (_, KeyCode::Esc) | (_, KeyCode::Char('q')) => {
            app.show_recovery_log = false;
            app.recovery_log_lines.clear();
        }

        // Jump to previous/next log entry (## header)
        (m, KeyCode::Up) if m.contains(KeyModifiers::ALT) => {
            recovery_jump_entry(app, -1);
        }
        (m, KeyCode::Down) if m.contains(KeyModifiers::ALT) => {
            recovery_jump_entry(app, 1);
        }

        (_, KeyCode::Char('j')) | (_, KeyCode::Down) => {
            app.recovery_log_scroll = app.recovery_log_scroll.saturating_add(1);
        }
        (_, KeyCode::Char('k')) | (_, KeyCode::Up) => {
            app.recovery_log_scroll = app.recovery_log_scroll.saturating_sub(1);
        }
        (_, KeyCode::Char('g')) => {
            app.recovery_log_scroll = 0;
        }
        (_, KeyCode::Char('G')) => {
            app.recovery_log_scroll = usize::MAX; // renderer clamps
        }
        (_, KeyCode::PageDown) => {
            app.recovery_log_scroll = app.recovery_log_scroll.saturating_add(20);
        }
        (_, KeyCode::PageUp) => {
            app.recovery_log_scroll = app.recovery_log_scroll.saturating_sub(20);
        }
        _ => {}
    }
}

/// Jump to the next (`direction == 1`) or previous (`direction == -1`) `## `
/// header line in the recovery log overlay. Scroll positions are in visual
/// line units (post word-wrap), so we map logical header positions through
/// `recovery_log_line_offsets`.
pub(super) fn recovery_jump_entry(app: &mut App, direction: i32) {
    let lines = &app.recovery_log_lines;
    let offsets = &app.recovery_log_line_offsets;
    if lines.is_empty() || offsets.is_empty() {
        return;
    }

    // Find which logical line is currently at the scroll position
    let cur_logical = offsets
        .partition_point(|&off| off <= app.recovery_log_scroll)
        .saturating_sub(1);

    if direction < 0 {
        for i in (0..cur_logical).rev() {
            if lines[i].starts_with("## ") {
                app.recovery_log_scroll = offsets[i];
                return;
            }
        }
        app.recovery_log_scroll = 0;
    } else {
        for i in (cur_logical + 1)..lines.len() {
            if lines[i].starts_with("## ") {
                app.recovery_log_scroll = offsets[i];
                return;
            }
        }
        app.recovery_log_scroll = usize::MAX; // renderer clamps
    }
}

// ---------------------------------------------------------------------------
// Results overlay input handling

pub(super) fn handle_results_overlay(app: &mut App, key: KeyEvent) {
    match (key.modifiers, key.code) {
        (_, KeyCode::Esc) | (_, KeyCode::Char('q')) => {
            app.show_results_overlay = false;
            app.results_overlay_lines.clear();
        }
        (_, KeyCode::Char('j')) | (_, KeyCode::Down) => {
            app.results_overlay_scroll = app.results_overlay_scroll.saturating_add(1);
        }
        (_, KeyCode::Char('k')) | (_, KeyCode::Up) => {
            app.results_overlay_scroll = app.results_overlay_scroll.saturating_sub(1);
        }
        (_, KeyCode::Char('g')) => {
            app.results_overlay_scroll = 0;
        }
        (_, KeyCode::Char('G')) => {
            app.results_overlay_scroll = app.results_overlay_lines.len();
        }
        (_, KeyCode::PageDown) => {
            app.results_overlay_scroll = app.results_overlay_scroll.saturating_add(20);
        }
        (_, KeyCode::PageUp) => {
            app.results_overlay_scroll = app.results_overlay_scroll.saturating_sub(20);
        }
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// Check project (palette action)
