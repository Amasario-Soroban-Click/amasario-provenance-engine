//! Fuzzes graph assembly, traversal, path search and cycle detection.
//!
//! # The topologies a fixture cannot hold
//!
//! The corpus holds five graphs, each chosen to demonstrate one thing: a chain, a longer
//! chain, a cycle, a disconnected node, two edges from one subject. None of them holds a
//! self-loop, a duplicate edge, a thousand-edge fan or a graph where every node reaches
//! every other. This target builds graphs from an unstructured byte stream, so those
//! shapes are reached by construction rather than by a maintainer thinking of them.
//!
//! # What the assertions are
//!
//! A traversal that visited a node the bound forbade, or reported a depth beyond its
//! bound, would be a correctness bug that no panic would reveal. So the boundary
//! conditions of every traversal are asserted on every input:
//!
//! * a walk never reports an entity deeper than the bound it was given;
//! * a walk that stopped early says so, and one that did not, does not;
//! * a path search reports at most the number of paths it was capped at;
//! * cycle detection agrees with itself - `has_cycle` is true exactly when `find_cycles`
//!   names at least one cycle, which is two implementations of one question and the pair
//!   most likely to drift.
//!
//! # Why the assembly is fussed over
//!
//! Every edge is added through the graph's own `add_edge`, which refuses a duplicate
//! identifier, a transitive entry and a dangling endpoint. The target therefore asserts
//! the refusal contract: an edge the graph accepted is found in the graph, and an edge it
//! refused changed nothing. A graph that half-accepted an edge would produce a traversal
//! that disagrees with its own node and edge counts.

#![no_main]

use arbitrary::Unstructured;
use libfuzzer_sys::fuzz_target;

use amasario_core::{
    Basis, EntityKind, EntityRef, EvidenceType, Network, NetworkType, ObservationBoundary,
    Relationship,
};
use amasario_dependency::classifier::{Candidate, EvidenceRef};
use amasario_dependency::transitive::{DEFAULT_MAX_NODES, Limits};
use amasario_dependency::{Dependency, resolve};
use amasario_graph::{Graph, all_paths_bounded, find_cycles, has_cycle, walk, walk_reverse};

/// The relationships an edge can be asserted with, including the one that carries no change.
const RELATIONSHIPS: [Relationship; 8] = [
    Relationship::DependsOn,
    Relationship::Invocates,
    Relationship::BuiltFrom,
    Relationship::DerivedFrom,
    Relationship::DeployedAs,
    Relationship::ObservedIn,
    Relationship::VerifiedBy,
    Relationship::Affects,
];

/// The entity kinds a node can be.
const KINDS: [EntityKind; 3] = [EntityKind::Contract, EntityKind::Wasm, EntityKind::Source];

fn entity(kind: EntityKind, index: u16) -> EntityRef {
    EntityRef::new(kind, format!("Cfuzz{index}")).expect("a non-empty identifier")
}

fn boundary() -> ObservationBoundary {
    let network = Network::new(
        "fuzz",
        NetworkType::Testnet,
        "Test SDF Network ; September 2015",
    )
    .expect("the testnet passphrase is not empty");
    ObservationBoundary::new(
        network,
        amasario_core::LedgerSequence::new(1_000).expect("a non-zero ledger"),
        "2026-01-01T00:00:00Z",
    )
}

/// One real, classified edge, used as the shape every fuzzed edge is copied from.
///
/// Built through `resolve` rather than assembled by hand so that the template is an edge
/// the engine actually produces - a hand-built one could carry a confidence the
/// classifier would never assign, and the graph would then be fuzzed over a state the
/// engine cannot reach.
fn template() -> Dependency {
    let subject = entity(EntityKind::Contract, 0);
    let object = entity(EntityKind::Contract, 1);
    let candidate = Candidate::new(
        subject.clone(),
        object,
        Relationship::Invocates,
        Basis::ObservedInvocation,
        vec![EvidenceRef::new(EvidenceType::Transaction, "e-fuzz").expect("a citation")],
    )
    .expect("the template candidate is permitted")
    .observed_at(boundary())
    .with_outcome(Some(true));
    let set = resolve(subject, Some(boundary()), &[candidate], 1).expect("a resolvable set");
    set.direct.into_iter().next().expect("one direct edge")
}

