//! Running the `amasario` binary from a test.
//!
//! # Why the path is derived rather than configured
//!
//! Cargo sets `CARGO_BIN_EXE_<name>` only for integration tests of the package that
//! declares the binary, and the binary belongs to `amasario-cli` while these suites
//! belong to `amasario-integration-tests`. Depending on the CLI crate would build only
//! its library, so there is no environment variable to read and no dependency that
//! would produce one.
//!
//! What is reliable is the layout: a test executable runs from
//! `target/<profile>/deps/<test>-<hash>`, so the binary it should run is two
//! directories up and named after the product. That is derived from
//! `std::env::current_exe()` rather than from a guess about the working directory, and
//! a missing binary is reported with the command that builds it rather than being
//! skipped - a suite that silently did nothing would be worse than no suite.

use std::path::PathBuf;
use std::process::{Command, Output};

/// The path the `amasario` binary is expected at.
///
/// # Panics
///
/// Panics when the test executable's own path cannot be read, which would mean the
/// process is not running from a Cargo build directory at all.
#[must_use]
pub fn binary() -> PathBuf {
    let executable = std::env::current_exe().unwrap_or_else(|error| {
        panic!(
            "the test executable's own path could not be read ({error}); the CLI suites \
             locate the product binary relative to it"
        )
    });
    // <target>/<profile>/deps/<test> -> <target>/<profile>
    let directory = executable
        .parent()
        .and_then(|deps| deps.parent())
        .unwrap_or_else(|| {
            panic!(
                "{} is not in a Cargo `deps` directory, so the product binary cannot be \
                 located relative to it",
                executable.display()
            )
        });
    let name = if cfg!(windows) {
        "amasario.exe"
    } else {
        "amasario"
    };
    directory.join(name)
}

/// Runs the binary with the given arguments.
///
/// # Panics
///
/// Panics when the binary has not been built, naming the command that builds it. This is
/// deliberately a panic rather than a skip, because a suite that quietly did nothing
/// would be worse than no suite.
///
/// # What the message used to claim, and why it was wrong
///
/// It said that `cargo test --workspace` builds every binary in the workspace, so an
/// absent binary meant the suite had been invoked in a way that did not build the
/// product. That is false, and it made the documented command fail on a clean checkout
/// with a message that blamed the caller. `cargo test` builds a binary target as a *test
/// harness* under `deps/`; the product binary at `target/<profile>/amasario` is produced
/// by `cargo build`, and by nothing else. So the two suites that run the process failed
/// on any machine without a previous build in the target directory - which was hidden
/// here for as long as it was, because a checked-in `target/` is not.
///
/// What changed is the claim, the `cargo test-all` alias, which builds first, and the
/// README's build commands. What did not change is failing loudly: the difference is
/// that the failure now describes the situation instead of inventing one.
#[must_use]
pub fn run(args: &[&str]) -> Output {
    let binary = binary();
    Command::new(&binary)
        .args(args)
        .output()
        .unwrap_or_else(|error| {
            panic!(
                "{} could not be run ({error}). Build it with `cargo build -p amasario-cli`; \
                 `cargo test --workspace` builds it as part of the workspace",
                binary.display()
            )
        })
}

/// Whether the binary is present, so that a suite can report rather than guess.
#[must_use]
pub fn is_built() -> bool {
    binary().is_file()
}

/// The standard output of a run, as text.
///
/// # Panics
///
/// Panics when the run wrote no UTF-8 to stdout, which would mean the assertion about
/// its output could not be evaluated.
#[must_use]
pub fn stdout(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).unwrap_or_else(|error| {
        panic!("the command's stdout was not UTF-8 ({error})");
    })
}

/// The standard error of a run, as text.
///
/// # Panics
///
/// Panics when the run wrote no UTF-8 to stderr.
#[must_use]
pub fn stderr(output: &Output) -> String {
    String::from_utf8(output.stderr.clone()).unwrap_or_else(|error| {
        panic!("the command's stderr was not UTF-8 ({error})");
    })
}
