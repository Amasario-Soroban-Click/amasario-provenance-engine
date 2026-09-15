//! The graph layer, checked against the documents it publishes.
//!
//! # Why the published document and not the in-memory graph
//!
//! `GraphDocument` is deliberately narrower than [`amasario_graph::Graph`]: it carries a
//! node, an edge document with the relationship and the citations, the boundary and the
//! metadata, and it does not carry the dependency layer's full class list or reason.
//! That narrowing is the point - a published graph must not be able to hold a second
//! opinion about why an edge exists - and it means the document is the artefact a
//! consumer actually reads.
//!
//! So this suite reads the committed documents with `GraphDocument::from_json`, which is
//! the loader a consumer would use, and asserts the properties that make a graph usable:
//! every endpoint resolves, a cycle is reported rather than collapsed, a truncated
//! traversal says so, and a disconnected node is visible.

use amasario_graph::GraphDocument;
use amasario_integration_tests::corpus::{Corpus, assert_committed, parse_committed};
use amasario_integration_tests::documents::{self, GraphKind};

/// Every graph fixture is committed, and what is committed is what the model produces.
#[test]
fn every_graph_fixture_matches_its_builder() {
    for kind in GraphKind::all() {
        let file = format!("{}.json", kind.file());
        let built = documents::graph_document(*kind);
        assert_committed("graphs", &file, &documents::rendered(&built));
    }
}

/// Every fixture loads through the loader a consumer would use.
#[test]
fn every_graph_fixture_loads_through_its_own_loader() {
    for kind in GraphKind::all() {
        let file = format!("{}.json", kind.file());
        let text = Corpus::read("graphs", &file);
        let document = GraphDocument::from_json(&text)
            .unwrap_or_else(|error| panic!("{file} did not load: {error}"));
        document.check_compatible().unwrap_or_else(|error| {
            panic!("{file} is not a version this engine can read: {error}")
        });
    }
}

/// No edge names a node the document does not define.
///
/// A dangling endpoint is the one defect that makes a graph unrenderable and
/// untraversable at once, which is why the loader refuses it rather than the renderer
/// discovering it later.
#[test]
fn every_endpoint_resolves_to_a_node_in_the_same_document() {
    for kind in GraphKind::all() {
        let file = format!("{}.json", kind.file());
        let document: GraphDocument = parse_committed("graphs", &file);
        let dangling = document.unresolvable_endpoints();
        assert!(
            dangling.is_empty(),
            "{file}: dangling endpoints {dangling:?}"
        );
    }
}

/// The metadata's counts agree with the document's contents.
///
/// The counts are what a renderer allocates from and what a reader trusts before parsing
/// the arrays. A mismatch would be a document that describes a different graph than it
/// carries.
#[test]
fn the_recorded_counts_match_the_document() {
    for kind in GraphKind::all() {
        let file = format!("{}.json", kind.file());
        let document: GraphDocument = parse_committed("graphs", &file);
        let metadata = document
            .metadata
            .as_ref()
            .unwrap_or_else(|| panic!("{file} carries no metadata"));
        assert_eq!(metadata.node_count, document.nodes.len(), "{file}");
        assert_eq!(metadata.edge_count, document.edges.len(), "{file}");
        assert_eq!(
            metadata.disconnected_nodes.len(),
            document.disconnected_node_ids().len(),
            "{file}"
        );
    }
}

/// The cyclic fixture reports a cycle, and the others do not.
#[test]
fn only_the_cyclic_fixture_is_cyclic() {
    for kind in GraphKind::all() {
        let file = format!("{}.json", kind.file());
        let document: GraphDocument = parse_committed("graphs", &file);
        let cyclic = document
            .metadata
            .as_ref()
            .is_some_and(|metadata| metadata.cyclic);
        assert_eq!(
            cyclic,
            *kind == GraphKind::Cyclic,
            "{file}: cyclic={cyclic} for {kind:?}"
        );
    }

    // And the cycle is a real one: `bravo` requires `charlie` and `charlie` requires
    // `bravo`, which is what mutual recursion between two contracts looks like.
    let cyclic = documents::graph(GraphKind::Cyclic);
    let cycles = amasario_graph::find_cycles(&cyclic);
    assert!(
        !cycles.is_empty(),
        "the cyclic fixture must contain a cycle the traversal can find"
    );
}

