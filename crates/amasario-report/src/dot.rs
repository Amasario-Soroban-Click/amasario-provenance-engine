//! The DOT renderer, for a picture of the graph.
//!
//! # What DOT is for here, and what it cannot say
//!
//! DOT draws topology. It has no way to express that an edge was inferred rather than
//! observed, that its confidence was assembled from a weaker link, or that the analysis
//! stopped early - and a graph picture that silently omitted those facts would be the
//! most persuasive form of the overstatement the specification forbids. Three decisions
//! follow:
//!
//! * An inferred edge is drawn **dashed** and an observed one solid, so the distinction
//!   that `dependency-edge.schema.json` calls out as the one most easily lost survives
//!   the rendering.
//! * The confidence level is written on the edge, because a reader comparing two edges
//!   needs to see that they do not carry the same weight.
//! * A truncated graph carries a node stating that it is truncated. A picture is where
//!   completeness is most easily assumed and least easily checked.
//!
//! # Why the graph and not the report
//!
//! [`render`] draws [`Report::graph`], which is held in memory and deliberately absent
//! from the published document: `report.schema.json` has no field for a graph. Drawing
//! the relationship findings instead would produce a picture of the explanations rather
//! than of the topology. When a report carries no graph, the renderer says so rather
//! than emitting an empty digraph, because an empty picture reads as "nothing is
//! related" rather than "this report has no graph".

use amasario_core::Result;

use amasario_graph::GraphDocument;

use crate::model::Report;

/// Renders a report's graph as DOT.
///
/// # Errors
///
/// Returns a report error when the report carries no graph, because an empty digraph
/// would be read as a finding.
pub fn render(report: &Report) -> Result<String> {
    let graph = report.graph.as_ref().ok_or_else(|| {
        amasario_core::EngineError::Report(
            "this report carries no graph, so there is nothing to draw as DOT; a report \
             document has no graph field, and an empty digraph would read as a finding \
             about the topology rather than as an absence of one"
                .to_owned(),
        )
    })?;
    Ok(render_graph(graph))
}

/// Renders a graph document as DOT.
#[must_use]
pub fn render_graph(graph: &GraphDocument) -> String {
    let mut out = String::from("digraph amasario {\n");
    out.push_str("  rankdir=LR;\n");
    // The label is set on the graph rather than emitted as a node because a node would
    // appear in any downstream analysis of the picture as though it were an entity.
    if let Some(boundary) = &graph.boundary {
        out.push_str(&format!(
            "  label=\"{}\";\n  labelloc=\"t\";\n",
            escape(&format!(
                "{} at ledger {}",
                boundary.network.id, boundary.ledger
            ))
        ));
    }

    for node in &graph.nodes {
        out.push_str(&format!(
            "  \"{}\" [label=\"{}\"];\n",
            escape(&node.id),
            escape(&node.id)
        ));
    }

    for edge in &graph.edges {
        // Dashed for an inference, solid for an observation. This is the one distinction
        // a reader of a picture can check at a glance, and it is also the one
        // `dependency-edge.schema.json` says is most easily lost in serialisation.
        let style = if edge.observed { "solid" } else { "dashed" };
        out.push_str(&format!(
            "  \"{}\" -> \"{}\" [label=\"{}\\n{}\", style={}];\n",
            escape(&edge.source),
            escape(&edge.target),
            escape(edge.relationship.as_str()),
            escape(edge.confidence.level.as_str()),
            style
        ));
    }

    if graph
        .metadata
        .as_ref()
        .is_some_and(|metadata| metadata.truncated)
    {
        out.push_str(
            "  \"amasario:truncated\" [label=\"the analysis was bounded\\nand did not run \
             to exhaustion\", shape=note];\n",
        );
    }

    out.push_str("}\n");
    out
}

/// Escapes a string for a DOT double-quoted context.
///
/// DOT has no escape for a newline inside a quoted label, and an unescaped quote or
/// backslash ends the string early and produces a file Graphviz rejects rather than a
/// picture with a wrong label.
fn escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => {},
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use amasario_core::{
        Basis, EntityKind, EntityRef, EvidenceType, LedgerSequence, Network, NetworkType,
        ObservationBoundary, Relationship,
    };
    use amasario_dependency::{Candidate, EvidenceRef, resolve};

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
        GraphDocument::of(&amasario_graph::Graph::from_dependencies("g1", &set).expect("a graph"))
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
