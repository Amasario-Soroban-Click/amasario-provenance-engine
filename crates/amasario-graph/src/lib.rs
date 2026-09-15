//! Typed dependency and provenance graphs: integrity, cycles, traversal and paths.
//!
//! # What this crate is for
//!
//! A resolved dependency set answers "what does this contract relate to". A graph answers
//! the questions that need the relationships *together*: what an entity reaches, what
//! reaches it, which entities are mutually dependent, and what can be published as the
//! artifact a consumer reconstructs provenance from. Those are different questions, and
//! this crate is where they are answered without re-deriving the relationships or
//! re-deciding what any of them mean.
//!
//! It sits above [`amasario_dependency`] and above [`amasario_core`], and it decides
//! nothing about what a dependency *is*. Every edge carries the dependency record it came
//! from, so a graph cannot hold a second opinion about a basis, a class or a verification
//! status.
//!
//! # The distinctions this crate exists to preserve
//!
//! **A reachability claim is not an edge.** [`Graph::derived`] holds transitive
//! dependencies and [`Graph::edges`] holds the relationships that were established. An
//! edge from `A` to a two-hop target would tell a consumer `A` relates to it directly,
//! which the analysis never found.
//!
//! **A dangling endpoint is a failure, not a warning.** `graph.schema.json` requires an
//! edge's endpoints to resolve to nodes in the same graph. A graph that looks complete
//! while containing an unresolvable relationship is worse than no graph, because a
//! consumer cannot tell. [`Graph::validate`] refuses it.
//!
//! **A cycle is reported, not removed.** [`cycles::find`] returns one concrete cycle per
//! strongly connected component. Removing a cycle would make the graph tidy and every
//! downstream statement about it false.
//!
//! **A refusal travels with the graph.** [`Graph::unestablished`] holds the candidates
//! that could not become dependencies. Dropping them would let a report describe an
//! analysis as complete when something was observed and refused.
//!
//! # What this crate does not claim
//!
//! Amasario is provenance, dependency and impact infrastructure. It is not a security
//! scanner and no term here means "secure", "safe", "malicious" or "vulnerable". A
//! verified edge says the evidence supporting it was complete, not that either endpoint is
//! trustworthy.

#![deny(missing_docs)]
#![deny(unsafe_code)]

pub mod cycles;
pub mod edges;
pub mod errors;
pub mod graph;
pub mod nodes;

// Re-exported together because they are used together: a caller building a graph also
// validates it and asks about its topology, and having to know which module each name
// lives in would be friction with no benefit.
pub use cycles::{components, cyclic_nodes, find as find_cycles, has_cycle};
pub use edges::{Edge, relationship_rank, render as render_edge};
pub use errors::{GraphFailure, describe as describe_failure, first_failure};
pub use graph::Graph;
pub use nodes::{Node, NodeAttributes};

#[cfg(test)]
mod tests {
    use super::*;
    use amasario_core::{
        Basis, EntityKind, EntityRef, EvidenceType, LedgerSequence, Network, NetworkType,
        ObservationBoundary, Relationship,
    };
    use amasario_dependency::{Candidate, EvidenceRef, resolve};

    fn candidate(from: &str, to: &str) -> Candidate {
        Candidate::new(
            EntityRef::new(EntityKind::Contract, from).expect("a reference"),
            EntityRef::new(EntityKind::Contract, to).expect("a reference"),
            Relationship::Invocates,
            Basis::ObservedInvocation,
            vec![EvidenceRef::new(EvidenceType::Transaction, "a".repeat(64)).expect("a citation")],
        )
        .expect("a candidate")
        .observed_at(ObservationBoundary {
            network: Network::new(
                "testnet",
                NetworkType::Testnet,
                "Test SDF Network ; September 2015",
            )
            .expect("a network"),
            ledger: LedgerSequence::new(9_000).expect("a ledger"),
            observed_at: "2026-09-15T00:00:00Z".to_owned(),
            spec_version: None,
        })
        .with_outcome(Some(true))
    }

    #[test]
    fn the_vocabulary_is_reachable_from_the_crate_root() {
        // A consumer should not have to know which module a concept lives in to name it. A
        // re-export that went missing would otherwise be a breaking change found downstream
        // rather than here.
        let subject = EntityRef::new(EntityKind::Contract, "C-subject").expect("a reference");
        let set = resolve(subject, None, &[candidate("C-subject", "C-a")], 4).expect("resolves");
        let graph = Graph::from_dependencies("g1", &set).expect("a graph");
        assert_eq!(graph.edge_count(), 1);
        assert_eq!(graph.node_count(), 2);
        assert!(!has_cycle(&graph));
        assert!(find_cycles(&graph).is_empty());
        assert!(components(&graph).is_empty());
        assert!(cyclic_nodes(&graph).is_empty());
        assert_eq!(
            relationship_rank(Relationship::DependsOn),
            0,
            "the taxonomy's order, not the alphabet's"
        );
        assert!(graph.validate().is_ok());
        assert!(NodeAttributes::default().is_empty());
        assert!(
            !GraphFailure::MissingGraphId
                .clone()
                .into_error()
                .to_string()
                .is_empty()
        );
        first_failure(Vec::new()).expect("nothing to report");
        assert_eq!(
            render_edge(graph.edges[0].dependency()),
            "CONTRACT:C-subject -INVOCATES-> CONTRACT:C-a"
        );
    }
}
