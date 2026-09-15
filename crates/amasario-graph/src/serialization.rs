//! The graph document the specification defines, and the canonical way to write it.
//!
//! # Why the document is a separate type from the graph
//!
//! `graph.schema.json` sets `additionalProperties: false` on a node and on an edge, and
//! its edge is narrower than a dependency record: it carries an identifier, the two
//! endpoints, the relationship, the evidence citations, the confidence, the boundary and
//! whether the edge was observed - and not the basis, the classes or the verification
//! status, which belong to the dependency document. The difference is a choice about a
//! *publication*, not about a graph, so it lives in a publication type.
//! [`EdgeDocument`] is that type, and it is deliberately unable to represent a full edge:
//! a type that could would tempt a caller to treat the two as interchangeable.
//!
//! # Why evidence is published as citations and not as records
//!
//! `provenance.schema.json` defines `evidenceRef` as a string - the identifier of an
//! evidence record - and the graph's edge references it. The engine's internal
//! [`amasario_dependency::EvidenceRef`] carries the kind alongside the identifier,
//! because classification needs it. Publishing the identifier alone is what the schema
//! asks for and is also the right shape: an evidence record is defined once in the
//! evidence layer, and a graph that repeated it per edge would be a second copy that
//! could disagree.
//!
//! # Why the serialization is canonical rather than merely valid
//!
//! Two runs over the same evidence must produce the same bytes, or a snapshot diff
//! reports every edge as changed and every consumer of a hash of the document sees
//! spurious differences. Canonical form here means: nodes and edges in the graph's own
//! canonical order, evidence citations in the order the dependency recorded them, and
//! no field emitted when its value is absent - so that "not established" and "established
//! as empty" cannot be confused, and an absent optional field does not appear as `null`.
//!
//! # Round-tripping is intentionally partial
//!
//! [`GraphDocument::from_json`] reads a document back, and what comes back is exactly
//! what the document carries: the topology, the relationships, the citations, the
//! confidence, the boundary and the observed flag. It is **not** a [`crate::graph::Graph`],
//! because a graph's edges are dependencies and a dependency's basis cannot be recovered
//! from a document that does not state one. Reconstructing a basis would mean inventing
//! one, which is the single thing this engine must never do. A consumer that needs the
//! dependency-level detail reads the dependency document, which is where it lives.

use amasario_core::{
    API_VERSION, Confidence, ObservationBoundary, Relationship, Result, SPEC_VERSION, engine,
};
use amasario_dependency::{Dependency, EvidenceRef};
use serde::{Deserialize, Serialize};

use crate::errors::GraphFailure;
use crate::graph::Graph;
use crate::nodes::Node;

/// The graph document, in the shape `graph.schema.json` defines.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GraphDocument {
    /// The major-version compatibility family.
    #[serde(rename = "apiVersion")]
    pub api_version: String,
    /// The normative specification version that produced the document.
    #[serde(rename = "specVersion")]
    pub spec_version: String,
    /// The graph's identifier within the producing document.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// The graph's nodes.
    pub nodes: Vec<Node>,
    /// The graph's edges.
    pub edges: Vec<EdgeDocument>,
    /// The observation boundary the graph was assembled at.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub boundary: Option<ObservationBoundary>,
    /// Structural facts about the graph, recorded so a consumer does not have to
    /// recompute them and cannot silently disagree about them.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<GraphMetadata>,
}

/// One edge of a published graph.
///
/// Narrower than a [`Dependency`] by design; see the module documentation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EdgeDocument {
    /// The stable edge identifier.
    pub id: String,
    /// The identifier of the source node.
    pub source: String,
    /// The identifier of the target node.
    pub target: String,
    /// The typed relationship.
    pub relationship: Relationship,
    /// The identifiers of the evidence records establishing the edge. Never empty.
    pub evidence: Vec<String>,
    /// How strongly the evidence supports the edge.
    pub confidence: Confidence,
    /// The observation boundary of this edge, where one was recorded.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub boundary: Option<ObservationBoundary>,
    /// Whether the edge was directly observed rather than inferred.
    pub observed: bool,
}

