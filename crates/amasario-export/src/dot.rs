//! The DOT export: the one DOT renderer in the engine.
//!
//! # Why there is only one
//!
//! `amasario-report` renders DOT as well, and it calls this function rather than
//! carrying its own. Two renderers for one format drift, and a reader comparing a
//! report's picture with an exported picture would have no way to tell which of the two
//! was wrong.
//!
//! # What the picture has to carry
//!
//! `graph.schema.json` requires an edge to preserve its evidence citations and its
//! confidence, and an export that dropped either would not satisfy the requirement that
//! an export "preserve enough information to reconstruct the graph and its evidence
//! references". DOT has no schema for any of that, so the renderer uses three
//! mechanisms:
//!
//! * **Line style** carries `observed`. An inferred edge is dashed and an observed one
//!   solid, because this is the distinction `dependency-edge.schema.json` calls the one
//!   most easily lost in serialisation, and a picture is where it is easiest to lose.
//! * **The edge label** carries the relationship and the confidence level.
//! * **Custom attributes** carry the evidence citations and the boundary. Graphviz
//!   ignores attributes it does not know, so `evidence="..."` produces a valid file and
//!   survives a round trip through a Graphviz parser for a consumer that reads the
//!   source rather than the layout.
//!
//! A truncated graph also gets a node saying so. A picture is where completeness is most
//! easily assumed and least easily checked.

use amasario_graph::GraphDocument;

/// Renders a graph document as DOT.
///
/// Never fails: every value a graph document can hold is representable in a quoted DOT
/// string, and the escaping below is total.
#[must_use]
pub fn render(graph: &GraphDocument) -> String {
    let mut out = String::from("digraph amasario {\n");
    out.push_str("  rankdir=LR;\n");
    // A graph-level label rather than a node, because a node would appear in any
    // downstream analysis of the picture as though it were an entity of the graph.
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
            "  \"{}\" [label=\"{}\", amasarioKind=\"{}\"",
            escape(&node.id),
            escape(&node.id),
            escape(node.kind.as_str())
        ));
        if let Some(label) = &node.label {
            out.push_str(&format!(", amasarioLabel=\"{}\"", escape(label)));
        }
        if let Some(digest) = &node.digest {
            out.push_str(&format!(", amasarioDigest=\"{}\"", escape(digest.value())));
        }
        out.push_str("];\n");
    }

    for edge in &graph.edges {
        let style = if edge.observed { "solid" } else { "dashed" };
        let evidence = edge
            .evidence
            .iter()
            .map(|citation| citation.as_str())
            .collect::<Vec<&str>>()
            .join(",");
        out.push_str(&format!(
            "  \"{}\" -> \"{}\" [label=\"{}\\n{}\", style={}, amasarioEdge=\"{}\", \
             amasarioObserved=\"{}\", amasarioConfidence=\"{}\"",
            escape(&edge.source),
            escape(&edge.target),
            escape(edge.relationship.as_str()),
            escape(edge.confidence.level.as_str()),
            style,
            escape(&edge.id),
            edge.observed,
            escape(edge.confidence.level.as_str())
        ));
        if !evidence.is_empty() {
            out.push_str(&format!(", amasarioEvidence=\"{}\"", escape(&evidence)));
        }
        if let Some(boundary) = &edge.boundary {
            out.push_str(&format!(
                ", amasarioBoundary=\"{}@ledger{}\"",
                escape(&boundary.network.id),
                boundary.ledger
            ));
        }
        out.push_str("];\n");
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
/// backslash ends the string early - which produces a file Graphviz rejects rather than
/// a picture with a wrong label.
#[must_use]
pub fn escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            // A carriage return has no DOT escape and no place in a label.
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
    fn the_export_carries_the_evidence_citations() {
        // The requirement is that an export preserve enough to follow the evidence
        // references, and nothing in DOT does that by itself.
        let dot = render(&document());
        assert!(
            dot.contains("amasarioEvidence=\"abab"),
            "the citation is on the edge: {dot}"
        );
        assert!(dot.contains("amasarioEdge=\""));
        assert!(dot.contains("amasarioConfidence=\"VERIFIED\""));
    }

    #[test]
    fn an_inferred_edge_is_dashed_and_an_observed_one_solid() {
        let mut document = document();
        assert!(render(&document).contains("style=solid"));
        document.edges[0].observed = false;
        let dot = render(&document);
        assert!(dot.contains("style=dashed"));
        assert!(!dot.contains("style=solid"));
        assert!(dot.contains("amasarioObserved=\"false\""));
    }

    #[test]
    fn the_document_is_well_formed() {
        let dot = render(&document());
        assert!(dot.starts_with("digraph amasario {"));
        assert!(dot.trim_end().ends_with('}'));
        let opens = dot.matches('[').count();
        let closes = dot.matches(']').count();
        assert_eq!(opens, closes, "every attribute list is closed");
        assert_eq!(
            dot.matches('"').count() % 2,
            0,
            "every string is terminated"
        );
    }

    #[test]
    fn a_truncated_graph_says_so_in_the_picture() {
        let mut document = document();
        if let Some(metadata) = document.metadata.as_mut() {
            metadata.truncated = true;
        }
        assert!(render(&document).contains("amasario:truncated"));
    }

    #[test]
    fn a_value_containing_a_quote_does_not_break_the_file() {
        assert_eq!(escape("a\"b"), "a\\\"b");
        assert_eq!(escape("a\\b"), "a\\\\b");
        assert_eq!(escape("a\nb"), "a\\nb");
        assert_eq!(escape("a\rb"), "ab");
    }

    #[test]
    fn the_rendering_is_deterministic() {
        let first = render(&document());
        for _ in 0..5 {
            assert_eq!(render(&document()), first);
        }
    }
}
