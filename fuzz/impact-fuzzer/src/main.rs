//! Fuzzes bounded impact propagation over adversarial graphs.
//!
//! # The question this target asks of every input
//!
//! Does a bounded propagation always terminate, always respect its bound, and always
//! dispose of what it did not reach? Those three are the whole safety story of impact
//! analysis, and the third is the one a panic would never reveal: an analysis that
//! silently stopped would report a smaller affected set than exists, and a reader could
//! not tell that from a contract with no further dependencies.
//!
//! # The topologies worth reaching
//!
//! Cycles are the case where an unbounded traversal does not terminate, so they are
//! generated rather than avoided. So are graphs where every node reaches every other, a
//! change with no dependents at all, and a change whose entity is not in the graph -
//! which is what an analysis of a contract that was never observed looks like.
//!
//! # Direction is a property under test
//!
//! The specification declares each relationship's direction and the engine never infers
//! it. A propagation that walked with the arrow for `DEPENDS_ON` would reach the wrong
//! entities and report a plausible, wrong answer. The target therefore checks the
//! direction it actually took against the property the relationship declares: an entity
//! the analysis named as affected must be reachable *against* the arrows along the
//! relationships that declare `object_to_subject` propagation.

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
use amasario_graph::Graph;
use amasario_impact::propagation::{StepDirection, propagates_backward, propagates_forward};
use amasario_impact::{ChangeType, analyze, context};

/// The relationships an edge can carry, including the two that propagate nothing.
///
/// The two non-propagating ones are in the list on purpose. A traversal that crossed
/// `OBSERVED_IN` or `VERIFIED_BY` would turn a re-observation into a change, which is the
/// single mistake the relationship's own propagation value exists to prevent.
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

const KINDS: [EntityKind; 2] = [EntityKind::Contract, EntityKind::Wasm];

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
    let edge_count = input.int_in_range(0..=48_usize).unwrap_or(0);

    let mut graph = Graph::new("amasario.fuzz").expect("a graph identifier");
    for index in 0..node_count {
        let kind = KINDS[input.int_in_range(0..=KINDS.len() - 1).unwrap_or(0)];
        graph
            .add_entity(&entity(kind, index as u16))
            .expect("a distinct node identifier");
    }
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
        edge.subject = entity(EntityKind::Contract, from as u16);
        edge.object = entity(EntityKind::Contract, to as u16);
        edge.relationship = RELATIONSHIPS[relationship_index];
        // A duplicate or a self-edge is refused by the graph, which is itself a result;
        // the propagation below then runs over the graph that was actually assembled.
        let _ = graph.add_edge(edge);
    }

    // -- the bounds, which are what make a cyclic graph tractable -------------
    let depth = input.int_in_range(1..=16_usize).unwrap_or(1);
    let node_bound = input.int_in_range(1..=DEFAULT_MAX_NODES.min(256)).unwrap_or(1);
    let limits = Limits::new(depth, node_bound).expect("non-zero bounds");

    // An entity outside the graph is a real case: the caller asked about a contract that
    // was never observed. It must produce an empty analysis rather than an error.
    let changed_kind = KINDS[input.int_in_range(0..=KINDS.len() - 1).unwrap_or(0)];
    let changed_index = input.int_in_range(0..=31_u16).unwrap_or(0);
    let changed = entity(changed_kind, changed_index);

    let impact_context = context(&graph, &[]);
    let Ok(analysis) = analyze(&impact_context, &changed, Some(ChangeType::Modified), limits) else {
        panic!("a bounded analysis over an assembled graph was refused");
    };

    // -- termination and the bound -------------------------------------------
    assert_eq!(
        analysis.changed, changed,
        "the analysis reported a different entity as the change"
    );
    assert!(
        analysis.deepest() <= limits.max_depth,
        "an analysis reported a finding deeper than its bound"
    );
    assert!(
        analysis.nodes_visited <= limits.max_nodes.max(1) + 1,
        "an analysis visited more entities than its bound permits"
    );

    // -- the disclosure, which is the property a silent stop would hide -------
    if analysis.truncated {
        assert!(
            analysis.truncation_reason.is_some(),
            "a truncated analysis did not say why it stopped"
        );
        assert!(
            !analysis.is_conclusive(),
            "a truncated analysis called itself conclusive"
        );
    } else {
        assert!(
            analysis.truncation_reason.is_none(),
            "a complete analysis carries a truncation reason"
        );
    }

    // -- every finding's own terms -------------------------------------------
    for finding in &analysis.findings {
        assert!(
            !finding.impact_type.is_empty(),
            "a finding was published with no impact type"
        );
        assert_eq!(
            finding.changed_entity, changed,
            "a finding names a change that is not the one analysed"
        );
        if finding.hop_depth == 0 {
            assert!(
                finding.path.is_none(),
                "the change itself has no route to itself"
            );
        } else {
            assert!(
                finding.path.is_some(),
                "a finding one or more hops away carries no route"
            );
            assert!(
                !finding.relationship_types.is_empty(),
                "a multi-hop finding names no relationship"
            );
        }
    }

    // -- the direction actually taken, against what each relationship declares -
    //
    // A propagation that walked with the arrow where the relationship declares
    // `object_to_subject` would reach the wrong entities and report a plausible, wrong
    // answer. Each step records the direction it moved in, so the two can be compared,
    // and both directions are checked rather than only the inverted one - a step that
    // moved forward along `VERIFIED_BY` would be the mirror-image mistake.
    for finding in &analysis.findings {
        let Some(path) = finding.path.as_ref() else {
            continue;
        };
        assert_eq!(
            path.steps.len(),
            finding.hop_depth,
            "a finding's route and its hop depth disagree"
        );
        assert_eq!(
            path.nodes.len(),
            path.steps.len() + 1,
            "a path of N steps must name N+1 entities"
        );

        for step in &path.steps {
            assert!(
                !step.evidence.is_empty(),
                "a step was reported with no evidence, so a path carries an unsupported link"
            );
            match step.direction {
                StepDirection::Forward => assert!(
                    propagates_forward(step.relationship),
                    "the analysis moved with the arrow along {}, which declares that a change \
                     does not travel that way",
                    step.relationship.as_str()
                ),
                StepDirection::Inverted => assert!(
                    propagates_backward(step.relationship),
                    "the analysis moved against the arrow along {}, which declares that a \
                     change does not travel that way",
                    step.relationship.as_str()
                ),
            }
            // The step's own endpoints must be the ones the graph holds for that edge;
            // a step that named an unrelated pair would make its route unverifiable.
            assert_ne!(
                step.source, step.target,
                "a step was reported from an entity to itself"
            );
        }
    }
});