impl EdgeDocument {
    /// Projects an edge into its published form.
    ///
    /// The boundary is taken from the edge when it has one and from the graph otherwise,
    /// because an edge established on a network is bounded by that network, and an edge
    /// that recorded none inherits the boundary of the analysis that produced it.
    fn of(dependency: &Dependency, graph_boundary: Option<&ObservationBoundary>) -> Self {
        Self {
            id: crate::edges::Edge::stable_id(
                &dependency.subject,
                dependency.relationship,
                &dependency.object,
            ),
            source: dependency.subject.to_string(),
            target: dependency.object.to_string(),
            relationship: dependency.relationship,
            evidence: citation_ids(&dependency.evidence),
            confidence: dependency.confidence.clone(),
            boundary: dependency
                .observed_at
                .clone()
                .or_else(|| graph_boundary.cloned()),
            observed: dependency.basis.is_observed(),
        }
    }

    /// The entity kinds an edge of a document is validated against, by its endpoints.
    ///
    /// Exposed because a consumer reading a document has only the endpoint identifiers,
    /// and the kind is the part of the identifier before the first colon.
    #[must_use]
    pub fn endpoint_kinds(&self) -> Option<(amasario_core::EntityKind, amasario_core::EntityKind)> {
        use std::str::FromStr as _;
        let source = self.source.split_once(':')?.0;
        let target = self.target.split_once(':')?.0;
        Some((
            amasario_core::EntityKind::from_str(source).ok()?,
            amasario_core::EntityKind::from_str(target).ok()?,
        ))
    }
}

/// Structural facts about a published graph.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GraphMetadata {
    /// Whether the graph's edges are directed. Always true.
    ///
    /// Typed as a boolean and always `true` because the schema requires the field and
    /// constrains it to `true`: Amasario relationship semantics are directional, so a
    /// graph reporting `false` is not an Amasario graph. The field exists so that a
    /// consumer can assert the property rather than infer it from the vocabulary.
    pub directed: bool,
    /// Whether the graph contains at least one cycle.
    pub cyclic: bool,
    /// How many nodes the graph has.
    #[serde(rename = "nodeCount")]
    pub node_count: usize,
    /// How many edges the graph has.
    #[serde(rename = "edgeCount")]
    pub edge_count: usize,
    /// The traversal depth the graph was bounded by, when it was bounded.
    #[serde(rename = "maxDepth", skip_serializing_if = "Option::is_none")]
    pub max_depth: Option<usize>,
    /// Whether traversal stopped before exhausting the reachable graph.
    pub truncated: bool,
    /// Identifiers of nodes with no edges.
    #[serde(rename = "disconnectedNodes")]
    pub disconnected_nodes: Vec<String>,
}

impl GraphDocument {
    /// Projects a graph into the document the specification defines.
    ///
    /// Nodes and edges are copied in the graph's canonical order, and the metadata is
    /// computed from the graph rather than taken from it: `cyclic` is derived from the
    /// edge topology, so a document cannot claim to be acyclic while containing a cycle.
    #[must_use]
    pub fn of(graph: &Graph) -> Self {
        let mut nodes = graph.nodes.clone();
        nodes.sort_by(crate::nodes::canonical_order);
        let mut edges: Vec<EdgeDocument> = graph
            .edges
            .iter()
            .map(|edge| EdgeDocument::of(edge.dependency(), graph.boundary.as_ref()))
            .collect();
        // The same rule the graph orders its own edges by, so that the document's edge
        // order is the graph's and not a second opinion about it.
        edges.sort_by(|left, right| {
            left.source
                .cmp(&right.source)
                .then_with(|| {
                    crate::edges::relationship_rank(left.relationship)
                        .cmp(&crate::edges::relationship_rank(right.relationship))
                })
                .then_with(|| left.target.cmp(&right.target))
        });

        let disconnected: Vec<String> = graph
            .disconnected_nodes()
            .into_iter()
            .map(ToOwned::to_owned)
            .collect();
        let metadata = GraphMetadata {
            directed: true,
            cyclic: crate::cycles::has_cycle(graph),
            node_count: nodes.len(),
            edge_count: edges.len(),
            max_depth: graph.max_depth,
            truncated: graph.truncated,
            disconnected_nodes: disconnected,
        };

        Self {
            api_version: API_VERSION.to_owned(),
            spec_version: SPEC_VERSION.to_owned(),
            id: (!graph.id.is_empty()).then(|| graph.id.clone()),
            nodes,
            edges,
            boundary: graph.boundary.clone(),
            metadata: Some(metadata),
        }
    }

