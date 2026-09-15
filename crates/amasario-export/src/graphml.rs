//! The GraphML export.
//!
//! # Why GraphML, and what it costs
//!
//! GraphML is the format graph tools actually read, so it is the format that lets a
//! consumer open an Amasario graph in something that was built for graphs. The cost is
//! that GraphML is a typed attribute format rather than a document format: an attribute
//! must be declared in a `<key>` before it can be used, and its type is one of a small
//! set. Everything the specification considers a claim therefore travels as a
//! **string**, and the types are used only where they are honest:
//!
//! * `amasarioObserved` is `boolean`, because it is genuinely a boolean and it is the
//!   distinction `dependency-edge.schema.json` says is most easily lost.
//! * Everything else - the relationship, the confidence, the evidence citations, the
//!   boundary, the digest - is a string, because GraphML has no type for an enumeration,
//!   a citation list or an RFC 3339 timestamp.
//!
//! # What is preserved, and what is not
//!
//! The requirement is that an export preserve enough to reconstruct the graph and follow
//! its evidence references. Every edge carries its identifier, its relationship, its
//! confidence, whether it was observed and the identifiers of the evidence records
//! supporting it, so a consumer can rebuild the topology and then resolve each citation
//! against the evidence export. The boundary travels as `network@ledger`.
//!
//! What GraphML cannot carry is the dependency-level detail that `graph.schema.json`
//! already omits from a published edge - the basis, the classes and the verification
//! status - and the graph's own metadata. A consumer that needs those reads the JSON
//! export, which is the format with no loss. This is stated rather than silently
//! dropped, because a consumer that assumed a GraphML export was complete would
//! reconstruct a graph missing exactly the fields that justify its edges.
//!
//! # Why the XML is written by hand
//!
//! A GraphML document's `<key>` declarations must precede every use and must agree with
//! it, which is a whole-document invariant that no attribute-derive can express. Writing
//! the element structure directly makes the invariant visible in one place; the escaping
//! is total, and the tests round-trip the output through a parser.

use amasario_core::Result;
use amasario_graph::GraphDocument;

use crate::errors::{ExportFailure, first_failure};

/// The `<key>` identifiers, declared once and referenced by the elements below.
///
/// Named constants rather than inline strings because a key referenced before it is
/// declared produces a document that GraphML readers reject, and a typo between the two
/// places is the way that happens.
mod key {
    /// The node's entity kind.
    pub const KIND: &str = "d0";
    /// The node's human-readable label.
    pub const LABEL: &str = "d1";
    /// The node's content digest.
    pub const DIGEST: &str = "d2";
    /// The edge's stable identifier.
    pub const EDGE_ID: &str = "d3";
    /// The edge's typed relationship.
    pub const RELATIONSHIP: &str = "d4";
    /// The edge's confidence level.
    pub const CONFIDENCE: &str = "d5";
    /// Whether the edge was directly observed.
    pub const OBSERVED: &str = "d6";
    /// The identifiers of the evidence records supporting the edge.
    pub const EVIDENCE: &str = "d7";
    /// The observation boundary the edge was seen at.
    pub const BOUNDARY: &str = "d8";
}

