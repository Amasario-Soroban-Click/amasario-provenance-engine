//! The export failures, classified so that a caller can tell them apart.
//!
//! An export can fail for reasons a caller must distinguish. A format that cannot
//! represent the content is not the same as content that is malformed, and neither is
//! the same as a format name that was never a format. The specification forbids
//! collapsing failures into one generic error, and the categories here are the ones the
//! export layer can actually produce.

use amasario_core::{EngineError, ErrorCategory};

/// A way an export can fail.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ExportFailure {
    /// The requested format is not one this crate implements.
    #[error(
        "unsupported export format {requested:?}; this engine exports JSON, YAML, GraphML \
         and DOT"
    )]
    UnsupportedFormat {
        /// The name that was asked for.
        requested: String,
    },

    /// The format cannot represent the content that was asked for.
    ///
    /// Distinct from a serialisation failure: the content is fine and the format is
    /// fine, and the two do not fit. A caller can act on this by choosing another
    /// format, which it cannot do for a malformed graph.
    #[error("{format} cannot represent {subject}: {reason}")]
    Unrepresentable {
        /// The format that cannot carry it.
        format: &'static str,
        /// What could not be carried.
        subject: String,
        /// Why the format cannot carry it.
        reason: String,
    },

    /// Serialisation failed.
    #[error("the {format} export could not be written: {detail}")]
    Serialisation {
        /// The format being written.
        format: &'static str,
        /// What the serialiser reported.
        detail: String,
    },

    /// The graph itself is not exportable.
    ///
    /// Carried rather than papered over: an export of a graph with a dangling endpoint
    /// would propagate the defect into a file that outlives the process, which is the
    /// outcome `graph.schema.json` exists to prevent.
    #[error("the graph cannot be exported: {detail}")]
    InvalidGraph {
        /// What is wrong with the graph.
        detail: String,
    },
}

impl ExportFailure {
    /// The error category this failure belongs to.
    #[must_use]
    pub const fn category(&self) -> ErrorCategory {
        match self {
            Self::UnsupportedFormat { .. } => ErrorCategory::Configuration,
            Self::Unrepresentable { .. } => ErrorCategory::Export,
            Self::Serialisation { .. } => ErrorCategory::Export,
            Self::InvalidGraph { .. } => ErrorCategory::Graph,
        }
    }

    /// Converts into the engine's error type.
    #[must_use]
    pub fn into_error(self) -> EngineError {
        match self {
            Self::UnsupportedFormat { .. } => EngineError::Configuration(self.to_string()),
            Self::Unrepresentable { .. } | Self::Serialisation { .. } => {
                EngineError::Export(self.to_string())
            },
            Self::InvalidGraph { .. } => EngineError::Graph(self.to_string()),
        }
    }
}

/// Returns the first failure in a list, or nothing when the list is empty.
///
/// The first rather than the last because validation is reported in the order the input
/// was read, and the earliest problem is the one a reader is most likely to fix first.
#[must_use]
pub fn first_failure(failures: Vec<ExportFailure>) -> Option<ExportFailure> {
    failures.into_iter().next()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unsupported_format_is_a_configuration_problem_not_an_export_problem() {
        // A caller acting on the category must be able to tell "you asked for something
        // that does not exist" from "the export failed", because the first is fixable by
        // changing an argument and the second is not.
        let failure = ExportFailure::UnsupportedFormat {
            requested: "toml".to_owned(),
        };
        assert_eq!(failure.category(), ErrorCategory::Configuration);
        assert!(
            failure.into_error().to_string().contains("JSON"),
            "the message names the formats that are available"
        );
    }

    #[test]
    fn an_unrepresentable_export_is_distinguishable_from_a_broken_graph() {
        let unrepresentable = ExportFailure::Unrepresentable {
            format: "graphml",
            subject: "the observation boundary".to_owned(),
            reason: "GraphML has no date-time attribute type".to_owned(),
        };
        let broken = ExportFailure::InvalidGraph {
            detail: "an edge names a node the graph does not contain".to_owned(),
        };
        assert_eq!(unrepresentable.category(), ErrorCategory::Export);
        assert_eq!(broken.category(), ErrorCategory::Graph);
        assert_ne!(unrepresentable.category(), broken.category());
    }

    #[test]
    fn the_first_failure_is_the_one_reported() {
        assert!(first_failure(Vec::new()).is_none());
        let reported = first_failure(vec![
            ExportFailure::InvalidGraph {
                detail: "first".to_owned(),
            },
            ExportFailure::InvalidGraph {
                detail: "second".to_owned(),
            },
        ])
        .expect("one failure");
        assert!(reported.to_string().contains("first"));
    }
}