    /// Checks that this engine may interpret the document.
    ///
    /// Delegates to the engine's own specification stamp rather than reimplementing the
    /// rule, so that a document accepted here is accepted everywhere in the engine and a
    /// document refused here is refused for a reason the rest of the engine already
    /// explains.
    ///
    /// # Errors
    ///
    /// Returns a specification compatibility error when the document's compatibility
    /// family is not this engine's, or when its specification version is newer than this
    /// engine implements.
    pub fn check_compatible(&self) -> Result<()> {
        engine::SpecificationStamp {
            api_version: self.api_version.clone(),
            spec_version: self.spec_version.clone(),
        }
        .check_compatible()
    }

    /// The document's canonical JSON form.
    ///
    /// Deterministic: the field order is the schema's, the node and edge arrays are in
    /// canonical order, and absent optional fields are omitted rather than written as
    /// `null`. Two runs over the same evidence therefore produce byte-identical output,
    /// which is what makes a hash of a document meaningful.
    ///
    /// # Errors
    ///
    /// Returns a graph error when the document cannot be serialised, which for this shape
    /// means a `f64`-only value or a map keyed by something other than a string - neither
    /// of which the type admits, so a failure here is a defect in the engine.
    pub fn canonical_json(&self) -> Result<String> {
        serde_json::to_string(self).map_err(|error| {
            GraphFailure::MalformedDocument {
                detail: format!("the graph document could not be written: {error}"),
            }
            .into_error()
        })
    }

    /// The document's canonical JSON form, indented for a person to read.
    ///
    /// # Errors
    ///
    /// As [`GraphDocument::canonical_json`].
    pub fn pretty_json(&self) -> Result<String> {
        serde_json::to_string_pretty(self).map_err(|error| {
            GraphFailure::MalformedDocument {
                detail: format!("the graph document could not be written: {error}"),
            }
            .into_error()
        })
    }

    /// Reads a document, checking its compatibility before its content.
    ///
    /// The order is the specification's: `apiVersion` is checked "before interpreting any
    /// field, including unknown fields", because a field that changed meaning between
    /// families could invert an analysis if it were read under the wrong rules.
    ///
    /// # Errors
    ///
    /// Returns a validation error when the document is not readable, and a specification
    /// compatibility error when it is readable but from an unsupported family or version.
    pub fn from_json(json: &str) -> Result<Self> {
        let envelope: VersionEnvelope = serde_json::from_str(json).map_err(|error| {
            GraphFailure::MalformedDocument {
                detail: error.to_string(),
            }
            .into_error()
        })?;
        engine::SpecificationStamp {
            api_version: envelope.api_version,
            spec_version: envelope.spec_version,
        }
        .check_compatible()?;
        serde_json::from_str(json).map_err(|error| {
            GraphFailure::MalformedDocument {
                detail: error.to_string(),
            }
            .into_error()
        })
    }

