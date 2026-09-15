//! Locating the committed corpus, and comparing it against what the builders produce.
//!
//! # A fixture is generated, and the generator is checked
//!
//! Every document under `fixtures/` is produced by [`crate::documents`] from the
//! engine's own types. A hand-edited fixture would be a document that no constructor
//! agreed to, and the digest-bearing ones - a snapshot, a graph's edge identifier, a
//! report's canonical form - cannot be hand-written correctly at all. So the corpus is
//! generated, and each suite asserts that the committed bytes equal the freshly built
//! ones. A drift therefore fails a test rather than becoming a fixture that quietly
//! describes a model that no longer exists.
//!
//! The regeneration command is
//! `cargo run -p amasario-integration-tests --bin generate-fixtures`, which is named in
//! every failure message so that a reader knows what to do about it.

use std::fs;
use std::path::{Path, PathBuf};

use serde::de::DeserializeOwned;

/// The repository root, derived from this crate's manifest rather than from the
/// working directory, because a test's working directory is the package root and a
/// fuzzer's is not.
#[must_use]
pub fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the integration crate sits in the repository root")
        .to_path_buf()
}

/// One fixture directory, and the files it is expected to hold.
#[derive(Debug, Clone, Copy)]
pub struct Corpus;

impl Corpus {
    /// The `fixtures/` root.
    #[must_use]
    pub fn root() -> PathBuf {
        repository_root().join("fixtures")
    }

    /// The path of one fixture, as `fixtures/<directory>/<file>`.
    #[must_use]
    pub fn path(directory: &str, file: &str) -> PathBuf {
        Self::root().join(directory).join(file)
    }

    /// The path of one fixture, relative to the repository root.
    ///
    /// This is the form a failure message should print, because it is the form a
    /// reader can paste into an editor or a `git diff`.
    #[must_use]
    pub fn relative(directory: &str, file: &str) -> String {
        format!("fixtures/{directory}/{file}")
    }

    /// Every file in one fixture directory, sorted by name.
    ///
    /// # Panics
    ///
    /// Panics when the directory does not exist, because a missing fixture directory
    /// is a defect rather than an empty surface: the workflow that reports an absent
    /// `fixtures/` tree is the one that decides whether the surface is unbuilt, and
    /// this crate is only ever built after that decision.
    #[must_use]
    pub fn files_in(directory: &str) -> Vec<PathBuf> {
        let dir = Self::root().join(directory);
        let entries = fs::read_dir(&dir).unwrap_or_else(|error| {
            panic!(
                "{} could not be read ({error}). The fixture corpus is generated: run \
                 `cargo run -p amasario-integration-tests --bin generate-fixtures`",
                dir.display()
            )
        });
        let mut files: Vec<PathBuf> = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
            .collect();
        files.sort();
        files
    }

    /// Reads one fixture as text.
    ///
    /// # Panics
    ///
    /// Panics with the regeneration command when the file is absent.
    #[must_use]
    pub fn read(directory: &str, file: &str) -> String {
        let path = Self::path(directory, file);
        fs::read_to_string(&path).unwrap_or_else(|error| {
            panic!(
                "{error}: {}. The fixture corpus is generated: run \
                 `cargo run -p amasario-integration-tests --bin generate-fixtures`",
                path.display()
            )
        })
    }

    /// Reads and parses one fixture.
    ///
    /// # Panics
    ///
    /// Panics when the fixture is absent or does not parse, naming the fixture and the
    /// parser's own diagnosis.
    #[must_use]
    pub fn json<T: DeserializeOwned>(directory: &str, file: &str) -> T {
        let text = Self::read(directory, file);
        serde_json::from_str(&text).unwrap_or_else(|error| {
            panic!(
                "{} did not parse ({error}). A committed fixture the engine cannot read is \
                 worse than a missing one, so this is a failure rather than a skip",
                Self::relative(directory, file)
            )
        })
    }
}

/// Asserts that a committed fixture is exactly what a builder produces.
///
/// # Panics
///
/// Panics with a unified diff of the two renderings when they differ, so that the
/// failure says *what* drifted rather than only that something did. The message names
/// the regeneration command, because the fix is always to regenerate and review.
pub fn assert_committed(directory: &str, file: &str, generated: &str) {
    let relative = Corpus::relative(directory, file);
    let path = Corpus::path(directory, file);
    let committed = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) => panic!(
            "{relative} could not be read ({error}). Run \
             `cargo run -p amasario-integration-tests --bin generate-fixtures`"
        ),
    };

    if committed == generated {
        return;
    }

    let first_difference = committed
        .lines()
        .zip(generated.lines())
        .position(|(committed, generated)| committed != generated)
        .map_or_else(
            || "the files differ only in length or trailing newline".to_owned(),
            |line| {
                format!(
                    "first difference at line {}:\n  committed: {}\n  rebuilt:   {}",
                    line + 1,
                    committed.lines().nth(line).unwrap_or(""),
                    generated.lines().nth(line).unwrap_or("")
                )
            },
        );

    panic!(
        "{relative} no longer matches the document its builder produces, so either the \
         fixture was edited by hand or the model changed and the fixture did not follow it.\n\
         {first_difference}\n\
         Regenerate with `cargo run -p amasario-integration-tests --bin generate-fixtures` \
         and review the diff."
    );
}

/// Reads a committed fixture and asserts it parses with `T`'s own deserialiser.
///
/// Returned so that a caller can then assert something about the parsed value; the
/// parse itself is the assertion.
///
/// # Panics
///
/// Panics when the fixture does not parse, naming the file.
pub fn parse_committed<T: DeserializeOwned>(directory: &str, file: &str) -> T {
    Corpus::json(directory, file)
}
