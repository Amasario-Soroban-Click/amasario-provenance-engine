//! Where a command's rendered output goes.
//!
//! # stdout is the result; stderr is the narration
//!
//! A machine reads stdout and a person reads both. Everything a command produces as its
//! result goes to stdout - or to the file named by `--output` - and nothing else does.
//! Progress, warnings and the summary line go to stderr, so that a pipeline may consume
//! stdout without stripping commentary and a person still sees what happened. A command
//! that mixed the two would make `amasario graph --format dot | dot -Tsvg` render the
//! narration as part of the picture.
//!
//! # A file is written whole or not at all
//!
//! Directory creation and the write are the only side effects, and a failure at either is
//! reported rather than half-applied: a report is either on disk or the command failed,
//! never partially written.

use std::io::Write;
use std::path::Path;

use crate::errors::CliError;

/// Writes rendered output to a file, or to stdout when no file is named.
///
/// # Errors
///
/// Returns an I/O error when the directory cannot be created, the file cannot be
/// written, or stdout cannot be flushed.
pub fn emit(rendered: &str, output: Option<&Path>) -> Result<(), CliError> {
    let text = with_trailing_newline(rendered);
    match output {
        Some(path) => {
            if let Some(parent) = path.parent()
                && !parent.as_os_str().is_empty()
            {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(path, text.as_bytes())?;
            note(&format!("wrote {}", path.display()));
            Ok(())
        },
        None => {
            let stdout = std::io::stdout();
            let mut handle = stdout.lock();
            handle.write_all(text.as_bytes())?;
            handle.flush()?;
            Ok(())
        },
    }
}

/// Writes a narration line to stderr.
///
/// Never to stdout, for the reason the module documents.
pub fn note(message: &str) {
    let stderr = std::io::stderr();
    let mut handle = stderr.lock();
    // A failed narration write is not worth failing a run over: the result is already on
    // stdout and the only thing lost is a human-readable line.
    let _ = writeln!(handle, "{message}");
}

/// Returns the text with exactly one trailing newline.
///
/// A renderer that returned text without one would make the shell prompt collide with the
/// output; a renderer that returned two would add a blank line to every file written.
/// Normalising here means neither renderer has to think about it.
#[must_use]
pub fn with_trailing_newline(text: &str) -> String {
    let trimmed = text.trim_end_matches('\n');
    let mut owned = trimmed.to_owned();
    owned.push('\n');
    owned
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_final_newline_is_added() {
        assert_eq!(with_trailing_newline("one line"), "one line\n");
    }

    #[test]
    fn a_single_final_newline_is_left_alone() {
        assert_eq!(with_trailing_newline("one line\n"), "one line\n");
    }

    #[test]
    fn extra_final_newlines_are_collapsed_to_one() {
        // A file that ended with a blank line would differ from one that did not for no
        // reason, and a diff over two exports would show it.
        assert_eq!(with_trailing_newline("one line\n\n\n"), "one line\n");
    }

    #[test]
    fn emitting_to_a_file_writes_the_content_and_creates_the_directory() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let path = directory.path().join("nested").join("out.json");
        emit("{\"ok\":true}", Some(path.as_path())).expect("the write succeeds");
        let written = std::fs::read_to_string(&path).expect("the file exists");
        assert_eq!(written, "{\"ok\":true}\n");
    }

    #[test]
    fn emitting_the_same_content_twice_is_byte_identical() {
        let directory = tempfile::tempdir().expect("a temporary directory");
        let first = directory.path().join("a.json");
        let second = directory.path().join("b.json");
        emit("same", Some(first.as_path())).expect("the first write succeeds");
        emit("same", Some(second.as_path())).expect("the second write succeeds");
        assert_eq!(
            std::fs::read(&first).expect("read a"),
            std::fs::read(&second).expect("read b")
        );
    }
}