    /// The identifiers of nodes that no edge connects.
    #[must_use]
    pub fn disconnected_node_ids(&self) -> Vec<&str> {
        self.metadata
            .as_ref()
            .map(|metadata| {
                metadata
                    .disconnected_nodes
                    .iter()
                    .map(String::as_str)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Whether the document's endpoints all resolve to nodes **it** contains.
    ///
    /// The reference-integrity rule, restated over the document rather than over a graph:
    /// a consumer that has only the document can still check it, and the schema says the
    /// rule is a property of a graph rather than of the structure that produced one.
    #[must_use]
    pub fn unresolvable_endpoints(&self) -> Vec<&str> {
        let mut dangling: Vec<&str> = Vec::new();
        for edge in &self.edges {
            for endpoint in [edge.source.as_str(), edge.target.as_str()] {
                if !self.nodes.iter().any(|node| node.id == endpoint) {
                    dangling.push(endpoint);
                }
            }
        }
        dangling.sort_unstable();
        dangling.dedup();
        dangling
    }

    /// The citation identifiers an edge publishes, for a report that lists evidence.
    #[must_use]
    pub fn citations(&self) -> Vec<&str> {
        self.edges
            .iter()
            .flat_map(|edge| edge.evidence.iter().map(String::as_str))
            .collect()
    }
}

/// The two version fields, read before anything else is interpreted.
///
/// Deliberately *not* `deny_unknown_fields`: this exists to read exactly two fields out of
/// a document that has many, so that compatibility can be checked before the rest is
/// interpreted. Refusing the document's other fields here would refuse every valid document.
#[derive(Debug, Deserialize)]
struct VersionEnvelope {
    #[serde(rename = "apiVersion")]
    api_version: String,
    #[serde(rename = "specVersion")]
    spec_version: String,
}

/// Collects the citation identifiers of an edge's evidence, in record order.
///
/// The dependency layer's own ordering is preserved rather than re-sorted, because two
/// orderings for one list would make a document's bytes depend on which layer wrote it.
/// Kept separate from the projection because the report layer lists citations the same
/// way, and one projection and one report must not disagree about the order.
#[must_use]
fn citation_ids(evidence: &[EvidenceRef]) -> Vec<String> {
    evidence
        .iter()
        .map(|citation| citation.id.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use amasario_core::{
        Basis, EntityKind, EntityRef, EvidenceType, LedgerSequence, Network, NetworkType,
        TruncationReason,
    };
    use amasario_dependency::{Candidate, EvidenceRef, resolve};

    fn entity(kind: EntityKind, id: &str) -> EntityRef {
        EntityRef::new(kind, id).expect("a reference")
    }

    fn boundary() -> ObservationBoundary {
        ObservationBoundary {
            network: Network::new(
                "testnet",
                NetworkType::Testnet,
                "Test SDF Network ; September 2015",
            )
            .expect("a network"),
            ledger: LedgerSequence::new(8_000).expect("a ledger"),
            observed_at: "2026-09-15T00:00:00Z".to_owned(),
            spec_version: None,
        }
    }

    fn edge(from: &str, to: &str, transaction: &str) -> Candidate {
        Candidate::new(
            entity(EntityKind::Contract, from),
            entity(EntityKind::Contract, to),
            Relationship::Invocates,
            Basis::ObservedInvocation,
            vec![EvidenceRef::new(EvidenceType::Transaction, transaction).expect("a citation")],
        )
        .expect("a candidate")
        .observed_at(boundary())
        .with_outcome(Some(true))
    }

    fn graph_of(candidates: &[Candidate]) -> Graph {
        let subject = entity(EntityKind::Contract, "C-subject");
        let set = resolve(subject, Some(boundary()), candidates, 6).expect("resolves");
        Graph::from_dependencies("g1", &set).expect("a graph")
    }

    fn document_of(candidates: &[Candidate]) -> GraphDocument {
        GraphDocument::of(&graph_of(candidates))
    }

    #[test]
    fn a_document_carries_the_version_stamp_the_specification_defines() {
        let document = document_of(&[edge("C-subject", "C-a", &"a".repeat(64))]);
        assert_eq!(document.api_version, "amasario.dev/v1");
        assert_eq!(document.spec_version, "1.0.0");
        assert_eq!(document.id.as_deref(), Some("g1"));
        assert!(document.check_compatible().is_ok());
        assert_eq!(document.nodes.len(), 2);
        assert_eq!(document.edges.len(), 1);
    }

    #[test]
    fn a_document_uses_exactly_the_field_names_the_schema_requires() {
        let document = document_of(&[edge("C-subject", "C-a", &"a".repeat(64))]);
        let json: serde_json::Value =
            serde_json::from_str(&document.canonical_json().expect("serialises"))
                .expect("valid JSON");
        let object = json.as_object().expect("an object");

        // The schema sets `additionalProperties: false` and requires four fields, so a
        // field name that drifted would make every document invalid.
        for expected in ["apiVersion", "specVersion", "nodes", "edges"] {
            assert!(object.contains_key(expected), "missing {expected}: {json}");
        }
        for unexpected in ["api_version", "specVersionNumber"] {
            assert!(
                !object.contains_key(unexpected),
                "{unexpected} is not a schema field"
            );
        }

        let node = &json["nodes"][0];
        let node_object = node.as_object().expect("an object");
        assert!(node_object.contains_key("id"));
        assert!(node_object.contains_key("kind"));
        assert!(
            !node_object.contains_key("attributes"),
            "an absent optional field must be omitted, not written as null: {node}"
        );

        let edge = &json["edges"][0];
        let edge_object = edge.as_object().expect("an object");
        for expected in [
            "id",
            "source",
            "target",
            "relationship",
            "evidence",
            "confidence",
        ] {
            assert!(
                edge_object.contains_key(expected),
                "missing {expected}: {edge}"
            );
        }
        assert!(
            !edge_object.contains_key("basis"),
            "the graph edge is narrower than a dependency: {edge}"
        );
        assert!(
            !edge_object.contains_key("classes"),
            "classes belong to the dependency document: {edge}"
        );
        assert_eq!(edge["relationship"], "INVOCATES");
        assert_eq!(edge["source"], "CONTRACT:C-subject");
        assert_eq!(edge["observed"], true);
    }

    #[test]
    fn evidence_is_published_as_citations_rather_than_as_records() {
        // `provenance.schema.json` defines evidenceRef as a string - the identifier of an
        // evidence record - so a document that published `{id, kind}` objects would be
        // invalid, and a graph that repeated the record per edge would be a second copy
        // that could disagree with the evidence layer.
        let document = document_of(&[edge("C-subject", "C-a", &"a".repeat(64))]);
        let json: serde_json::Value =
            serde_json::from_str(&document.canonical_json().expect("serialises"))
                .expect("valid JSON");
        let evidence = json["edges"][0]["evidence"].as_array().expect("an array");
        assert_eq!(evidence.len(), 1);
        assert!(
            evidence[0].is_string(),
            "an evidence citation is a string: {evidence:?}"
        );
        assert_eq!(evidence[0], "a".repeat(64));
        assert_eq!(document.citations().len(), 1);
    }

    #[test]
    fn the_metadata_is_computed_from_the_topology_rather_than_asserted() {
        let document = document_of(&[
            edge("C-subject", "C-a", &"a".repeat(64)),
            edge("C-a", "C-b", &"b".repeat(64)),
            edge("C-b", "C-a", &"c".repeat(64)),
        ]);
        let metadata = document.metadata.as_ref().expect("metadata");
        assert!(metadata.directed, "Amasario relationships are directional");
        assert!(metadata.cyclic, "the document contains a cycle");
        assert_eq!(metadata.node_count, 3);
        assert_eq!(metadata.edge_count, 3);
        assert_eq!(metadata.max_depth, Some(6));
        assert!(!metadata.truncated);
        assert!(metadata.disconnected_nodes.is_empty());
    }

    #[test]
    fn an_acyclic_graph_is_reported_as_acyclic() {
        let document = document_of(&[
            edge("C-subject", "C-a", &"a".repeat(64)),
            edge("C-a", "C-b", &"b".repeat(64)),
        ]);
        assert!(!document.metadata.as_ref().expect("metadata").cyclic);
    }

    #[test]
    fn a_discovered_node_with_no_edges_is_named_in_the_metadata() {
        let mut graph = graph_of(&[edge("C-subject", "C-a", &"a".repeat(64))]);
        graph
            .add_entity(&entity(EntityKind::Contract, "C-unrelated"))
            .expect("a node");
        graph.canonicalise();
        let document = GraphDocument::of(&graph);
        assert_eq!(
            document.disconnected_node_ids(),
            vec!["CONTRACT:C-unrelated"]
        );
        assert!(
            document.unresolvable_endpoints().is_empty(),
            "an unrelated node is not a dangling reference"
        );
    }

    #[test]
    fn a_truncated_graph_says_so_in_its_metadata() {
        let mut graph = graph_of(&[edge("C-subject", "C-a", &"a".repeat(64))]);
        graph.truncated = true;
        graph.truncation_reason = Some(TruncationReason::MaxDepthReached);
        let document = GraphDocument::of(&graph);
        assert!(document.metadata.as_ref().expect("metadata").truncated);
    }

    #[test]
    fn the_canonical_form_is_stable_across_runs_and_insertion_orders() {
        let forwards = [
            edge("C-subject", "C-zeta", &"1".repeat(64)),
            edge("C-subject", "C-alpha", &"2".repeat(64)),
            edge("C-alpha", "C-deep", &"3".repeat(64)),
        ];
        let mut backwards = forwards.clone();
        backwards.reverse();

        let first = document_of(&forwards).canonical_json().expect("serialises");
        let second = document_of(&backwards)
            .canonical_json()
            .expect("serialises");
        assert_eq!(first, second, "one topology, one document");

        for _ in 0..8 {
            assert_eq!(
                document_of(&forwards).canonical_json().expect("serialises"),
                first
            );
        }
        // The pretty form differs only in whitespace, not in content.
        let pretty: serde_json::Value =
            serde_json::from_str(&document_of(&forwards).pretty_json().expect("serialises"))
                .expect("valid JSON");
        let compact: serde_json::Value = serde_json::from_str(&first).expect("valid JSON");
        assert_eq!(pretty, compact);
    }

    #[test]
    fn a_document_round_trips_through_its_canonical_form() {
        let document = document_of(&[
            edge("C-subject", "C-a", &"a".repeat(64)),
            edge("C-a", "C-b", &"b".repeat(64)),
        ]);
        let json = document.canonical_json().expect("serialises");
        let parsed = GraphDocument::from_json(&json).expect("reads back");
        assert_eq!(parsed, document);
        assert_eq!(
            parsed.canonical_json().expect("serialises"),
            json,
            "reading and re-writing changes nothing"
        );
    }

    #[test]
    fn reading_back_gives_what_the_document_carries_and_nothing_more() {
        // A graph's edges are dependencies and a dependency's basis cannot be recovered
        // from a document that does not state one. Reconstructing it would mean inventing
        // it, so the document deliberately cannot become a full graph.
        let document = document_of(&[edge("C-subject", "C-a", &"a".repeat(64))]);
        let parsed = GraphDocument::from_json(&document.canonical_json().expect("serialises"))
            .expect("reads back");
        assert_eq!(parsed.edges[0].source, "CONTRACT:C-subject");
        assert_eq!(
            parsed.edges[0].endpoint_kinds(),
            Some((EntityKind::Contract, EntityKind::Contract))
        );
        assert_eq!(
            parsed.edges[0].evidence,
            vec!["a".repeat(64)],
            "the citations survive, which is what a consumer needs to follow the claim"
        );
        assert!(parsed.edges[0].observed);
        assert_eq!(
            parsed.edges[0].confidence.level,
            amasario_core::ConfidenceLevel::Verified
        );
    }

    #[test]
    fn an_incompatible_family_is_refused_before_any_field_is_interpreted() {
        let document = document_of(&[edge("C-subject", "C-a", &"a".repeat(64))]);
        let mut json: serde_json::Value =
            serde_json::from_str(&document.canonical_json().expect("serialises"))
                .expect("valid JSON");
        json["apiVersion"] = serde_json::Value::String("amasario.dev/v2".to_owned());
        let error = GraphDocument::from_json(&json.to_string())
            .expect_err("a changed family could invert an analysis");
        assert_eq!(
            error.category(),
            amasario_core::ErrorCategory::SpecificationCompatibility
        );
        assert!(
            error.to_string().contains("amasario.dev/v2"),
            "got: {error}"
        );
    }

    #[test]
    fn a_newer_minor_specification_version_is_refused_rather_than_partially_read() {
        let document = document_of(&[edge("C-subject", "C-a", &"a".repeat(64))]);
        let mut json: serde_json::Value =
            serde_json::from_str(&document.canonical_json().expect("serialises"))
                .expect("valid JSON");
        json["specVersion"] = serde_json::Value::String("1.9.0".to_owned());
        let error = GraphDocument::from_json(&json.to_string())
            .expect_err("a producer of a newer minor version may rely on fields we lack");
        assert_eq!(
            error.category(),
            amasario_core::ErrorCategory::SpecificationCompatibility
        );
    }

    #[test]
    fn malformed_input_is_reported_as_a_bad_document_not_as_a_broken_graph() {
        let error = GraphDocument::from_json("{not json").expect_err("unreadable");
        assert_eq!(error.category(), amasario_core::ErrorCategory::Validation);

        // A document that is readable but missing the version stamp cannot be checked at
        // all, and reading it anyway would be reading fields under unknown rules.
        let error = GraphDocument::from_json(r#"{"nodes":[],"edges":[]}"#)
            .expect_err("the version stamp is not optional");
        assert_eq!(error.category(), amasario_core::ErrorCategory::Validation);
    }

    #[test]
    fn an_unknown_field_is_refused_rather_than_silently_dropped() {
        // `additionalProperties: false` on every object in the schema. Accepting an
        // unknown field would mean discarding a fact someone considered worth stating.
        let document = document_of(&[edge("C-subject", "C-a", &"a".repeat(64))]);
        let mut json: serde_json::Value =
            serde_json::from_str(&document.canonical_json().expect("serialises"))
                .expect("valid JSON");
        json["surprise"] = serde_json::Value::Bool(true);
        let error = GraphDocument::from_json(&json.to_string())
            .expect_err("an unknown field is not a schema field");
        assert_eq!(error.category(), amasario_core::ErrorCategory::Validation);
    }

    #[test]
    fn a_documents_endpoints_all_resolve_within_it() {
        let document = document_of(&[
            edge("C-subject", "C-a", &"a".repeat(64)),
            edge("C-a", "C-b", &"b".repeat(64)),
        ]);
        assert!(document.unresolvable_endpoints().is_empty());

        // A document assembled by hand can dangle, and a consumer holding only the
        // document can still detect it.
        let mut broken = document;
        broken.nodes.retain(|node| node.id != "CONTRACT:C-b");
        assert_eq!(broken.unresolvable_endpoints(), vec!["CONTRACT:C-b"]);
    }

    #[test]
    fn an_edge_without_a_boundary_inherits_the_graphs() {
        // A dependency that recorded no boundary of its own: the graph's boundary is what
        // bounds the analysis, and an edge inside it is bounded by it. Leaving the field
        // absent would tell a consumer the edge cannot be reproduced, which is a stronger
        // claim than the truth.
        let subject = entity(EntityKind::Contract, "C-subject");
        let set = resolve(
            subject,
            Some(boundary()),
            &[edge("C-subject", "C-a", &"a".repeat(64))],
            4,
        )
        .expect("resolves");
        let mut dependency = set.direct[0].clone();
        dependency.observed_at = None;
        let mut graph = Graph::from_dependencies_flat("g1", vec![dependency]).expect("a graph");
        assert!(graph.edges[0].dependency().observed_at.is_none());
        graph.boundary = Some(boundary());
        graph.canonicalise();

        let document = GraphDocument::of(&graph);
        assert_eq!(
            document.edges[0]
                .boundary
                .as_ref()
                .map(|boundary| boundary.network.id.as_str()),
            Some("testnet"),
            "the edge is bounded by the analysis that produced it"
        );
    }

    #[test]
    fn the_serialization_helper_agrees_with_the_documents_own_projection() {
        // `citation_ids` and the edge projection must not be two orderings for one list.
        let evidence = vec![
            EvidenceRef::new(EvidenceType::Transaction, "a".repeat(64)).expect("a citation"),
            EvidenceRef::new(EvidenceType::Event, "e1").expect("a citation"),
        ];
        assert_eq!(
            citation_ids(&evidence),
            vec!["a".repeat(64), "e1".to_owned()]
        );
    }
}
