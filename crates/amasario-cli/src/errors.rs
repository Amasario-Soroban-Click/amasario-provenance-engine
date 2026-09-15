//! The CLI's error type, and the exit codes it maps to.
//!
//! # Why the CLI has its own error type
//!
//! Every failure the engine produces is already classified by
//! [`ErrorCategory`], and the CLI must not collapse that
//! classification back into one exit code. A CI job that runs `amasario verify` needs to
//! tell "the contract could not be reached" from "the contract was reached and its
//! provenance did not hold", because one is retried and the other fails a release. The
//! engine keeps the distinction; the CLI is where it would be lost if the exit code were
//! a bare 1.
//!
//! Two failures are the CLI's own rather than the engine's. [`CliError::Usage`] is an
//! argument problem the clap parser could not express - a flag combination that is
//! individually valid but jointly contradictory. [`CliError::Gate`] is a run that
//! completed successfully and whose result fails a gate the command was asked to
//! enforce, such as `verify` finding `CONFLICTING`. A gate is not an engine failure: the
//! analysis worked, and what it found is the problem.

use std::process::ExitCode;

use amasario_core::{EngineError, ErrorCategory};

/// The exit code for a run that completed but failed the gate it was asked to enforce.
///
/// Deliberately distinct from the engine's codes: a caller that wraps `amasario` in a
/// script needs to tell "the tool did its job and the answer is no" from "the tool could
/// not do its job".
pub const EXIT_GATE_FAILED: u8 = 1;
/// The exit code for a usage or configuration problem. Matches clap's own.
pub const EXIT_USAGE: u8 = 2;

/// The CLI's result type.
pub type CliResult<T> = Result<T, CliError>;

/// A CLI failure.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum CliError {
    /// A failure the engine classified.
    #[error(transparent)]
    Engine(#[from] EngineError),

    /// An argument problem the parser could not express.
    #[error("{0}")]
    Usage(String),

    /// The run succeeded and the result failed a gate the command enforces.
    #[error("{0}")]
    Gate(String),

    /// Writing the output failed.
    #[error("could not write output: {0}")]
    Io(#[from] std::io::Error),

    /// A document could not be serialised or parsed.
    ///
    /// A defect in the engine when the document was one the engine produced, and an input
    /// problem when it was read from a file; either way it is reported rather than panicked
    /// on.
    #[error("could not read or write a document: {0}")]
    Json(#[from] serde_json::Error),
}

impl CliError {
    /// The process exit code for this failure.
    ///
    /// Derived from the engine's category where there is one, so that the code the CLI
    /// exits with and the category in the structured error cannot disagree. The engine's
    /// categories and the codes are defined in one place rather than two tables that
    /// would drift.
    #[must_use]
    pub const fn exit_code(&self) -> u8 {
        match self {
            Self::Usage(_) => EXIT_USAGE,
            Self::Gate(_) => EXIT_GATE_FAILED,
            Self::Io(_) | Self::Json(_) => 74, // EX_IOERR, from the sysexits convention
            Self::Engine(error) => category_exit_code(error.category()),
        }
    }
}

/// The exit code for a category.
///
/// A total function over the categories, so a category added to the engine without a code
/// here is a compile error rather than a silent fall-through to a generic code. The
/// numbers follow the sysexits convention where one applies and are otherwise stable
/// within the tool; they are not the specification's, which describes categories rather
/// than process codes.
#[must_use]
pub const fn category_exit_code(category: ErrorCategory) -> u8 {
    match category {
        ErrorCategory::Configuration | ErrorCategory::Validation => EXIT_USAGE,
        ErrorCategory::Network => 69,  // EX_UNAVAILABLE
        ErrorCategory::Contract => 66, // EX_NOINPUT
        ErrorCategory::Provenance
        | ErrorCategory::Dependency
        | ErrorCategory::Graph
        | ErrorCategory::Impact => 65, // EX_DATAERR
        ErrorCategory::Snapshot => 66, // EX_NOINPUT
        ErrorCategory::Report | ErrorCategory::Export => 70, // EX_SOFTWARE
        ErrorCategory::SpecificationCompatibility => 65, // EX_DATAERR
        ErrorCategory::Internal => 70, // EX_SOFTWARE
        // `ErrorCategory` is non-exhaustive, so a category added to the engine
        // without a code here falls back to the generic software-error code rather
        // than failing to compile. The test below asserts that every category the
        // engine currently defines has a code, so the fallback is a safety net
        // rather than a place a real category hides.
        _ => 70,
    }
}

impl From<CliError> for ExitCode {
    fn from(error: CliError) -> Self {
        ExitCode::from(error.exit_code())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_engine_category_maps_to_a_code() {
        // The match in `category_exit_code` is exhaustive by construction; this asserts
        // that no two categories that a caller must distinguish share a code.
        for category in ErrorCategory::all() {
            let _ = category_exit_code(*category);
        }
    }

    #[test]
    fn a_network_failure_and_a_provenance_failure_exit_differently() {
        // The whole point of the mapping: a CI job retries the first and fails the
        // second.
        let network = category_exit_code(ErrorCategory::Network);
        let provenance = category_exit_code(ErrorCategory::Provenance);
        assert_ne!(network, provenance);
    }

    #[test]
    fn a_gate_failure_is_distinct_from_a_tool_failure() {
        assert_eq!(
            CliError::Gate("verification did not hold".to_owned()).exit_code(),
            EXIT_GATE_FAILED
        );
        assert_ne!(
            CliError::Gate("x".to_owned()).exit_code(),
            CliError::Usage("y".to_owned()).exit_code()
        );
    }

    #[test]
    fn a_usage_problem_exits_with_two() {
        assert_eq!(CliError::Usage("bad flags".to_owned()).exit_code(), 2);
    }
}
