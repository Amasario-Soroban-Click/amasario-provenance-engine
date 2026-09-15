//! Conformance checks that only make sense across the crate's modules.
//!
//! The unit tests inside each module check that module's own behaviour. What they cannot
//! check is that the modules agree with one another, and that is where an impact analysis
//! is most likely to go quietly wrong: the direction is read in one place, the traversal
//! is bounded in another, the exclusions are computed in a third, and a disagreement
//! between any two of them produces a finding that looks entirely reasonable.
//!
//! Every test here is stated against the specification rather than against the code:
//! `taxonomies/relationship-types.yaml` for the propagation, `rules/impact/direct-impact.yaml`
//! for the one-hop pass, and `rules/impact/transitive-impact.yaml` for the multi-hop one.

use amasario_core::{
    Basis, Confidence, ConfidenceLevel, DependencyClass, EntityKind, EntityRef, LedgerSequence,
    Relationship, VerificationStatus,
};
use amasario_dependency::{Dependency, EvidenceRef, Limits};
use amasario_graph::Graph;
use amasario_impact::{
    ChangeType, DeploymentRecord, DeploymentStatus, ImpactContext, ImpactType, analyze,
    analyze_direct, context, excluded_deployments,
};

fn entity(kind: EntityKind, id: &str) -> EntityRef {
    EntityRef::new(kind, id).expect("a non-empty identifier")
}

const TRANSACTION: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

/// An edge with the confidence and evidence the rule requires of every step.
fn edge(
    subject: &EntityRef,
    object: &EntityRef,
    relationship: Relationship,
    level: ConfidenceLevel,
) -> Dependency {
    Dependency {
        subject: subject.clone(),
        object: object.clone(),
        relationship,
        classes: vec![DependencyClass::Contract],
        basis: Basis::ObservedInvocation,
        confidence: Confidence::new(level, vec![TRANSACTION.to_owned()], Vec::new())
            .expect("a confidence with evidence"),
        verification: VerificationStatus::Verified,
        evidence: vec![
            EvidenceRef::new(amasario_core::EvidenceType::Transaction, TRANSACTION)
                .expect("a citation"),
        ],
        path: Vec::new(),
        depth: 0,
        reason: "an observed relationship".to_owned(),
        observed_at: None,
    }
}

fn graph(edges: Vec<Dependency>) -> Graph {
    let mut graph = Graph::new("conformance").expect("a graph");
    for edge in edges {
        graph.add_edge(edge).expect("an edge");
    }
    graph.canonicalise();
    graph
}

/// A graph containing one edge of every relationship the taxonomy defines, so that a
/// propagation rule that is wrong for one of them cannot pass by being right for the rest.
///
/// ```text
/// C-proxy         DEPENDS_ON     C-implementation
/// C-implementation DEPLOYED_AS   D-implementation
/// C-implementation OBSERVED_IN   <the deployment transaction>
/// C-proxy         AFFECTS        C-other
/// <the executable> VERIFIED_BY   build-1
/// artifact-a      DERIVED_FROM   artifact-b
/// ```
fn mixed_graph() -> Graph {
    let proxy = entity(EntityKind::Contract, "C-proxy");
    let implementation = entity(EntityKind::Contract, "C-implementation");
    let other = entity(EntityKind::Contract, "C-other");
    let deployment = entity(EntityKind::Deployment, "D-implementation");
    let transaction = entity(EntityKind::Transaction, TRANSACTION);
    let executable = entity(EntityKind::Wasm, &"ab".repeat(32));
    let build = entity(EntityKind::Build, "build-1");
    let artifact = entity(EntityKind::Artifact, &"cd".repeat(32));
    let input = entity(EntityKind::Artifact, &"ef".repeat(32));

    graph(vec![
        edge(
            &proxy,
            &implementation,
            Relationship::DependsOn,
            ConfidenceLevel::HighConfidence,
        ),
        edge(
            &implementation,
            &deployment,
            Relationship::DeployedAs,
            ConfidenceLevel::HighConfidence,
        ),
        edge(
            &implementation,
            &transaction,
            Relationship::ObservedIn,
            ConfidenceLevel::HighConfidence,
        ),
        edge(
            &proxy,
            &other,
            Relationship::Affects,
            ConfidenceLevel::MediumConfidence,
        ),
        edge(
            &executable,
            &build,
            Relationship::VerifiedBy,
            ConfidenceLevel::HighConfidence,
        ),
        edge(
            &artifact,
            &input,
            Relationship::DerivedFrom,
            ConfidenceLevel::LowConfidence,
        ),
    ])
}

