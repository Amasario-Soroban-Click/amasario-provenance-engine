//! The DOT rendering of a report's graph.
//!
//! # Why this module is thin
//!
//! `amasario-export` holds the engine's DOT renderer, and this module calls it. Two
//! renderers for one format drift, and a reader comparing a report's picture with an
//! exported picture would have no way to tell which of the two was wrong. What belongs
//! here is only the part that is about a *report* rather than about a graph: which graph
//! to draw, and what to say when there is none.
//!
//! # What DOT cannot express, and what the renderer does about it
//!
//! DOT draws topology. It has no way to express that an edge was inferred rather than
//! observed, that its confidence was assembled from a weaker link, or that the analysis
//! stopped early - and a graph picture that silently omitted those would be the most
//! persuasive form of the overstatement the specification forbids. The renderer answers
//! each: an inferred edge is drawn dashed, the confidence level is written on the edge,
//! the evidence citations travel as attributes, and a truncated graph gets a node saying
//! so. See [`amasario_export::dot`] for the details.
//!
//! # Why the graph and not the report
//!
//! [`render`] draws [`Report::graph`], which is held in memory and deliberately absent
//! from the published document: `report.schema.json` has no field for a graph, and
//! `additionalProperties: false` means adding one would invalidate the document. Drawing
//! the relationship findings instead would produce a picture of the explanations rather
//! than of the topology.
//!
//! When a report carries no graph the renderer fails rather than emitting an empty
//! digraph, because an empty picture reads as "nothing is related" rather than as "there
//! is nothing to draw".

use amasario_core::{EngineError, Result};
use amasario_graph::GraphDocument;

use crate::model::Report;

/// Renders a report's graph as DOT.
///
/// # Errors
///
/// Returns a report error when the report carries no graph, because an empty digraph
/// would be read as a finding about the topology rather than as an absence of one.
pub fn render(report: &Report) -> Result<String> {
    let graph = report.graph.as_ref().ok_or_else(|| {
        EngineError::Report(
            "this report carries no graph, so there is nothing to draw as DOT; a report \
             document has no graph field, and an empty digraph would read as a finding \
             about the topology rather than as an absence of one"
                .to_owned(),
        )
    })?;
    Ok(render_graph(graph))
}

/// Renders a graph document as DOT.
///
/// A thin alias for the export crate's renderer, kept so that a caller holding a report
/// does not have to reach into another crate to draw its graph.
#[must_use]
pub fn render_graph(graph: &GraphDocument) -> String {
    amasario_export::dot::render(graph)
}

/// Escapes a string for a DOT double-quoted context.
///
/// Re-exported from the export crate so that the one implementation is used in both
/// places, rather than each crate carrying its own escaping that could disagree.
#[must_use]
pub fn escape(value: &str) -> String {
    amasario_export::dot::escape(value)
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
    fn a_graph_becomes_a_well_formed_digraph() {
        let dot = render_graph(&document());
        assert!(dot.starts_with("digraph amasario {"));
        assert!(dot.trim_end().ends_with('}'));
        assert!(dot.contains("\"CONTRACT:C-subject\" -> \"CONTRACT:C-callee\""));
        assert!(dot.contains("INVOCATES"));
        assert!(dot.contains("VERIFIED"));
        assert!(dot.contains("solid"), "an observed edge is solid");
    }

    #[test]
    fn an_inferred_edge_is_drawn_dashed() {
        // Derived from the basis rather than passed in, so a picture cannot show an
        // inference as an observation.
        let mut document = document();
        document.edges[0].observed = false;
        let dot = render_graph(&document);
        assert!(dot.contains("dashed"));
        assert!(!dot.contains("style=solid"));
    }

    #[test]
    fn a_truncated_graph_says_so_in_the_picture() {
        let mut document = document();
        if let Some(metadata) = document.metadata.as_mut() {
            metadata.truncated = true;
        }
        let dot = render_graph(&document);
        assert!(
            dot.contains("amasario:truncated"),
            "a picture is where completeness is most easily assumed"
        );
    }

    #[test]
    fn the_report_and_the_export_produce_the_same_picture() {
        // One renderer, two callers. If these diverged, a reader comparing a report with
        // an export would have no way to tell which was wrong.
        let graph = document();
        assert_eq!(
            render_graph(&graph),
            amasario_export::dot::render(&graph),
            "the report renderer must be the export renderer"
        );
    }

    #[test]
    fn a_report_without_a_graph_is_refused_rather_than_drawn_empty() {
        let report = Report::new(EntityRef::new(EntityKind::Contract, "C-x").expect("a reference"));
        let error = render(&report).expect_err("no graph to draw");
        assert!(error.to_string().contains("no graph"));
    }

    #[test]
    fn a_label_containing_a_quote_does_not_break_the_file() {
        // Graphviz rejects a file with an unterminated string, so an unescaped quote
        // would produce no picture at all rather than a picture with a wrong label.
        assert_eq!(escape("a\"b"), "a\\\"b");
        assert_eq!(escape("a\\b"), "a\\\\b");
        assert_eq!(escape("a\nb"), "a\\nb");
    }
}
