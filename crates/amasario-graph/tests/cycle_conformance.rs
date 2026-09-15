//! The graph layer's rendering and cycle detection, checked against the dependency layer's.
//!
//! # Why this test exists
//!
//! Two crates in this workspace render an edge as a string and report a cycle as a list of
//! entities and edges. The dependency layer is below and owns the classes; the graph layer
//! is above and owns the topology. Neither can import the other's private helper, so the
//! format string is written twice - and a duplicated format string is exactly the kind of
//! thing that drifts silently while both sides' own tests keep passing.
//!
//! This test is what makes the duplication safe. It builds one cycle, asks both layers about
//! it, and compares. If the two ever disagree, a report that merges dependency results with
//! graph results would list one cycle twice in two spellings, or attribute an edge to the
//! wrong relationship.

use amasario_core::{
    Basis, EntityKind, EntityRef, EvidenceType, LedgerSequence, Network, NetworkType,
    ObservationBoundary, Relationship,
};
use amasario_dependency::{Candidate, EvidenceRef, Limits, close, resolve};
use amasario_graph::{Graph, find_cycles, has_cycle, render_edge};

fn boundary() -> ObservationBoundary {
    ObservationBoundary {
        network: Network::new(
            "testnet",
            NetworkType::Testnet,
            "Test SDF Network ; September 2015",
        )
        .expect("a network"),
        ledger: LedgerSequence::new(12_345).expect("a ledger"),
        observed_at: "2026-09-15T00:00:00Z".to_owned(),
        spec_version: None,
    }
}

fn contract(id: &str) -> EntityRef {
    EntityRef::new(EntityKind::Contract, id).expect("a reference")
}

fn invocation(from: &str, to: &str, transaction: &str) -> Candidate {
    Candidate::new(
        contract(from),
        contract(to),
        Relationship::Invocates,
        Basis::ObservedInvocation,
        vec![EvidenceRef::new(EvidenceType::Transaction, transaction).expect("a citation")],
    )
    .expect("a candidate")
    .observed_at(boundary())
    .with_outcome(Some(true))
}

/// A three-node cycle reachable from the subject, and the graph built from it.
fn cycle_fixture() -> (Graph, Vec<Candidate>) {
    let candidates = vec![
        invocation("C-subject", "C-a", &"a".repeat(64)),
        invocation("C-a", "C-b", &"b".repeat(64)),
        invocation("C-b", "C-c", &"c".repeat(64)),
        invocation("C-c", "C-a", &"d".repeat(64)),
    ];
    let set = resolve(contract("C-subject"), Some(boundary()), &candidates, 8).expect("resolves");
    let graph = Graph::from_dependencies("g-cycle", &set).expect("a graph");
    (graph, candidates)
}

#[test]
fn both_layers_render_an_edge_the_same_way() {
    let (graph, _) = cycle_fixture();
    let subject = contract("C-subject");
    let closure = close(&subject, &graph, Limits::defaults());

    // The dependency layer renders the closing edge of its cycle; the graph layer renders
    // the same edge from its own edge set. They must be the same string.
    assert!(
        closure.has_cycles(),
        "the fixture is built to contain a cycle"
    );
    for rendered in &closure.cycles[0].edges {
        assert!(
            graph
                .edges
                .iter()
                .any(|edge| &render_edge(edge.dependency()) == rendered),
            "the graph does not render {rendered:?} identically: {:?}",
            graph
                .edges
                .iter()
                .map(|edge| render_edge(edge.dependency()))
                .collect::<Vec<_>>()
        );
    }
}

#[test]
fn the_graph_reports_the_same_edges_the_dependency_layer_traversed() {
    let (graph, _) = cycle_fixture();
    let subject = contract("C-subject");
    let closure = close(&subject, &graph, Limits::defaults());
    let from_graph = find_cycles(&graph);
    assert_eq!(
        from_graph.len(),
        1,
        "one component, one representative cycle"
    );
    for rendered in &from_graph[0].edges {
        assert!(
            closure.cycles[0].edges.contains(rendered),
            "{rendered:?} is reported by the graph but not by the closure: {:?}",
            closure.cycles[0].edges
        );
    }
    // The closure reports the loop *as reached from the subject*, so its edge list is the
    // approach followed by the loop. The graph reports the component, which is the loop
    // alone - hence the strict inequality, and hence the subset check above rather than an
    // equality check. On the loop itself the two agree edge for edge.
    assert_eq!(
        from_graph[0].edges.len(),
        3,
        "three entities, three loop edges"
    );
    assert_eq!(
        closure.cycles[0].edges.len(),
        from_graph[0].edges.len() + 1,
        "the closure's list carries exactly one approach edge: {:?}",
        closure.cycles[0].edges
    );
}

#[test]
fn the_graph_layer_sees_a_cycle_the_closure_does_not_because_it_reads_the_whole_graph() {
    // The closure starts at one subject, so a cycle it cannot reach is invisible to it.
    // The graph layer answers the question about the graph, not about a traversal of it.
    let mut candidates = cycle_fixture().1;
    candidates.push(invocation("C-far", "C-far2", &"e".repeat(64)));
    candidates.push(invocation("C-far2", "C-far", &"f".repeat(64)));
    let set = resolve(contract("C-subject"), Some(boundary()), &candidates, 8).expect("resolves");
    let graph = Graph::from_dependencies("g-cycle", &set).expect("a graph");

    let closure = close(&contract("C-subject"), &graph, Limits::defaults());
    assert_eq!(
        closure.cycles.len(),
        1,
        "the subject cannot reach the second loop"
    );
    assert_eq!(
        find_cycles(&graph).len(),
        2,
        "the graph contains both loops, and a consumer asking about the graph must see both"
    );
    assert!(has_cycle(&graph));
}

#[test]
fn a_reachability_claim_is_never_mistaken_for_an_edge_by_either_layer() {
    // `dependency/transitive-dependency` requires a transitive dependency to carry its
    // intermediates, and the graph keeps such a claim out of its edge set. If the graph
    // turned one into an edge, the closure over the graph would report a shortcut from the
    // subject to a depth-two target, which is the confusion the rule exists to prevent.
    let candidates = vec![
        invocation("C-subject", "C-a", &"a".repeat(64)),
        invocation("C-a", "C-deep", &"b".repeat(64)),
    ];
    let mut set =
        resolve(contract("C-subject"), Some(boundary()), &candidates, 8).expect("resolves");
    let graph = Graph::from_dependencies("g-chain", &set).expect("a graph");
    amasario_dependency::close_set(&mut set, &graph, Limits::defaults()).expect("closes");

    assert!(
        set.transitive
            .iter()
            .any(|dependency| dependency.object == contract("C-deep")),
        "the dependency layer reaches the deep entity transitively"
    );
    // The graph does contain an edge into `C-deep` - `C-a` invokes it, and that was
    // observed. What it must not contain is an edge *from the subject*, which would tell a
    // consumer the subject relates to the deep entity directly.
    assert!(
        graph
            .edges_between(&contract("C-subject"), &contract("C-deep"))
            .is_empty(),
        "the graph does not invent a shortcut from the subject"
    );

    let graph = Graph::from_dependencies("g-chain", &set).expect("a graph");
    let deep = graph
        .derived
        .iter()
        .find(|claim| claim.object == contract("C-deep"))
        .expect("the reachability claim travels with the graph");
    assert_eq!(deep.path, vec![contract("C-a")]);
    assert_eq!(deep.depth, 2);
}
