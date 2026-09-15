//! The impact layer, checked against the finding sets it publishes.
//!
//! # What a fixture can prove that a unit test cannot
//!
//! A unit test builds its graph inside the test, so it can only ever check the case its
//! author thought of. These fixtures are the *same* graph under four different bounds
//! and two different starting points, and the assertions below are about the differences
//! between them: one hop reaches one thing, two hops reach more, and a bound that
//! cannot reach the end of the chain must say that it could not.
//!
//! The last of those is the one worth having. An analysis that silently stopped at its
//! bound would report a smaller affected set than exists, and a reader would have no way
//! to tell that from a contract with no further dependencies. The `bounded` fixture is
//! the case that fails if the disclosure is ever lost.

use amasario_core::EntityKind;
use amasario_impact::ImpactFinding;
use amasario_integration_tests::corpus::{Corpus, assert_committed, parse_committed};
use amasario_integration_tests::documents::{self, ImpactKind};

/// Every impact fixture is committed, and what is committed is what the model produces.
#[test]
fn every_impact_fixture_matches_its_builder() {
    for kind in ImpactKind::all() {
        let file = format!("{}.json", kind.file());
        let built = documents::impact_analysis(*kind).findings;
        assert_committed("impact", &file, &documents::rendered(&built));
    }
}

/// Every fixture parses as a finding set and every finding satisfies its own rules.
#[test]
fn every_finding_satisfies_the_rules_it_is_derived_from() {
    for kind in ImpactKind::all() {
        let file = format!("{}.json", kind.file());
        let findings: Vec<ImpactFinding> = parse_committed("impact", &file);
        for finding in &findings {
            let failures = finding.failures();
            assert!(
                failures.is_empty(),
                "{file}: finding {}: {failures:?}",
                finding.id
            );
        }
    }
}

/// A finding one or more hops away carries a path and a direction.
///
/// A hop count without a route is an assertion with nothing behind it: the reader cannot
/// see which edges the claim rests on, and cannot disagree with it.
#[test]
fn a_multi_hop_finding_carries_the_route_it_travelled() {
    for kind in ImpactKind::all() {
        let file = format!("{}.json", kind.file());
        let findings: Vec<ImpactFinding> = parse_committed("impact", &file);
        for finding in &findings {
            if finding.hop_depth == 0 {
                assert!(
                    finding.path.is_none(),
                    "{file}: a zero-hop finding is the change itself and has no route"
                );
                continue;
            }
            let path = finding
                .path
                .as_ref()
                .unwrap_or_else(|| panic!("{file}: finding {} has no path", finding.id));
            assert_eq!(
                path.hop_depth, finding.hop_depth,
                "{file}: the path's length and the finding's hop depth disagree"
            );
            assert_eq!(
                path.nodes.len(),
                path.steps.len() + 1,
                "{file}: a path of N steps has N+1 nodes"
            );
            assert!(
                finding.direction.is_some(),
                "{file}: finding {} does not say which way the change travelled",
                finding.id
            );
            assert_eq!(
                finding.relationship_types.len(),
                path.steps.len(),
                "{file}: one relationship per step"
            );
        }
    }
}

/// A bound that cannot exhaust the graph is disclosed rather than hidden.
#[test]
fn the_bounded_fixture_says_it_was_bounded_and_the_unbounded_one_does_not() {
    let bounded = documents::impact_analysis(ImpactKind::Bounded);
    assert!(
        bounded.truncated,
        "a bound the chain outruns must be disclosed: {}",
        bounded.summary()
    );
    assert!(
        bounded.truncation_reason.is_some(),
        "a bounded analysis must say why it stopped"
    );
    assert!(
        !bounded.is_conclusive(),
        "a truncated analysis is not conclusive, and saying so is the point"
    );

    let unbounded = documents::impact_analysis(ImpactKind::MultiHop);
    assert!(
        !unbounded.truncated,
        "the same graph reached to its end is not truncated: {}",
        unbounded.summary()
    );
    assert!(unbounded.is_conclusive());
    assert!(
        unbounded.deepest() > bounded.deepest(),
        "the bounded fixture reaches less far than the unbounded one, which is why the \
         disclosure matters"
    );
}