fuzz_target!(|data: &[u8]| {
    let mut input = Unstructured::new(data);
    let shape = template();

    let node_count = input.int_in_range(1..=24_usize).unwrap_or(1);
    let edge_count = input.int_in_range(0..=64_usize).unwrap_or(0);

    let mut graph = Graph::new("amasario.fuzz").expect("a graph identifier");
    for index in 0..node_count {
        let kind = KINDS[input.int_in_range(0..=KINDS.len() - 1).unwrap_or(0)];
        graph
            .add_entity(&entity(kind, index as u16))
            .expect("a distinct node identifier");
    }
    let nodes_before = graph.node_count();

    let mut accepted = 0_usize;
    for _ in 0..edge_count {
        let Ok(from) = input.int_in_range(0..=node_count - 1) else {
            break;
        };
        let Ok(to) = input.int_in_range(0..=node_count - 1) else {
            break;
        };
        let Ok(relationship_index) = input.int_in_range(0..=RELATIONSHIPS.len() - 1) else {
            break;
        };

        let mut edge = shape.clone();
        edge.subject = entity(KINDS[0], from as u16);
        edge.object = entity(KINDS[0], to as u16);
        edge.relationship = RELATIONSHIPS[relationship_index];

        match graph.add_edge(edge.clone()) {
            Ok(()) => {
                accepted += 1;
                // Nothing accepted may be lost, and nothing may be gained: an assembly
                // that dropped an edge would make every traversal disagree with the node
                // and edge counts a report publishes.
                assert_eq!(
                    graph.edge_count(),
                    accepted,
                    "an accepted edge did not appear in the graph"
                );
                assert!(
                    graph
                        .edges
                        .iter()
                        .any(|held| held.source() == &edge.subject && held.target() == &edge.object),
                    "an accepted edge is not among the graph's edges"
                );
            }
            Err(_) => {
                // A refusal must leave the graph exactly as it was.
                assert_eq!(
                    graph.edge_count(),
                    accepted,
                    "a refused edge still changed the graph"
                );
            }
        }
    }

    assert!(
        graph.node_count() >= nodes_before,
        "assembly removed a node"
    );

    // -- traversal, with bounds derived from the input ----------------------
    let depth = input.int_in_range(1..=8_usize).unwrap_or(1);
    let node_bound = input.int_in_range(1..=DEFAULT_MAX_NODES).unwrap_or(1);
    let limits = Limits::new(depth, node_bound).expect("non-zero bounds");
    let start = entity(KINDS[0], input.int_in_range(0..=23_u16).unwrap_or(0));

    let forward = walk(&graph, &start, limits);
    assert!(
        forward.deepest <= limits.max_depth,
        "a walk reported an entity deeper than its bound"
    );
    if forward.is_inconclusive() {
        assert!(
            forward.truncation_reason.is_some(),
            "an inconclusive walk did not say why it stopped"
        );
    } else {
        assert!(
            forward.truncation_reason.is_none(),
            "a conclusive walk cannot carry a truncation reason"
        );
    }

    let reverse = walk_reverse(&graph, &start, limits);
    assert!(
        reverse.deepest <= limits.max_depth,
        "a reverse walk reported an entity deeper than its bound"
    );

    // -- path search, with a cap derived from the input ---------------------
    let destination = entity(KINDS[0], input.int_in_range(0..=23_u16).unwrap_or(0));
    let cap = input.int_in_range(0..=16_usize).unwrap_or(0);
    let search = all_paths_bounded(&graph, &start, &destination, limits, cap);
    assert!(
        search.paths.len() <= cap,
        "a bounded path search reported more paths than its cap"
    );
    if search.truncated {
        assert!(
            search.truncation_reason.is_some(),
            "a truncated path search did not say why it stopped"
        );
    }

    // -- cycles, the pair of implementations most likely to drift -----------
    let found = find_cycles(&graph);
    assert_eq!(
        has_cycle(&graph),
        !found.is_empty(),
        "cycle detection disagrees with itself: has_cycle says {} and find_cycles found {}",
        has_cycle(&graph),
        found.len()
    );

    // A cycle must name at least two entities: the graph refuses a self-dependency, so a
    // one-entity cycle would mean an edge from a node to itself had been admitted.
    for cycle in &found {
        assert!(
            cycle.entities.len() >= 2,
            "a cycle was reported over fewer than two entities, so a self-edge was admitted"
        );
    }
});