fn limits(depth: usize) -> Limits {
    Limits::new(depth, 256).expect("bounded limits")
}

/// The affected entities of an analysis, as `KIND:id` pairs, sorted.
fn affected(analysis: &amasario_impact::ImpactAnalysis) -> Vec<String> {
    let mut entities: Vec<String> = analysis
        .affected_entities()
        .into_iter()
        .map(ToString::to_string)
        .collect();
    entities.sort_unstable();
    entities
}

#[test]
fn no_finding_ever_traverses_a_relationship_that_carries_no_change() {
    // `rules/impact/direct-impact.yaml`: an impact finding "MUST NOT traverse a
    // relationship whose declared change propagation is none". The mixed graph contains
    // one `OBSERVED_IN` and one `VERIFIED_BY` edge, so a traversal that crossed either
    // would show up here as a step.
    let graph = mixed_graph();
    let impact = ImpactContext::new(&graph);
    for start in [
        entity(EntityKind::Contract, "C-implementation"),
        entity(EntityKind::Contract, "C-proxy"),
        entity(EntityKind::Contract, "C-other"),
        entity(EntityKind::Artifact, &"ef".repeat(32)),
    ] {
        let analysis = analyze(&impact, &start, None, limits(8)).expect("an analysis");
        analysis.validate().expect("every finding passes its rules");
        for finding in &analysis.findings {
            let path = finding
                .path
                .as_ref()
                .expect("a finding at a non-zero depth has a path");
            for step in &path.steps {
                assert!(
                    !matches!(
                        step.relationship,
                        Relationship::ObservedIn | Relationship::VerifiedBy
                    ),
                    "{} traversed {}",
                    start,
                    step.relationship.as_str()
                );
            }
        }
    }

    // An entity reachable only through a non-propagating edge is not reached. The
    // transaction and the build are the objects of the two non-propagating edges, so a
    // change to either carries nothing: a re-observation does not change the fact, and a
    // re-verification is not a change.
    for start in [
        entity(EntityKind::Transaction, TRANSACTION),
        entity(EntityKind::Build, "build-1"),
        entity(EntityKind::Wasm, &"ab".repeat(32)),
    ] {
        let analysis = analyze(&impact, &start, None, limits(8)).expect("an analysis");
        assert!(
            analysis.is_empty(),
            "{} should reach nothing: observation and verification carry no change",
            start
        );
        assert!(
            analysis.is_conclusive(),
            "the empty result is a negative finding, not a gap"
        );
    }
}

#[test]
fn the_one_hop_pass_and_a_one_hop_bound_agree_on_which_entities_are_affected() {
    // Two implementations of the same question must not disagree. The pass in
    // `amasario_impact::direct` exists so that a caller wanting only the immediate
    // dependents of a large graph does not run a bounded traversal and discard the rest,
    // and the price of having both is this check.
    let graph = mixed_graph();
    let impact = ImpactContext::new(&graph);
    for start in [
        entity(EntityKind::Contract, "C-implementation"),
        entity(EntityKind::Contract, "C-proxy"),
        entity(EntityKind::Artifact, &"ef".repeat(32)),
        entity(EntityKind::Deployment, "D-implementation"),
    ] {
        let pass = analyze_direct(&impact, &start, None).expect("a direct analysis");
        let bounded = analyze(&impact, &start, None, limits(1)).expect("a bounded analysis");
        assert_eq!(
            affected(&pass),
            affected(&bounded),
            "{start}: the one-hop pass and a one-hop bound disagree"
        );
        assert_eq!(
            pass.truncated, bounded.truncated,
            "{start}: the two disagree about whether the pass was bounded"
        );
        assert_eq!(
            pass.deepest(),
            bounded.deepest(),
            "{start}: the two disagree about how far the pass reached"
        );
    }
}

