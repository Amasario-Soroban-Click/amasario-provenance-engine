//! The dependency layer, checked against the documents it publishes.
//!
//! # What this suite is for
//!
//! The unit tests in `amasario-dependency` prove the resolver's behaviour against
//! candidates built inside the test. This suite proves something the unit tests cannot:
//! that the *documents committed under `fixtures/dependencies/`* are the ones the
//! engine produces today, and that they still satisfy the rules a consumer will apply
//! to them - the partition rule, the citation rule, and the rule that a refusal is
//! published as a refusal rather than as an absence.
//!
//! A fixture is the only artefact in this repository that a reader meets without
//! running anything. It therefore has to be right in a way that a test does not.

use amasario_dependency::document::{DependencySetDocument, UnresolvedDocument, UnresolvedReason};
use amasario_integration_tests::corpus::{Corpus, assert_committed, parse_committed};
use amasario_integration_tests::documents::{self, SetKind};

/// Every set kind is committed, and what is committed is what the model produces.
#[test]
fn every_set_fixture_matches_its_builder() {
    for kind in SetKind::all() {
        let file = format!("{}.json", kind.file());
        let built = documents::set_document(*kind);
        assert_committed("dependencies", &file, &documents::rendered(&built));
    }
}

/// Every fixture parses with the document type's own deserialiser.
///
/// Separate from the drift check on purpose: a fixture could match its builder byte for
/// byte and still be unreadable, if the type acquired a field the fixtures predate. The
/// parse is what a consumer does first, so it is what is checked first.
#[test]
fn every_set_fixture_parses_as_the_document_the_specification_defines() {
    for kind in SetKind::all() {
        let file = format!("{}.json", kind.file());
        let document: DependencySetDocument = parse_committed("dependencies", &file);
        assert!(
            document.partition_violations().is_empty(),
            "{file}: {:?}",
            document.partition_violations()
        );
    }
}

/// The partition the schema requires holds in every committed fixture.
///
/// `dependency-set.schema.json` names the rule and cannot express it, so a consumer
/// checks it in code. Every fixture is checked here so that a future fixture cannot be
/// committed with an edge in neither partition.
#[test]
fn every_edge_appears_in_exactly_one_partition() {
    for kind in SetKind::all() {
        let file = format!("{}.json", kind.file());
        let document: DependencySetDocument = parse_committed("dependencies", &file);
        assert_eq!(
            document.edges.len(),
            document.direct.len() + document.transitive.len(),
            "{file}: an edge is missing from both partitions"
        );
        for id in &document.direct {
            assert!(
                !document.transitive.contains(id),
                "{file}: {id} is in both partitions"
            );
        }
    }
}

/// The direct fixture is direct, and the transitive one is not.
#[test]
fn the_direct_and_transitive_fixtures_are_what_they_claim() {
    let direct = documents::set(SetKind::Direct);
    assert_eq!(direct.direct.len(), 2);
    assert!(
        direct.transitive.is_empty(),
        "a set bounded at one hop has no transitive edges"
    );
    for dependency in &direct.direct {
        assert_eq!(
            dependency.depth, 0,
            "a direct edge is zero hops from itself"
        );
        assert!(
            dependency.path.is_empty(),
            "a direct edge has no intermediate entities"
        );
    }

    let transitive = documents::set(SetKind::Transitive);
    assert_eq!(
        transitive.direct.len(),
        1,
        "the corpus subject invokes one contract"
    );
    assert!(
        !transitive.transitive.is_empty(),
        "the transitive fixture must actually hold a transitive edge"
    );
    for dependency in &transitive.transitive {
        assert!(
            dependency.depth >= 2,
            "a transitive dependency is one the subject reaches only through an \
             intermediate, which is at least two hops: {} is at {}",
            dependency.object.id,
            dependency.depth
        );
        assert!(
            !dependency.path.is_empty(),
            "a transitive edge must carry the path that establishes it"
        );
        assert!(
            dependency
                .classes
                .contains(&amasario_core::DependencyClass::Transitive),
            "a closed edge carries the transitive class"
        );
    }

    // The far contract is reached through `bravo` and not directly, which is what makes
    // the second hop the route that establishes the dependency rather than a redundant
    // alternative to a direct edge.
    let bravo = documents::bravo();
    let charlie = documents::charlie();
    assert!(
        !transitive
            .direct
            .iter()
            .any(|edge| edge.object.id == charlie.id),
        "sanity: the subject has no direct edge to the far contract"
    );
    assert!(
        transitive
            .direct
            .iter()
            .all(|edge| edge.object.id != charlie.id),
        "the subject must not reach the two-hop contract directly"
    );
    assert!(
        transitive
            .transitive
            .iter()
            .any(|edge| edge.object.id == charlie.id
                && edge.path.iter().any(|hop| hop.id == bravo.id)),
        "the two-hop contract is reached through bravo"
    );
}

/// Every edge cites evidence, and every citation is non-empty.
///
/// The specification makes evidence non-negotiable: a dependency with no evidence is a
/// claim with no support, which is the thing the whole repository exists to prevent.
#[test]
fn no_edge_without_evidence_is_published() {
    for kind in SetKind::all() {
        let file = format!("{}.json", kind.file());
        let document: DependencySetDocument = parse_committed("dependencies", &file);
        for edge in &document.edges {
            assert!(
                !edge.evidence.is_empty(),
                "{file}: edge {} cites no evidence",
                edge.id
            );
            assert!(
                !edge.confidence.evidence.is_empty(),
                "{file}: edge {} has a confidence with no supporting citation",
                edge.id
            );
        }
    }
}