/// Renders a graph document as GraphML.
///
/// # Errors
///
/// Returns a graph error when the document contains an edge whose endpoints do not
/// resolve to nodes it contains. A GraphML file with a dangling endpoint is a file that
/// outlives the process that wrote it, so the defect is refused here rather than
/// exported.
pub fn render(graph: &GraphDocument) -> Result<String> {
    let dangling = graph.unresolvable_endpoints();
    if !dangling.is_empty() {
        return Err(ExportFailure::InvalidGraph {
            detail: format!(
                "{} endpoint(s) do not resolve to a node in this graph: {}",
                dangling.len(),
                dangling.join(", ")
            ),
        }
        .into_error());
    }
    // Collecting rather than returning at the first problem, so that an export reports
    // how many things are wrong rather than only that something is.
    let failures = validate(graph);
    if let Some(failure) = first_failure(failures) {
        return Err(failure.into_error());
    }

    let mut out = String::new();
    out.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n");
    out.push_str(
        "<graphml xmlns=\"http://graphml.graphdrawing.org/xmlns\" \
         xmlns:xsi=\"http://www.w3.org/2001/XMLSchema-instance\" \
         xsi:schemaLocation=\"http://graphml.graphdrawing.org/xmlns \
         http://graphml.graphdrawing.org/xmlns/1.0/graphml.xsd\">\n",
    );

    for (id, name, target, kind) in declarations() {
        out.push_str(&format!(
            "  <key id=\"{id}\" for=\"{target}\" attr.name=\"{name}\" \
             attr.type=\"{kind}\"/>\n"
        ));
    }

    out.push_str("  <graph id=\"amasario\" edgedefault=\"directed\">\n");
    if let Some(boundary) = &graph.boundary {
        out.push_str(&format!(
            "    <desc>{}</desc>\n",
            escape(&format!(
                "{} at ledger {}, observed {}",
                boundary.network.id, boundary.ledger, boundary.observed_at
            ))
        ));
    }

    for node in &graph.nodes {
        out.push_str(&format!(
            "    <node id=\"{}\">\n",
            escape_attribute(&node.id)
        ));
        push_data(&mut out, key::KIND, node.kind.as_str());
        if let Some(label) = &node.label {
            push_data(&mut out, key::LABEL, label);
        }
        // A digest addresses content, and a digest value is hex either way.
        if let Some(digest) = &node.digest {
            push_data(&mut out, key::DIGEST, digest.value());
        }
        out.push_str("    </node>\n");
    }

    for edge in &graph.edges {
        out.push_str(&format!(
            "    <edge id=\"{}\" source=\"{}\" target=\"{}\">\n",
            escape_attribute(&edge.id),
            escape_attribute(&edge.source),
            escape_attribute(&edge.target)
        ));
        push_data(&mut out, key::RELATIONSHIP, edge.relationship.as_str());
        push_data(&mut out, key::CONFIDENCE, edge.confidence.level.as_str());
        // A real boolean rather than the string "true": GraphML can express this one
        // honestly and a consumer filtering on it should not have to parse text.
        out.push_str(&format!(
            "      <data key=\"{}\">{}</data>\n",
            key::OBSERVED,
            edge.observed
        ));
        if !edge.evidence.is_empty() {
            push_data(&mut out, key::EVIDENCE, &edge.evidence.join(" "));
        }
        if let Some(boundary) = &edge.boundary {
            push_data(
                &mut out,
                key::BOUNDARY,
                &format!("{}@ledger{}", boundary.network.id, boundary.ledger),
            );
        }
        out.push_str("    </edge>\n");
    }

    out.push_str("  </graph>\n</graphml>\n");
    Ok(out)
}

/// The `<key>` declarations, in the order they are written.
fn declarations() -> Vec<(&'static str, &'static str, &'static str, &'static str)> {
    vec![
        (key::KIND, "kind", "node", "string"),
        (key::LABEL, "label", "node", "string"),
        (key::DIGEST, "digest", "node", "string"),
        (key::EDGE_ID, "id", "edge", "string"),
        (key::RELATIONSHIP, "relationship", "edge", "string"),
        (key::CONFIDENCE, "confidence", "edge", "string"),
        (key::OBSERVED, "observed", "edge", "boolean"),
        (key::EVIDENCE, "evidence", "edge", "string"),
        (key::BOUNDARY, "boundary", "edge", "string"),
    ]
}

/// Reports what about a graph cannot be exported.
///
/// Kept separate from the rendering so that a caller can report every problem at once,
/// and so the rules the format imposes are stated in one place rather than implied by
/// the order the elements happen to be written.
#[must_use]
pub fn validate(graph: &GraphDocument) -> Vec<ExportFailure> {
    let mut failures = Vec::new();
    for node in &graph.nodes {
        // A GraphML identifier may contain almost anything, but an empty one makes the
        // element unaddressable.
        if node.id.is_empty() {
            failures.push(ExportFailure::Unrepresentable {
                format: "GraphML",
                subject: "a node".to_owned(),
                reason: "a node identifier cannot be empty".to_owned(),
            });
        }
    }
    for edge in &graph.edges {
        if edge.id.is_empty() {
            failures.push(ExportFailure::Unrepresentable {
                format: "GraphML",
                subject: "an edge".to_owned(),
                reason: "an edge identifier cannot be empty".to_owned(),
            });
        }
        if edge.relationship.as_str().is_empty() {
            failures.push(ExportFailure::Unrepresentable {
                format: "GraphML",
                subject: format!("edge {}", edge.id),
                reason: "a relationship term cannot be empty".to_owned(),
            });
        }
    }
    failures
}