#[test]
fn a_change_to_a_deployment_reaches_the_contract_it_installed_and_beyond() {
    // `DEPLOYED_AS` is `object_to_subject`, so a change to the deployment record reaches
    // the contract that came from it. Stated as its own test because getting this
    // backwards would report an upgrade as having no impact on the contract it changed.
    let graph = mixed_graph();
    let impact = ImpactContext::new(&graph);
    let deployment = entity(EntityKind::Deployment, "D-implementation");
    let analysis = analyze(&impact, &deployment, None, limits(4)).expect("an analysis");
    analysis.validate().expect("every finding passes its rules");

    // One hop: the contract that was installed. Two hops: the proxy that depends on it.
    // Three: what the proxy affects.
    assert_eq!(
        affected(&analysis),
        vec![
            "CONTRACT:C-implementation",
            "CONTRACT:C-other",
            "CONTRACT:C-proxy",
        ]
    );

    let finding = &analysis.findings[0];
    assert!(finding.is_direct());
    assert!(finding.has(ImpactType::Contract));
    assert!(
        !finding.has(ImpactType::Deployment),
        "the deployment is the changed entity, not the affected one"
    );
    // The route is shown, so a reader can check the single hop rather than trust it.
    let path = finding.path.as_ref().expect("a path");
    assert_eq!(path.nodes.len(), 2);
    assert_eq!(path.steps.len(), 1);
    assert_eq!(path.steps[0].direction.as_str(), "inverted");
    assert_eq!(
        path.termination.expect("a termination").as_str(),
        "TARGET_REACHED",
        "the contract has further to go, so the pass stopped by choice"
    );
}

#[test]
fn a_deployment_that_never_took_effect_reaches_nothing_and_is_reached_by_nothing() {
    let graph = mixed_graph();
    let deployment = entity(EntityKind::Deployment, "D-implementation");
    let failed = DeploymentRecord::new(
        deployment.clone(),
        LedgerSequence::new(1_000).expect("a real ledger"),
        DeploymentStatus::Failed,
    )
    .expect("a deployment record");
    assert_eq!(
        excluded_deployments(std::slice::from_ref(&failed)),
        vec![deployment.clone()]
    );
    assert!(!failed.is_eligible());

    // Without the record, a change to the deployment reaches three contracts, which is
    // what makes the exclusion the reason for the empty result rather than the graph.
    let open = ImpactContext::new(&graph);
    let from_deployment = analyze(&open, &deployment, None, limits(4)).expect("an analysis");
    assert_eq!(
        affected(&from_deployment),
        vec![
            "CONTRACT:C-implementation",
            "CONTRACT:C-other",
            "CONTRACT:C-proxy",
        ]
    );

    let impact = context(&graph, std::slice::from_ref(&failed));

    // The failed deployment cannot be affected by the contract that would have installed
    // it, because the installation did not happen.
    let from_contract = analyze(
        &impact,
        &entity(EntityKind::Contract, "C-implementation"),
        None,
        limits(4),
    )
    .expect("an analysis");
    assert!(
        from_contract
            .affected_of_kind(EntityKind::Deployment)
            .is_empty(),
        "a failed deployment must not be reported as affected"
    );

    // The contract is still reachable through its dependents, so the exclusion is
    // scoped to the deployment rather than silencing the whole analysis.
    assert_eq!(
        affected(&from_contract),
        vec!["CONTRACT:C-other", "CONTRACT:C-proxy"]
    );

    // And nothing is reachable from the failed deployment either: the executable behind
    // it is not the one at that address.
    let from_deployment = analyze(&impact, &deployment, None, limits(4)).expect("an analysis");
    assert!(from_deployment.is_empty());
    assert!(from_deployment.is_conclusive());
    assert!(
        !DeploymentStatus::Failed.eligible_for_impact(),
        "the status vocabulary the impact layer re-exports must agree with the record's own"
    );
}