/// A refused candidate is published as a refusal, not dropped.
///
/// This is the assertion the corpus is most valuable for. Without it, a consumer that
/// saw an empty `edges` array would conclude the contract depends on nothing, when the
/// engine was actually told the contract invokes something and refused to call it a
/// dependency because the outcome was never reported.
#[test]
fn the_refused_fixture_publishes_the_refusal_rather_than_an_empty_set() {
    let document: DependencySetDocument = parse_committed("dependencies", "refused.json");
    assert!(
        document.edges.is_empty(),
        "the fixture's candidate is refused, so no edge exists"
    );
    assert_eq!(
        document.unresolved.len(),
        1,
        "the refusal must be published; an empty set with no explanation reads as \
         'nothing depends on anything'"
    );

    let entry: &UnresolvedDocument = &document.unresolved[0];
    assert_eq!(entry.reason, UnresolvedReason::NotPermitted);
    assert!(entry.source.is_some(), "the refusal is attributable");
    assert!(
        entry
            .detail
            .as_deref()
            .is_some_and(|detail| detail.contains("OBSERVED_INVOCATION")),
        "the basis survives in the detail: {:?}",
        entry.detail
    );
}

/// A rule refusal is never published as a transport failure.
///
/// The distinction `docs/verification.md` requires: "the endpoint could not be asked"
/// and "the specification would not permit the claim" are different findings, and a
/// consumer that could not tell them apart would retry a refusal forever.
#[test]
fn no_fixture_confuses_a_refusal_with_a_transport_failure() {
    for kind in SetKind::all() {
        let file = format!("{}.json", kind.file());
        let document: DependencySetDocument = parse_committed("dependencies", &file);
        for entry in &document.unresolved {
            assert_eq!(
                entry.reason,
                UnresolvedReason::NotPermitted,
                "{file}: only a rule refusal can reach a fixture, because the fixtures are \
                 built from observed invocations rather than from a network"
            );
        }
    }
}

/// The fixtures that hold a refusal publish one, and the others do not.
///
/// Two of the four exist to carry a refusal: `mixed` records one alongside its resolved
/// edges, to show that the two live side by side, and `refused` records one with no
/// edges at all, to show that an empty set is not the same as an empty result.
#[test]
fn the_fixtures_that_carry_a_refusal_publish_it() {
    let expected = |kind: SetKind| match kind {
        SetKind::Direct | SetKind::Transitive => 0,
        SetKind::Mixed | SetKind::Refused => 1,
    };
    for kind in SetKind::all() {
        let document = documents::set_document(*kind);
        assert_eq!(
            document.unresolved.len(),
            expected(*kind),
            "{}: unexpected number of refusals",
            kind.file()
        );
    }

    // The distinction the pair exists to draw.
    let mixed = documents::set_document(SetKind::Mixed);
    assert!(!mixed.edges.is_empty() && !mixed.unresolved.is_empty());
    let refused = documents::set_document(SetKind::Refused);
    assert!(refused.edges.is_empty() && !refused.unresolved.is_empty());
}

/// Two builds of one fixture produce the same bytes.
///
/// The corpus is generated, so determinism is what makes it reviewable: a diff under
/// `fixtures/` must mean the model changed, never that a hash map iterated differently.
#[test]
fn building_a_fixture_twice_produces_the_same_bytes() {
    for kind in SetKind::all() {
        assert_eq!(
            documents::rendered(&documents::set_document(*kind)),
            documents::rendered(&documents::set_document(*kind)),
            "{} is not deterministic",
            kind.file()
        );
    }
}

/// The document is byte-identical to the in-memory set's projection.
///
/// A weaker check than it looks: it catches a fixture that was edited by hand into
/// something the model still accepts. The drift check above catches more, and this
/// catches the case where both sides were regenerated from a changed model without the
/// change being noticed.
#[test]
fn a_committed_fixture_describes_the_same_set_as_the_builder() {
    for kind in SetKind::all() {
        let file = format!("{}.json", kind.file());
        let committed: DependencySetDocument = parse_committed("dependencies", &file);
        assert_eq!(
            committed,
            documents::set_document(*kind),
            "{file} describes a different set than the builder produces"
        );
    }
}

/// The corpus directory holds exactly the fixture set the builder defines.
///
/// A stale file left behind by a renamed builder is a fixture no test reads, which is
/// worse than a missing one: it looks like coverage.
#[test]
fn the_corpus_holds_no_fixture_the_builder_does_not_define() {
    let mut expected: Vec<String> = SetKind::all()
        .iter()
        .map(|kind| format!("{}.json", kind.file()))
        .collect();
    expected.sort();
    let actual: Vec<String> = Corpus::files_in("dependencies")
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

/// The corpus observes calls from more than one contract.
///
/// A single observer's set cannot produce a transitive dependency: a second hop needs
/// another contract's edge, and the detector attributes a call to the contract that made
/// it. This asserts the corpus actually holds the second observer's edge, so a
/// transitive fixture cannot silently become a direct one.
#[test]
fn the_corpus_observes_calls_from_more_than_one_contract() {
    let edges = documents::observed_edges();
    let subjects: std::collections::BTreeSet<&str> =
        edges.iter().map(|edge| edge.subject.id.as_str()).collect();
    assert!(
        edges.len() >= 3,
        "the corpus observes {} edge(s): {:?}",
        edges.len(),
        edges
            .iter()
            .map(|edge| format!("{}->{}", edge.subject.id, edge.object.id))
            .collect::<Vec<_>>()
    );
    assert!(
        subjects.len() >= 3,
        "the corpus observes calls from {} contract(s): {subjects:?}",
        subjects.len()
    );
    assert_eq!(
        edges.iter().filter(|edge| edge.depth == 0).count(),
        edges.len()
    );
}
