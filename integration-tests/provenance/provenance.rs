//! The provenance layer, checked against the chains it publishes.
//!
//! # The claim this suite exists to falsify
//!
//! The most dangerous thing a provenance tool can do is call a claim verified when the
//! evidence does not support it. `fixtures/provenance/mismatched-wasm.json` is the
//! fixture that would catch that: a deployment whose recorded module digest disagrees
//! with the digest the build produced. The specification's rule is that the result is
//! `CONFLICTING`, never `VERIFIED`, and if a future change to the matching layer
//! collapsed that distinction the fixture would still parse and the assertion below
//! would still fail - which is exactly what a fixture is for.
//!
//! # Direction is checked because it is easy to get backwards
//!
//! A chain link connects its *subject* to its *object*, and the direction is the
//! model's: `SOURCE_TO_BUILD` connects a `BUILD` to the `SOURCE` it read, not the other
//! way round. A chain built the wrong way reads plausibly and means the opposite, so
//! every link's endpoint kinds are asserted rather than assumed.

use amasario_core::{EntityKind, VerificationStatus};
use amasario_integration_tests::corpus::{Corpus, assert_committed, parse_committed};
use amasario_integration_tests::documents::{self, ProvenanceKind};
use amasario_provenance::{ChainLink, ChainLinkKind, ProvenanceChain};

/// Every chain fixture is committed, and what is committed is what the model produces.
#[test]
fn every_provenance_fixture_matches_its_builder() {
    for kind in ProvenanceKind::all() {
        let file = format!("{}.json", kind.file());
        let built = documents::provenance_chain(*kind);
        assert_committed("provenance", &file, &documents::rendered(&built));
    }
}

/// Every fixture parses as the chain type, and its links are in the model's order.
#[test]
fn every_chain_parses_with_its_links_in_order() {
    for kind in ProvenanceKind::all() {
        let file = format!("{}.json", kind.file());
        let chain: ProvenanceChain = parse_committed("provenance", &file);
        assert_eq!(chain.contract.kind, EntityKind::Contract);
        assert!(
            !chain.links.is_empty(),
            "{file} holds a chain with no links, which says nothing"
        );

        let positions: Vec<usize> = chain
            .links
            .iter()
            .map(|link| link.kind.position())
            .collect();
        let mut sorted = positions.clone();
        sorted.sort_unstable();
        assert_eq!(
            positions, sorted,
            "{file}: a chain's links describe one sequence and must not be reordered"
        );
    }
}

/// Every link connects the endpoint kinds its vocabulary requires.
#[test]
fn every_link_connects_the_endpoints_its_vocabulary_requires() {
    for kind in ProvenanceKind::all() {
        let file = format!("{}.json", kind.file());
        let chain: ProvenanceChain = parse_committed("provenance", &file);
        for link in &chain.links {
            assert_eq!(
                link.subject.kind,
                link.kind.subject_kind(),
                "{file}: {} has the wrong subject kind",
                link.kind
            );
            match (link.kind.object_kind(), &link.object) {
                (None, None) => {},
                (Some(expected), Some(object)) => assert_eq!(
                    object.kind, expected,
                    "{file}: {} has the wrong object kind",
                    link.kind
                ),
                (expected, actual) => panic!(
                    "{file}: {} must have object kind {expected:?} and has {actual:?}",
                    link.kind
                ),
            }
        }
    }
}

/// The verified fixture is verified all the way along.
#[test]
fn the_verified_chain_is_verified_at_every_link() {
    let chain = documents::provenance_chain(ProvenanceKind::Verified);
    assert_eq!(
        chain.links.len(),
        ChainLinkKind::all().len(),
        "a verified chain reaches every link the model names"
    );
    for link in &chain.links {
        assert_eq!(link.verification, VerificationStatus::Verified, "{link:?}");
        assert!(link.is_affirmed());
        assert!(!link.is_contradicted());
    }
}

