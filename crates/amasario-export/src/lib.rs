//! Machine-readable export: JSON, YAML, GraphML and DOT.
//!
//! # The requirement this crate is written against
//!
//! An export must "preserve enough information to reconstruct the graph and its evidence
//! references". That is a stronger statement than "write the graph out", and it is what
//! decides the design here: every format carries the edge identifiers and the evidence
//! citations, because a consumer that can rebuild the topology but cannot follow a
//! citation back to the record supporting it has a drawing rather than an analysis.
//!
//! # The formats, and what each can honestly carry
//!
//! | Format | Loss | Read by |
//! | --- | --- | --- |
//! | JSON | none | programs |
//! | YAML | none | people and programs |
//! | GraphML | the dependency-level detail `graph.schema.json` already omits from a published edge, and the graph metadata | graph tools |
//! | DOT | as GraphML, rendered for a reader | people and Graphviz |
//!
//! The two lossy formats are lossy in the same way and for the same reason: neither has
//! a type for an enumeration, a citation list or an RFC 3339 timestamp, so everything
//! that is a claim travels as a string and the one genuinely boolean field - whether an
//! edge was observed - travels as a boolean. What they cannot carry is stated in each
//! module rather than left for a consumer to discover by finding a field missing.
//!
//! # One DOT renderer
//!
//! [`dot`] holds the engine's only DOT implementation, and `amasario-report` calls it.
//! Two renderers for one format drift, and a reader comparing a report's picture with an
//! exported picture would have no way to tell which of the two was wrong.
//!
//! # What this crate will not do
//!
//! It will not export a graph with a dangling endpoint. A file outlives the process that
//! wrote it, so a defect that `Graph::validate` would have caught in memory would be
//! propagated into something a consumer reads later with no way to tell. GraphML refuses
//! it outright; the other formats carry the graph document, whose own validation the
//! caller is expected to have run.
//!
//! # Example
//!
//! ```
//! use amasario_core::{
//!     Basis, EntityKind, EntityRef, EvidenceType, LedgerSequence, Network, NetworkType,
//!     ObservationBoundary, Relationship,
//! };
//! use amasario_dependency::{Candidate, EvidenceRef, resolve};
//! use amasario_export::{Format, export_graph};
//! use amasario_graph::{Graph, GraphDocument};
//!
//! # fn main() -> amasario_core::Result<()> {
//! let boundary = ObservationBoundary {
//!     network: Network::new("testnet", NetworkType::Testnet, "Test SDF Network ; September 2015")?,
//!     ledger: LedgerSequence::new(1_000)?,
//!     observed_at: "2026-09-15T00:00:00Z".to_owned(),
//!     spec_version: None,
//! };
//! let subject = EntityRef::new(EntityKind::Contract, "C-subject")?;
//! let candidate = Candidate::new(
//!     subject.clone(),
//!     EntityRef::new(EntityKind::Contract, "C-callee")?,
//!     Relationship::Invocates,
//!     Basis::ObservedInvocation,
//!     vec![EvidenceRef::new(EvidenceType::Transaction, "ab".repeat(32))?],
//! )?
//! .observed_at(boundary.clone())
//! .with_outcome(Some(true));
//!
//! let set = resolve(subject, Some(boundary), &[candidate], 5)?;
//! let graph = GraphDocument::of(&Graph::from_dependencies("g1", &set)?);
//!
//! let json = export_graph(&graph, Format::Json)?;
//! assert!(json.contains("INVOCATES"));
//! // Every format carries the citation, because an export that cannot be followed
//! // back to its evidence does not satisfy the requirement.
//! let dot = export_graph(&graph, Format::Dot)?;
//! assert!(dot.contains(&"ab".repeat(32)));
//! let graphml = export_graph(&graph, Format::GraphML)?;
//! assert!(graphml.contains(&"ab".repeat(32)));
//! # Ok(())
//! # }
//! ```

#![deny(missing_docs)]
#![deny(unsafe_code)]

pub mod dot;
pub mod errors;
pub mod graphml;
pub mod json;
pub mod yaml;

use amasario_core::{EngineError, Result};
use amasario_graph::GraphDocument;

pub use errors::{ExportFailure, first_failure};

/// A format a graph can be exported in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum Format {
    /// The graph document the specification defines, as canonical JSON.
    Json,
    /// The same document, as YAML.
    Yaml,
    /// The graph, as GraphML, for graph tools.
    GraphML,
    /// The graph, as Graphviz DOT.
    Dot,
}

impl Format {
    /// The stable name, used on the command line and in output.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Json => "json",
            Self::Yaml => "yaml",
            Self::GraphML => "graphml",
            Self::Dot => "dot",
        }
    }

    /// Every format, in a stable order.
    #[must_use]
    pub const fn all() -> &'static [Self] {
        &[Self::Json, Self::Yaml, Self::GraphML, Self::Dot]
    }

    /// The customary file extension, without a dot.
    #[must_use]
    pub const fn extension(self) -> &'static str {
        match self {
            Self::Json => "json",
            Self::Yaml => "yaml",
            Self::GraphML => "graphml",
            Self::Dot => "dot",
        }
    }

    /// Whether the format can carry a document with no loss.
    ///
    /// Exposed because a caller choosing a format for archival purposes needs to know,
    /// and because stating it in code is what keeps the module documentation's claim
    /// about each format checkable.
    #[must_use]
    pub const fn is_lossless(self) -> bool {
        matches!(self, Self::Json | Self::Yaml)
    }

    /// Parses a format name.
    ///
    /// # Errors
    ///
    /// Returns a configuration error naming the formats that are accepted.
    pub fn parse(name: &str) -> Result<Self> {
        let normalised = name.trim().to_ascii_lowercase();
        let found = match normalised.as_str() {
            "json" => Some(Self::Json),
            "yaml" | "yml" => Some(Self::Yaml),
            "graphml" | "xml" => Some(Self::GraphML),
            "dot" | "graphviz" => Some(Self::Dot),
            _ => None,
        };
        found.ok_or_else(|| {
            ExportFailure::UnsupportedFormat {
                requested: name.to_owned(),
            }
            .into_error()
        })
    }

    /// Every name this format accepts, for help text.
    #[must_use]
    pub const fn accepted_names(self) -> &'static [&'static str] {
        match self {
            Self::Json => &["json"],
            Self::Yaml => &["yaml", "yml"],
            Self::GraphML => &["graphml", "xml"],
            Self::Dot => &["dot", "graphviz"],
        }
    }
}