#[test]
fn every_route_is_disclosed_and_its_aggregate_is_its_weakest_link() {
    // `rules/impact/transitive-impact.yaml`: a multi-hop finding "MUST carry a path with
    // per-step evidence and confidence", "MUST state the direction", and "its aggregated
    // confidence MUST equal the minimum confidence ordinal among its steps".
    let graph = mixed_graph();
    let impact = ImpactContext::new(&graph);

    // A change to the artifact input reaches the derived artifact, one hop away, at
    // LOW_CONFIDENCE - the edge's own level, with no upgrade and no downgrade.
    let input = entity(EntityKind::Artifact, &"ef".repeat(32));
    let analysis =
        analyze(&impact, &input, Some(ChangeType::Modified), limits(4)).expect("an analysis");
    let finding = analysis
        .findings
        .iter()
        .find(|finding| finding.affected_entity == entity(EntityKind::Artifact, &"cd".repeat(32)))
        .expect("the derived artifact is affected");
    assert_eq!(finding.confidence.level, ConfidenceLevel::LowConfidence);
    assert_eq!(finding.change_type, Some(ChangeType::Modified));
    assert!(
        finding
            .evidence
            .iter()
            .all(|citation| citation.id == TRANSACTION),
        "the finding cites what its steps cite"
    );
    assert_eq!(
        finding.direction,
        Some(amasario_impact::ImpactDirection::Dependents)
    );
    assert!(finding.is_valid(), "{:?}", finding.failures());

    // A change to the deployed contract reaches the proxy that depends on it, and what
    // the proxy affects. Every route is disclosed and every route's classification
    // matches its own length.
    let implementation = entity(EntityKind::Contract, "C-implementation");
    let analysis = analyze(&impact, &implementation, None, limits(4)).expect("an analysis");
    assert_eq!(
        affected(&analysis),
        vec!["CONTRACT:C-other", "CONTRACT:C-proxy"]
    );
    for finding in &analysis.findings {
        let path = finding.path.as_ref().expect("a finding shows its route");
        assert_eq!(path.steps.len(), finding.hop_depth);
        assert_eq!(path.nodes.len(), finding.hop_depth + 1);
        assert!(
            path.steps.iter().all(|step| !step.evidence.is_empty()),
            "every step cites evidence, whatever its depth"
        );
        assert_eq!(
            finding.has(ImpactType::Direct),
            finding.hop_depth == 1,
            "the distance classification is derived from the depth"
        );
        assert_eq!(
            finding.confidence.level,
            path.confidence.as_ref().expect("an aggregate").level,
            "the finding's aggregate is its path's weakest link"
        );
    }
}

#[test]
fn a_bounded_analysis_is_never_indistinguishable_from_a_complete_one() {
    // The single most important property: "nothing is affected" and "nothing was found
    // within the bound" lead to opposite decisions.
    let independent = entity(EntityKind::Contract, "C-independent");
    let top = entity(EntityKind::Contract, "C-top");
    let mid = entity(EntityKind::Contract, "C-mid");
    let mut graph = graph(vec![
        edge(
            &top,
            &independent,
            Relationship::DependsOn,
            ConfidenceLevel::Verified,
        ),
        edge(
            &mid,
            &top,
            Relationship::DependsOn,
            ConfidenceLevel::Verified,
        ),
    ]);
    graph.canonicalise();
    let impact = ImpactContext::new(&graph);

    // Nothing depends on C-mid, so a change to it reaches nothing and the traversal was
    // complete. That is a negative finding.
    let complete = analyze(&impact, &mid, None, limits(4)).expect("an analysis");
    assert!(complete.is_empty());
    assert!(complete.is_conclusive());
    assert!(complete.truncation_reason.is_none());
    assert!(!complete.summary().contains("bounded"));

    // A one-hop bound over a graph with further to go is not conclusive, and the finding
    // carries that fact rather than leaving it to the summary.
    let bounded = analyze(&impact, &independent, None, limits(1)).expect("an analysis");
    assert!(!bounded.is_conclusive());
    assert!(bounded.truncated);
    assert!(bounded.summary().contains("bounded because"));
    assert_eq!(bounded.len(), 1, "only the first hop was reported");
    for finding in &bounded.findings {
        assert_eq!(finding.truncated, Some(true));
        let path = finding.path.as_ref().expect("a path");
        assert!(!path.is_complete());
        assert_eq!(
            path.termination.expect("a termination").as_str(),
            "MAX_DEPTH_REACHED"
        );
    }
}

#[test]
fn an_analysis_is_deterministic_and_its_identifiers_are_derived() {
    // `rules/impact/change-impact.yaml` requires a diff to be produced by canonical
    // comparison, which is only possible if two runs over one graph agree.
    let graph = mixed_graph();
    let impact = ImpactContext::new(&graph);
    let start = entity(EntityKind::Contract, "C-implementation");
    let first =
        analyze(&impact, &start, Some(ChangeType::Replaced), limits(4)).expect("an analysis");
    let second =
        analyze(&impact, &start, Some(ChangeType::Replaced), limits(4)).expect("an analysis");
    assert_eq!(first, second);

    // The identifiers are distinct within a run, so a diff can report a second finding
    // rather than the reordering of the first.
    assert_eq!(first.len(), 2);
    assert_ne!(first.findings[0].id, first.findings[1].id);

    // And they are derived rather than allocated: the same claim made twice is the same
    // finding, which is what lets a snapshot diff say "unchanged".
    for finding in &first.findings {
        assert_eq!(
            finding.id,
            amasario_impact::ImpactFinding::stable_id(
                &finding.changed_entity,
                &finding.affected_entity,
                finding.change_type,
            )
        );
        assert!(
            finding.path.as_ref().expect("a path").termination.is_some(),
            "every route says why it stopped"
        );
    }
}