/// A chain whose deployment disagrees with its build is `CONFLICTING`, not `VERIFIED`.
///
/// The single most important assertion in this file.
#[test]
fn a_mismatched_module_produces_a_conflict_rather_than_a_verification() {
    let chain = documents::provenance_chain(ProvenanceKind::MismatchedWasm);

    let conflicting: Vec<&ChainLink> = chain
        .links
        .iter()
        .filter(|link| link.verification == VerificationStatus::Conflicting)
        .collect();
    assert!(
        !conflicting.is_empty(),
        "a deployment whose module digest disagrees with the built one must be recorded as \
         conflicting"
    );
    assert!(
        conflicting
            .iter()
            .any(|link| link.kind == ChainLinkKind::ArtifactToWasm),
        "the disagreement is between the built artifact and the deployed module"
    );

    // And the conflict is visible on the link rather than only in a status word.
    for link in &conflicting {
        assert!(link.is_contradicted() || link.verification.is_refutation());
    }

    // Nothing in this chain may claim to be fully verified end to end.
    let fully_verified = chain
        .links
        .iter()
        .all(|link| link.verification == VerificationStatus::Verified);
    assert!(
        !fully_verified,
        "the mismatched chain must not read as a complete verification"
    );

    // The conflicting links cite both sides of the disagreement, which is what lets a
    // reader see that it is a disagreement rather than a gap.
    for link in &conflicting {
        assert!(
            link.confidence.evidence.len() >= 2,
            "{:?} cites both the build and the deployment: {:?}",
            link.kind,
            link.confidence.evidence
        );
    }
}

/// The unknown-source fixture stops where the evidence stops.
#[test]
fn an_unknown_source_produces_a_chain_that_stops_rather_than_pads() {
    let chain = documents::provenance_chain(ProvenanceKind::UnknownSource);
    assert_eq!(
        chain.links.len(),
        1,
        "a chain with no source evidence has one link, not six links of absence"
    );
    let link = &chain.links[0];
    assert_eq!(link.kind, ChainLinkKind::DeploymentToContract);
    assert_ne!(
        link.verification,
        VerificationStatus::Verified,
        "the deployment establishes the contract exists and nothing about its origin"
    );
}

/// The partially-verified fixture is partial, and says which links are.
#[test]
fn the_partial_chain_distinguishes_what_looked_and_what_did_not() {
    let chain = documents::provenance_chain(ProvenanceKind::Partial);
    assert!(
        chain
            .links
            .iter()
            .any(|link| link.verification == VerificationStatus::Verified),
        "the source was established"
    );
    assert!(
        chain
            .links
            .iter()
            .any(|link| link.verification == VerificationStatus::PartiallyVerified),
        "the build was established in part"
    );
    assert!(
        chain
            .links
            .iter()
            .any(|link| link.verification == VerificationStatus::Unverified),
        "the deployment was not established"
    );
}

/// A chain never claims more confidence than the basis supporting it permits.
#[test]
fn no_link_claims_more_confidence_than_its_basis_supports() {
    for kind in ProvenanceKind::all() {
        let file = format!("{}.json", kind.file());
        let chain: ProvenanceChain = parse_committed("provenance", &file);
        for link in &chain.links {
            // Ordinals run from weakest to strongest, so a level above its basis's
            // ceiling would be a claim the evidence cannot support.
            assert!(
                link.confidence.level.ordinal() <= link.basis.confidence_ceiling().ordinal(),
                "{file}: {} claims {:?} on a {:?} basis, whose ceiling is {:?}",
                link.kind,
                link.confidence.level,
                link.basis,
                link.basis.confidence_ceiling()
            );
        }
    }
}

/// A link with confidence cites the evidence that confidence rests on.
#[test]
fn every_link_cites_evidence() {
    for kind in ProvenanceKind::all() {
        let file = format!("{}.json", kind.file());
        let chain: ProvenanceChain = parse_committed("provenance", &file);
        for link in &chain.links {
            assert!(
                !link.confidence.evidence.is_empty(),
                "{file}: {} carries a confidence with no citation",
                link.kind
            );
            assert!(
                link.confidence.has_support(),
                "{file}: {} has no supporting evidence",
                link.kind
            );
        }
    }
}

/// Building a fixture twice produces the same bytes.
#[test]
fn building_a_chain_twice_produces_the_same_bytes() {
    for kind in ProvenanceKind::all() {
        assert_eq!(
            documents::rendered(&documents::provenance_chain(*kind)),
            documents::rendered(&documents::provenance_chain(*kind)),
            "{kind:?} is not deterministic"
        );
    }
}

/// The corpus directory holds exactly the fixture set the builder defines.
#[test]
fn the_corpus_holds_no_fixture_the_builder_does_not_define() {
    let mut expected: Vec<String> = ProvenanceKind::all()
        .iter()
        .map(|kind| format!("{}.json", kind.file()))
        .collect();
    expected.sort();
    let actual: Vec<String> = Corpus::files_in("provenance")
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