/// A graph with no cycle is a graph every node of which is reachable in one direction.
#[test]
fn the_acyclic_fixtures_contain_no_cycle() {
    for kind in [
        GraphKind::Direct,
        GraphKind::Transitive,
        GraphKind::MultiHop,
    ] {
        let graph = documents::graph(kind);
        assert!(
            !amasario_graph::has_cycle(&graph),
            "{kind:?} must not contain a cycle"
        );
    }
}

/// The disconnected fixture holds a node no edge reaches.
///
/// Worth a fixture of its own because a graph whose every node lies on some path is
/// exactly the graph for which the traversal's disconnected case is untested.
#[test]
fn the_disconnected_fixture_holds_a_node_with_no_edges() {
    let document = documents::graph_document(GraphKind::Disconnected);
    let disconnected = document.disconnected_node_ids();
    assert!(
        !disconnected.is_empty(),
        "the fixture exists to hold a disconnected node"
    );
    let metadata = document.metadata.as_ref().expect("metadata");
    for id in &disconnected {
        assert!(
            metadata.disconnected_nodes.contains(&(*id).to_owned()),
            "the metadata must disclose {id}"
        );
    }
}

/// Every edge carries a relationship, a confidence and at least one citation.
#[test]
fn no_edge_is_published_without_a_citation() {
    for kind in GraphKind::all() {
        let file = format!("{}.json", kind.file());
        let document: GraphDocument = parse_committed("graphs", &file);
        for edge in &document.edges {
            assert!(
                !edge.evidence.is_empty(),
                "{file}: edge {} cites no evidence",
                edge.id
            );
            assert!(
                !edge.confidence.evidence.is_empty(),
                "{file}: edge {} has a confidence with no citation",
                edge.id
            );
        }
    }
}

/// An edge's identifier is the same in a graph document as in a dependency document.
///
/// The two layers must agree, or an edge could not be matched across them and every
/// diff would report every edge as replaced. The graph suite asserts it because the
/// agreement is a property of the *documents*, and each document is written by a
/// different crate.
#[test]
fn an_edge_keeps_one_identifier_across_the_two_document_kinds() {
    let graph = documents::graph_document(GraphKind::Direct);
    let set = documents::set_document(documents::SetKind::Direct);

    for edge in &graph.edges {
        // The two documents name an endpoint differently on purpose: a graph document
        // carries a node identifier string - `KIND:id`, which is what a node's own `id`
        // is - while a dependency document carries a structured reference. The
        // comparison therefore goes through the edge identifier, which is the thing the
        // two layers must agree on, and the endpoint strings are checked against the
        // reference the dependency document holds.
        let matching = set
            .edges
            .iter()
            .find(|candidate| candidate.id == edge.id)
            .unwrap_or_else(|| {
                panic!(
                    "the graph holds an edge {} ({} -> {}) the dependency set does not",
                    edge.id, edge.source, edge.target
                )
            });
        assert_eq!(
            edge.source,
            format!("{}:{}", matching.source.kind.as_str(), matching.source.id),
            "the graph names the source differently than the dependency document"
        );
        assert_eq!(
            edge.target,
            format!("{}:{}", matching.target.kind.as_str(), matching.target.id),
            "the graph names the target differently than the dependency document"
        );
        assert_eq!(edge.relationship, matching.relationship);
    }
}

/// Building a fixture twice produces the same bytes.
#[test]
fn building_a_graph_twice_produces_the_same_bytes() {
    for kind in GraphKind::all() {
        assert_eq!(
            documents::rendered(&documents::graph_document(*kind)),
            documents::rendered(&documents::graph_document(*kind)),
            "{kind:?} is not deterministic"
        );
    }
}

/// The corpus directory holds exactly the fixture set the builder defines.
#[test]
fn the_corpus_holds_no_fixture_the_builder_does_not_define() {
    let mut expected: Vec<String> = GraphKind::all()
        .iter()
        .map(|kind| format!("{}.json", kind.file()))
        .collect();
    expected.sort();
    let actual: Vec<String> = Corpus::files_in("graphs")
        .into_iter()
        .map(|path| {
            path.file_name()
                .expect("a file has a name")
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    assert_eq!(actual, expected);
}