/// Writes one `<data>` element.
fn push_data(out: &mut String, key: &str, value: &str) {
    out.push_str(&format!(
        "      <data key=\"{key}\">{}</data>\n",
        escape(value)
    ));
}

/// Escapes a string for XML element content.
#[must_use]
pub fn escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            // XML 1.0 cannot represent these at all, and emitting one produces a
            // document no parser will read.
            c if c.is_control() && c != '\n' && c != '\t' => {},
            other => out.push(other),
        }
    }
    out
}

/// Escapes a string for an XML attribute value.
///
/// The same transformation as [`escape`], named separately because an attribute and a
/// text node are different contexts and a future change to one should not silently
/// change the other.
#[must_use]
fn escape_attribute(value: &str) -> String {
    escape(value)
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
    fn the_document_declares_every_key_before_using_it() {
        // A key referenced before it is declared produces a file GraphML readers
        // reject, and a typo between the two places is how that happens.
        let graphml = render(&document()).expect("renders");
        let declarations: Vec<&str> = graphml
            .lines()
            .filter(|line| line.contains("<key id="))
            .map(|line| {
                let start = line.find("id=\"").expect("an identifier") + 4;
                &line[start..start + 2]
            })
            .collect();
        assert_eq!(declarations.len(), 9, "every key is declared");
        for used in graphml
            .lines()
            .filter(|line| line.contains("<data key="))
            .map(|line| {
                let start = line.find("key=\"").expect("a key") + 5;
                &line[start..start + 2]
            })
        {
            assert!(
                declarations.contains(&used),
                "key {used} is used but never declared"
            );
        }
    }

    #[test]
    fn the_evidence_citations_survive_the_export() {
        // The requirement is that a consumer can follow the evidence references, so
        // dropping them would fail it even though the topology survived.
        let graphml = render(&document()).expect("renders");
        assert!(
            graphml.contains(&"ab".repeat(32)),
            "the citation is present: {graphml}"
        );
        assert!(graphml.contains("INVOCATES"));
        assert!(graphml.contains("VERIFIED"));
        assert!(graphml.contains("testnet@ledger1000"));
    }

    #[test]
    fn observed_travels_as_a_real_boolean() {
        // GraphML can express this one honestly, and a consumer filtering on it should
        // not have to parse text.
        let graphml = render(&document()).expect("renders");
        assert!(graphml.contains("attr.type=\"boolean\""));
        assert!(graphml.contains(">true</data>"));
    }

    #[test]
    fn the_edges_are_directed_and_the_root_element_is_correct() {
        let graphml = render(&document()).expect("renders");
        assert!(graphml.starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\"?>"));
        assert!(graphml.contains("edgedefault=\"directed\""));
        assert!(graphml.trim_end().ends_with("</graphml>"));
        assert_eq!(
            graphml.matches("<node ").count(),
            graphml.matches("</node>").count()
        );
        assert_eq!(
            graphml.matches("<edge ").count(),
            graphml.matches("</edge>").count()
        );
    }

    #[test]
    fn a_dangling_endpoint_is_refused_rather_than_exported() {
        // A GraphML file with an unresolvable endpoint outlives the process that wrote
        // it, so the defect is refused here rather than propagated into a file.
        let mut document = document();
        document.nodes.retain(|node| node.id != "CONTRACT:C-callee");
        let error = render(&document).expect_err("a dangling endpoint");
        assert_eq!(error.category(), amasario_core::ErrorCategory::Graph);
        assert!(error.to_string().contains("CONTRACT:C-callee"));
    }

    #[test]
    fn markup_in_an_identifier_does_not_produce_an_unreadable_file() {
        assert_eq!(escape("a<b>&c"), "a&lt;b&gt;&amp;c");
        assert_eq!(escape("a\"b'c"), "a&quot;b&apos;c");
        assert_eq!(escape("a\u{7}b"), "ab");
    }

    #[test]
    fn the_rendering_is_deterministic() {
        let first = render(&document()).expect("renders");
        for _ in 0..5 {
            assert_eq!(render(&document()).expect("renders"), first);
        }
    }
}