/// A bound stops the analysis at the node it could not pass, not at the edge.
#[test]
fn the_bounded_fixture_reaches_exactly_its_bound() {
    let bounded = documents::impact_analysis(ImpactKind::Bounded);
    assert_eq!(bounded.limits.max_depth, 2);
    assert!(
        bounded.deepest() <= bounded.limits.max_depth,
        "no finding may be deeper than the bound"
    );
    // The entity past the bound must not appear, because appearing is what would make
    // the truncation invisible. The change starts at `delta` and travels against the
    // arrows, so the entity one hop beyond a bound of two is `alpha`.
    let alpha = documents::alpha();
    assert!(
        !bounded
            .affected_entities()
            .iter()
            .any(|entity| entity.id == alpha.id),
        "the entity beyond the bound must not be reported as affected: {}",
        bounded.summary()
    );

    let reached = documents::impact_analysis(ImpactKind::MultiHop);
    assert!(
        reached
            .affected_entities()
            .iter()
            .any(|entity| entity.id == alpha.id),
        "with enough depth the same analysis does reach it, so the two fixtures differ \
         only by the bound"
    );
}

/// One hop reaches less than two, and two less than the whole chain.
#[test]
fn a_wider_bound_reaches_further() {
    let direct = documents::impact_analysis(ImpactKind::Direct);
    assert_eq!(direct.limits.max_depth, 1);
    assert_eq!(direct.deepest(), 1, "one hop from the change");
    assert!(
        !direct.direct().is_empty(),
        "the contract the change starts at is invoked by another, so one direct dependent \
         is affected"
    );
    let alpha = documents::alpha();
    assert!(
        direct
            .affected_entities()
            .iter()
            .any(|entity| entity.id == alpha.id),
        "the invoker is affected by a change to what it invokes: {}",
        direct.summary()
    );

    let multi = documents::impact_analysis(ImpactKind::MultiHop);
    assert!(
        multi.findings.len() > direct.findings.len(),
        "a wider bound reaches more entities"
    );
}

/// The transitive fixture starts at a dependency, so the direction is the interesting one.
///
/// `charlie` changed and `alpha` requires `charlie`, so `alpha` is affected. This is the
/// direction that makes impact analysis worth having, and the fixture exists so that a
/// change which reversed it would fail rather than pass.
#[test]
fn a_change_to_a_dependency_reaches_the_contract_that_requires_it() {
    let analysis = documents::impact_analysis(ImpactKind::Transitive);
    assert_eq!(analysis.changed.id, documents::charlie().id);

    let alpha = documents::alpha();
    assert!(
        analysis
            .affected_entities()
            .iter()
            .any(|entity| entity.id == alpha.id),
        "the subject requires the changed contract, so it is affected: {}",
        analysis.summary()
    );
    assert!(
        analysis
            .findings
            .iter()
            .any(|finding| finding.changed_entity.id == documents::charlie().id),
        "at least one finding names the change's own entity"
    );
}

/// Every finding names a changed entity, an affected entity and an impact type.
#[test]
fn no_finding_is_published_without_its_terms() {
    for kind in ImpactKind::all() {
        let file = format!("{}.json", kind.file());
        let findings: Vec<ImpactFinding> = parse_committed("impact", &file);
        assert!(!findings.is_empty(), "{file} holds no findings at all");
        for finding in &findings {
            assert_eq!(
                finding.changed_entity.kind,
                EntityKind::Contract,
                "{file}: a change to something other than a contract is not what these \
                 fixtures describe"
            );
            assert!(
                !finding.impact_type.is_empty(),
                "{file}: finding {} carries no impact type",
                finding.id
            );
        }
    }
}

/// A finding's identifier is stable across two builds of the same fixture.
#[test]
fn finding_identifiers_are_stable_across_runs() {
    for kind in ImpactKind::all() {
        let first = documents::impact_analysis(*kind);
        let second = documents::impact_analysis(*kind);
        let first_ids: Vec<&str> = first.findings.iter().map(|f| f.id.as_str()).collect();
        let second_ids: Vec<&str> = second.findings.iter().map(|f| f.id.as_str()).collect();
        assert_eq!(
            first_ids, second_ids,
            "{kind:?}: a finding's identifier must survive a re-run, or a diff would report \
             every finding as replaced"
        );
    }
}

/// Building a fixture twice produces the same bytes.
#[test]
fn building_an_analysis_twice_produces_the_same_bytes() {
    for kind in ImpactKind::all() {
        assert_eq!(
            documents::rendered(&documents::impact_analysis(*kind).findings),
            documents::rendered(&documents::impact_analysis(*kind).findings),
            "{kind:?} is not deterministic"
        );
    }
}

/// The corpus directory holds exactly the fixture set the builder defines.
#[test]
fn the_corpus_holds_no_fixture_the_builder_does_not_define() {
    let mut expected: Vec<String> = ImpactKind::all()
        .iter()
        .map(|kind| format!("{}.json", kind.file()))
        .collect();
    expected.sort();
    let actual: Vec<String> = Corpus::files_in("impact")
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
