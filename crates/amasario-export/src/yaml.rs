//! The YAML export.
//!
//! # Why YAML is a format here rather than a JSON alias
//!
//! A graph document is read by people as often as by programs - the specification's own
//! fixtures, models and rules are YAML - so an export that only produced JSON would push
//! a reader towards a tool. The document is the same document: the field names, the
//! nesting and the omission of absent optional fields all match the JSON export, so a
//! consumer can convert between them without learning two shapes.
//!
//! # The one difference that matters
//!
//! YAML distinguishes a missing key from a key whose value is null, and so does the
//! specification. `serde_norway` writes an absent optional field as nothing at all when
//! the field is `skip_serializing_if`-annotated, which every optional field in the graph
//! document is, so "not established" does not become "established as null".
//!
//! # Why not `serde_yaml`
//!
//! It is unmaintained. `serde_yml`, the fork that succeeded it, has not attracted the
//! maintenance its predecessor lost. `serde_norway` is the maintained continuation of
//! the original codebase, which matters for a parser that reads text a person wrote.

use amasario_core::Result;
use amasario_graph::GraphDocument;

use crate::errors::ExportFailure;

/// Renders a graph document as YAML.
///
/// # Errors
///
/// Returns an export error when the document cannot be serialised, which for this shape
/// would be a defect in the engine.
pub fn render(graph: &GraphDocument) -> Result<String> {
    serde_norway::to_string(graph).map_err(|error| {
        ExportFailure::Serialisation {
            format: "YAML",
            detail: error.to_string(),
        }
        .into_error()
    })
}

/// Renders any serialisable value as YAML.
///
/// # Errors
///
/// As [`render`].
pub fn render_value<T: serde::Serialize + ?Sized>(value: &T) -> Result<String> {
    serde_norway::to_string(value).map_err(|error| {
        ExportFailure::Serialisation {
            format: "YAML",
            detail: error.to_string(),
        }
        .into_error()
    })
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
    fn the_yaml_carries_the_same_document_as_the_json() {
        // One document, two spellings. A consumer must not have to learn two shapes.
        let yaml = render(&document()).expect("renders");
        let json: serde_json::Value =
            serde_json::from_str(&document().canonical_json().expect("renders"))
                .expect("valid JSON");
        let from_yaml: serde_json::Value =
            serde_norway::from_str(&yaml).expect("the YAML parses as the same document");
        assert_eq!(from_yaml, json);
    }

    #[test]
    fn the_version_stamp_survives() {
        let yaml = render(&document()).expect("renders");
        assert!(yaml.contains("apiVersion: amasario.dev/v1"));
        assert!(yaml.contains("specVersion: 1.0.0"));
    }

    #[test]
    fn an_absent_optional_field_is_absent_rather_than_null() {
        // YAML distinguishes a missing key from a null-valued one, and so does the
        // specification: "not established" must not become "established as null".
        let yaml = render(&document()).expect("renders");
        assert!(
            !yaml.contains("attributes: null"),
            "an absent optional field must be omitted: {yaml}"
        );
        assert!(!yaml.contains(": null"));
    }

    #[test]
    fn the_rendering_is_deterministic() {
        let first = render(&document()).expect("renders");
        for _ in 0..5 {
            assert_eq!(render(&document()).expect("renders"), first);
        }
    }
}