impl std::fmt::Display for Format {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Exports a graph document in the requested format.
///
/// # Errors
///
/// Returns an export error when the document cannot be written in the format - including
/// a graph error when GraphML would have to emit a dangling endpoint, which is refused
/// rather than written.
pub fn export_graph(graph: &GraphDocument, format: Format) -> Result<String> {
    match format {
        Format::Json => json::render(graph),
        Format::Yaml => yaml::render(graph),
        Format::GraphML => graphml::render(graph),
        Format::Dot => Ok(dot::render(graph)),
    }
}

/// Converts a failure into the engine's error type.
///
/// Present so that a caller inside this crate and a caller outside it produce the same
/// error for the same condition.
#[must_use]
pub fn to_engine_error(failure: ExportFailure) -> EngineError {
    failure.into_error()
}

#[cfg(test)]
mod tests {
    use super::*;
    use amasario_core::{
        Basis, EntityKind, EntityRef, EvidenceType, LedgerSequence, Network, NetworkType,
        ObservationBoundary, Relationship,
    };
    use amasario_dependency::{Candidate, EvidenceRef, resolve};
    use amasario_graph::Graph;

    fn document() -> GraphDocument {
        let boundary = ObservationBoundary {
            network: Network::new(
                "testnet",
                NetworkType::Testnet,
                "Test SDF Network ; September 2015",
            )
            .expect("a network"),
            ledger: LedgerSequence::new(1_000).expect("a ledger"),
            observed_at: "2026-09-15T00:00:00Z".to_owned(),
            spec_version: None,
        };
        let subject = EntityRef::new(EntityKind::Contract, "C-subject").expect("a reference");
        let candidate = Candidate::new(
            subject.clone(),
            EntityRef::new(EntityKind::Contract, "C-callee").expect("a reference"),
            Relationship::Invocates,
            Basis::ObservedInvocation,
            vec![EvidenceRef::new(EvidenceType::Transaction, "ab".repeat(32)).expect("a citation")],
        )
        .expect("a candidate")
        .observed_at(boundary.clone())
        .with_outcome(Some(true));
        let set = resolve(subject, Some(boundary), &[candidate], 5).expect("resolves");
        GraphDocument::of(&Graph::from_dependencies("g1", &set).expect("a graph"))
    }

    #[test]
    fn the_vocabulary_is_reachable_from_the_crate_root() {
        assert_eq!(Format::Json.as_str(), "json");
        assert_eq!(Format::GraphML.extension(), "graphml");
        assert!(Format::Json.is_lossless());
        assert!(Format::Yaml.is_lossless());
        assert!(!Format::GraphML.is_lossless());
        assert_eq!(Format::parse("yml").expect("a format"), Format::Yaml);
        assert_eq!(Format::parse("graphviz").expect("a format"), Format::Dot);
    }

    #[test]
    fn an_unknown_format_is_a_configuration_failure() {
        let error = Format::parse("parquet").expect_err("not an export format");
        assert_eq!(
            error.category(),
            amasario_core::ErrorCategory::Configuration
        );
        assert!(error.to_string().contains("GraphML"), "got: {error}");
    }

    #[test]
    fn every_format_carries_the_evidence_citations() {
        // The requirement is that an export preserve enough to follow the evidence
        // references. A format that dropped them would fail it.
        let graph = document();
        for format in Format::all() {
            let exported = export_graph(&graph, *format).expect("exports");
            assert!(
                exported.contains(&"ab".repeat(32)),
                "{format} dropped the evidence citation"
            );
            assert!(!exported.is_empty(), "{format} produced nothing");
        }
    }

    #[test]
    fn every_format_names_the_relationship_and_the_endpoints() {
        let graph = document();
        for format in Format::all() {
            let exported = export_graph(&graph, *format).expect("exports");
            assert!(
                exported.contains("INVOCATES"),
                "{format} lost the relationship"
            );
            assert!(exported.contains("C-subject"), "{format} lost an endpoint");
            assert!(exported.contains("C-callee"), "{format} lost an endpoint");
        }
    }

    #[test]
    fn the_lossy_formats_say_so_in_their_type_rather_than_only_in_prose() {
        assert!(Format::all().iter().filter(|f| !f.is_lossless()).count() == 2);
    }

    #[test]
    fn every_export_is_deterministic() {
        let graph = document();
        for format in Format::all() {
            let first = export_graph(&graph, *format).expect("exports");
            for _ in 0..3 {
                assert_eq!(
                    export_graph(&graph, *format).expect("exports"),
                    first,
                    "{format} is not deterministic"
                );
            }
        }
    }
}
