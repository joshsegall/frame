//! Editing text in the user's external editor (`$VISUAL`, `$EDITOR`, `vi`).
//!
//! Shared by the TUI's `E` and `fr edit`. The caller supplies the text and a
//! parser; [`edit_until_valid`] reopens the editor on the user's own text, with
//! the parse error on top, until it parses or the user gives up.

use std::fmt::Display;
use std::io::{self, IsTerminal};
use std::path::Path;
use std::process::Command;

use tempfile::{Builder, NamedTempFile};

/// Prefix of the line that explains why the editor reopened.
const ERROR_PREFIX: &str = "# ERROR: ";

/// How an editing session ended.
pub enum Edited<T> {
    /// Saved and parsed. The temp file still holds the text, so a caller that
    /// cannot apply it can `keep()` it rather than lose it.
    Saved(T, NamedTempFile),
    /// Saved unchanged, or emptied: the way out for editors that cannot exit
    /// non-zero.
    Unchanged,
    /// The editor exited non-zero.
    Cancelled,
}

/// The editor command: `$VISUAL`, then `$EDITOR`, then `vi`.
pub fn editor_command() -> String {
    ["VISUAL", "EDITOR"]
        .iter()
        .filter_map(|k| std::env::var(k).ok())
        .find(|v| !v.trim().is_empty())
        .unwrap_or_else(|| "vi".to_string())
}

/// Refuse to start an editor nobody can see. Without this, a script or an
/// agent calling `fr edit` waits forever on `vi`.
pub fn require_terminal() -> Result<(), String> {
    // Debug builds only: lets the integration tests drive a scripted editor.
    #[cfg(debug_assertions)]
    if std::env::var_os("FRAME_EDITOR_NO_TTY").is_some() {
        return Ok(());
    }
    if io::stdin().is_terminal() && io::stdout().is_terminal() {
        Ok(())
    } else {
        Err("an external editor needs a terminal on stdin and stdout".to_string())
    }
}

/// Run the editor on `path`; `Ok(true)` if it exited 0. Goes through `sh` the
/// way git does, so `EDITOR="code --wait"` and quoted paths both work.
fn run_editor(editor: &str, path: &Path) -> io::Result<bool> {
    let status = Command::new("sh")
        .arg("-c")
        .arg(format!("{editor} \"$@\""))
        .arg(editor)
        .arg(path)
        .status()
        .map_err(|e| io::Error::new(e.kind(), format!("cannot run editor `{editor}`: {e}")))?;
    // 127: `sh` could not find the command — a failure to start, not a cancel.
    if status.code() == Some(127) {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("cannot run editor `{editor}`: command not found"),
        ));
    }
    Ok(status.success())
}

/// `text` with the error lines a previous attempt pinned on top removed.
fn strip_error_lines(text: &str) -> String {
    let mut out: String = text
        .lines()
        .skip_while(|l| l.starts_with(ERROR_PREFIX))
        .collect::<Vec<_>>()
        .join("\n");
    out.push('\n');
    out
}

/// Open `initial` in the editor until `parse` accepts what was saved.
///
/// `around` wraps each editor run, so the TUI can hand the terminal over and
/// take it back; the CLI passes `|run| run()`.
pub fn edit_until_valid<T, E: Display>(
    initial: &str,
    around: impl Fn(&dyn Fn() -> io::Result<bool>) -> io::Result<bool>,
    parse: impl Fn(&str) -> Result<T, E>,
) -> io::Result<Edited<T>> {
    let tmp = Builder::new().suffix(".md").tempfile()?;
    let editor = editor_command();
    let mut opened = initial.to_string();
    loop {
        std::fs::write(tmp.path(), &opened)?;
        if !around(&|| run_editor(&editor, tmp.path()))? {
            return Ok(Edited::Cancelled);
        }
        let edited = std::fs::read_to_string(tmp.path())?;
        if edited == opened || edited.trim().is_empty() {
            return Ok(Edited::Unchanged);
        }
        match parse(&edited) {
            Ok(parsed) => return Ok(Edited::Saved(parsed, tmp)),
            Err(e) => opened = format!("{ERROR_PREFIX}{e}\n{}", strip_error_lines(&edited)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_lines_do_not_pile_up() {
        let text = "# ERROR: one\n# ERROR: two\nT-1\n---\n";
        assert_eq!(strip_error_lines(text), "T-1\n---\n");
    }
}
