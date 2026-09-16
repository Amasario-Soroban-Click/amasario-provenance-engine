//! Fuzzes classification, resolution and the closure over adversarial candidate sets.
//!
//! # What this explores that a unit test cannot
//!
//! The dependency layer is where the specification's rules are enforced, and each rule
//! is a refusal: no self-dependency, no candidate without evidence, no relationship whose
//! endpoints it does not permit, no transitive entry without its intermediates, no edge
//! in two partitions. A test covers the refusals its author thought of. This target
//! builds candidate sets from an unstructured byte stream, so the refusals meet
//! combinations no author enumerated - and the invariants below hold over all of them.
//!
//! # The invariants, which are the point
//!
//! Every assertion here is a rule the specification states and the engine claims to
//! enforce on any output it produces. A fuzz run that only avoided panics would let a
//! regression through as long as it failed politely; these catch a set that is published
//! in a shape the specification forbids.
//!
//! # Termination
//!
//! The closure is bounded by hops and by nodes, and a candidate set derived from fuzzer
//! bytes can describe a cycle. That is deliberate: an unbounded traversal over a cyclic
//! graph does not terminate, so the bounds are the property under test, not a detail of
//! it. The closure is asked for its truncation rather than assumed to have finished.

#![no_main]

use arbitrary::Unstructured;
use libfuzzer_sys::fuzz_target;

use amasario_core::{
    Basis, DependencyClass, EntityKind, EntityRef, EvidenceType, Network, NetworkType,
    ObservationBoundary, Relationship,
};
use amasario_dependency::classifier::{Candidate, EvidenceRef};
use amasario_dependency::transitive::{
    DEFAULT_MAX_NODES, Limits, close, close_set,
};
use amasario_dependency::{DependencySet, resolve};

/// The relationships a cross-contract dependency can be asserted with.
///
/// A fixed list rather than a coerced integer, because the point is to reach the rules
/// that gate each relationship, and an unrecognised code would only reach the same
/// refusal every time.
const RELATIONSHIPS: [Relationship; 6] = [
    Relationship::DependsOn,
    Relationship::Invocates,
    Relationship::BuiltFrom,
    Relationship::DerivedFrom,
    Relationship::DeployedAs,
    Relationship::Affects,
];

/// The bases a candidate can rest on.
const BASES: [Basis; 5] = [
    Basis::ObservedInvocation,
    Basis::ObservedEvent,
    Basis::ResolvedLockfile,
    Basis::EmbeddedDigest,
    Basis::DeclaredManifest,
];

/// The entity kinds an endpoint can be.
const KINDS: [EntityKind; 3] = [EntityKind::Contract, EntityKind::Wasm, EntityKind::Source];

/// The evidence kinds a citation can name.
const EVIDENCE: [EvidenceType; 2] = [EvidenceType::Transaction, EvidenceType::Observation];

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

fuzz_target!(|data: &[u8]| {
    let mut input = Unstructured::new(data);

    let attempts = input.int_in_range(0..=16_u16).unwrap_or(0);
    let mut candidates: Vec<Candidate> = Vec::new();

    for _ in 0..attempts {
        let Ok(subject_index) = input.int_in_range(0..=7_u16) else {
            break;
        };
        let Ok(object_index) = input.int_in_range(0..=7_u16) else {
            break;
        };
        let Ok(relationship_index) = input.int_in_range(0..=RELATIONSHIPS.len() - 1) else {
            break;
        };
        let Ok(basis_index) = input.int_in_range(0..=BASES.len() - 1) else {
            break;
        };
        let Ok(subject_kind_index) = input.int_in_range(0..=KINDS.len() - 1) else {
            break;
        };
        let Ok(object_kind_index) = input.int_in_range(0..=KINDS.len() - 1) else {
            break;
        };
        let Ok(evidence_index) = input.int_in_range(0..=EVIDENCE.len() - 1) else {
            break;
        };
        // Half the candidates cite no evidence, which is a refused candidate rather than
        // an unclassifiable one; both paths are worth reaching.
        let Ok(citation_count) = input.int_in_range(0..=2_usize) else {
            break;
        };
        let Ok(outcome) = input.arbitrary::<bool>() else {
            break;
        };

        let citation: Vec<EvidenceRef> = (0..citation_count)
            .filter_map(|index| {
                EvidenceRef::new(EVIDENCE[evidence_index], format!("e-fuzz-{index}")).ok()
            })
            .collect();

        let candidate = Candidate::new(
            entity(KINDS[subject_kind_index], subject_index),
            entity(KINDS[object_kind_index], object_index),
            RELATIONSHIPS[relationship_index],
            BASES[basis_index],
            citation,
        );
        // A refused candidate is a result, not a failure: the constructor exists to
        // refuse the impossible before anything can be classified, and reaching the
        // refusal is what shows the rule is enforced.
        if let Ok(candidate) = candidate {
            candidates.push(candidate.observed_at(boundary()).with_outcome(Some(outcome)));
        }
    }

    let subject = entity(EntityKind::Contract, 0);
    let Ok(mut set) = resolve(subject.clone(), Some(boundary()), &candidates, 4) else {
        return;
    };

    assert_set_invariants(&set);

    // The closure walks the set's own observations, bounded, and merges what it finds
    // back in. Both the walking and the merging are checked.
    let source = set.clone();
    let Ok(limits) = Limits::new(4, DEFAULT_MAX_NODES) else {
        return;
    };
    let closure = close(&subject, &source, limits);
    assert!(
        closure.deepest <= limits.max_depth,
        "a closure reported a dependency deeper than its own bound"
    );
    if closure.truncated {
        assert!(
            closure.truncation_reason.is_some(),
            "a truncated closure did not say why it stopped"
        );
    }

    if close_set(&mut set, &source, limits).is_ok() {
        assert_set_invariants(&set);
    }
});

/// Every rule the specification states about a published set, checked on the result.
fn assert_set_invariants(set: &DependencySet) {
    if set.truncated {
        assert!(
            set.truncation_reason.is_some(),
            "a truncated set must state why traversal stopped"
        );
    } else {
        assert!(
            set.truncation_reason.is_none(),
            "a complete set cannot carry a truncation reason"
        );
    }

    for dependency in set.all() {
        assert!(
            !dependency.evidence.is_empty(),
            "a dependency was published with no evidence"
        );
        assert!(
            !dependency.reason.is_empty(),
            "a dependency was published with no reason"
        );
        assert!(
            !dependency.classes.is_empty(),
            "a dependency was published with no class"
        );
        assert!(
            dependency.subject != dependency.object,
            "a self-dependency was published"
        );

        if dependency.classes.contains(&DependencyClass::Transitive) {
            assert!(
                dependency.depth >= 2,
                "a transitive dependency is at least two hops from its subject"
            );
            assert!(
                !dependency.path.is_empty(),
                "a transitive dependency was published without the path that establishes it"
            );
        } else {
            assert_eq!(
                dependency.depth, 0,
                "only a transitive dependency is more than zero hops from itself"
            );
            assert!(
                dependency.path.is_empty(),
                "a direct dependency has no intermediate entities"
            );
        }
    }

    // The partition rule: one relationship cannot be in both partitions.
    for direct in &set.direct {
        assert!(
            !set.transitive.iter().any(|transitive| {
                transitive.subject == direct.subject
                    && transitive.object == direct.object
                    && transitive.relationship == direct.relationship
            }),
            "a relationship appears in both partitions"
        );
    }
}
