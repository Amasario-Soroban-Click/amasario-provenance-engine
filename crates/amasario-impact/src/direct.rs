//! One-hop impact: the entities a change reaches directly.
//!
//! # What the rule asks of a one-hop finding
//!
//! `rules/impact/direct-impact.yaml` is short and entirely mechanical: a depth-one
//! finding "MUST include `DIRECT` in its impact type set, MUST carry a path whose node
//! count exceeds its step count by one, MUST carry evidence on the finding and on every
//! step of its path, and MUST state the direction in which the change propagated for the
//! finding and for each step." Each of those is satisfied by construction here rather
//! than checked afterwards: the classification is derived from the depth in
//! [`crate::affected::ImpactFinding::new`], the path is built from steps so its node
//! count is `steps + 1` by definition, a step exists only if its edge cited evidence,
//! and the direction is read from the relationship's `changePropagation`.
//!
//! # Why the direct pass is separate from the transitive one
//!
//! Not for efficiency. A one-hop finding is the one a reviewer will actually check by
//! hand, and the rule holds it to the same evidence standard as a five-hop one. Keeping
//! the pass separate keeps that standard visible: there is no path here that skips the
//! per-step evidence requirement because the hop count is small.
//!
//! # The reason, not a score
//!
//! `rules/impact/direct-impact.yaml` ends with a non-goal worth repeating: "This rule
//! does not claim the affected entity is broken, only that a change may reach it." The
//! reason text this module produces says what was traversed and nothing about
//! consequence, because a consequence is not derivable from a graph.

use amasario_core::EntityRef;

use crate::affected::{ChangeType, ImpactFinding, ImpactPath, PathTermination, path_evidence};
use crate::propagation::{ImpactContext, describe_route};

/// The direct findings a change produces, in canonical order.
///
/// Sorted by affected entity kind then identifier, which is the order the graph layer
/// already uses, so two runs over one graph produce the same list and a snapshot diff
/// does not report a reordering as a change.
///
/// A finding is built only for a step whose edge cited evidence, so an entity reachable
/// only through an unsupported edge is absent rather than present with a hollow
/// justification. That is deliberate: the specification requires evidence on every step,
/// and a finding with none could not be published anyway.
#[must_use]
pub fn findings(
    context: &ImpactContext<'_>,
    changed: &EntityRef,
    change_type: Option<ChangeType>,
    max_depth: usize,
) -> Vec<ImpactFinding> {
    let mut findings: Vec<ImpactFinding> = Vec::new();
    for step in context.steps_from(changed) {
        let target = step.target.clone();
        let Ok(path) = ImpactPath::from_steps(changed.clone(), vec![step]) else {
            // `from_steps` starts the path at the step's own source, and the step was
            // built with `changed` as its source, so this cannot fail. Skipping rather
            // than panicking keeps a defect in this module from taking down a run.
            continue;
        };
        let path = path.with_termination(termination_for(context, &target, max_depth));
        let Some(confidence) = path.confidence.clone() else {
            continue;
        };
        let evidence = path_evidence(&path.steps);
        let reason = describe_route(changed, &target, &path.steps);
        let Ok(mut finding) = ImpactFinding::new(
            changed.clone(),
            target,
            path.hop_depth,
            Some(path),
            change_type,
            evidence,
            confidence,
            reason,
        ) else {
            continue;
        };
        if let Some(boundary) = &context.boundary {
            finding = finding.with_boundary(boundary.clone());
        }
        findings.push(finding);
    }
    // Whether the pass was bounded is a property of the run rather than of one finding,
    // so it is decided once and attached to every finding the run produced.
    let truncated = findings.iter().any(|finding| {
        finding
            .path
            .as_ref()
            .and_then(|path| path.termination)
            .is_some_and(PathTermination::is_bound)
    });
    for finding in &mut findings {
        *finding = finding.clone().with_truncation(truncated);
    }
    findings.sort_by(|left, right| {
        left.affected_entity
            .kind
            .cmp(&right.affected_entity.kind)
            .then_with(|| left.affected_entity.id.cmp(&right.affected_entity.id))
            .then_with(|| left.relationship_types.cmp(&right.relationship_types))
    });
    findings
}

/// Why a one-hop path stops where it does.
///
/// A direct pass stops at depth one because the analysis asked it to, so the useful
/// distinction is whether the affected entity has anywhere further to go. If it does
/// not, the graph itself is exhausted and [`PathTermination::NoPropagatingEdge`] supports
/// the stronger statement that nothing lies beyond. If it does, the pass stopped at a
/// limit: [`PathTermination::MaxDepthReached`] when the limit is the one-hop scope, and
/// [`PathTermination::TargetReached`] when a wider analysis is asking only for the first
/// level of a route it could have followed further.
///
/// `max_depth` is a parameter rather than a constant so that the one-hop pass and a
/// bounded propagation agree: the conformance test in `tests/` requires them to report
/// the same entities *and* the same termination for the same bound.
#[must_use]
pub fn termination_for(
    context: &ImpactContext<'_>,
    entity: &EntityRef,
    max_depth: usize,
) -> PathTermination {
    if context.steps_from(entity).is_empty() {
        PathTermination::NoPropagatingEdge
    } else if max_depth <= 1 {
        PathTermination::MaxDepthReached
    } else {
        PathTermination::TargetReached
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use amasario_core::{
        Basis, ConfidenceLevel, DependencyClass, Digest, EntityKind, Relationship,
    };
    use amasario_dependency::{Dependency, EvidenceRef};
    use amasario_graph::Graph;

    fn entity(kind: EntityKind, id: &str) -> EntityRef {
        EntityRef::new(kind, id).expect("a non-empty identifier")
    }

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
            confidence: amasario_core::Confidence::new(level, vec!["a".repeat(64)], Vec::new())
                .expect("a confidence"),
            verification: amasario_core::VerificationStatus::Verified,
            evidence: vec![
                EvidenceRef::new(amasario_core::EvidenceType::Transaction, "a".repeat(64))
                    .expect("a citation"),
            ],
            path: Vec::new(),
            depth: 0,
            reason: "observed".to_owned(),
            observed_at: None,
        }
    }

    fn graph(edges: Vec<Dependency>) -> Graph {
        let mut graph = Graph::new("g").expect("a graph");
        for edge in edges {
            graph.add_edge(edge).expect("an edge");
        }
        graph.canonicalise();
        graph
    }

    #[test]
    fn a_dependency_produces_a_finding_against_every_dependent() {
        let dependency = entity(EntityKind::Contract, "C-dependency");
        let dependent = entity(EntityKind::Contract, "C-dependent");
        let graph = graph(vec![edge(
            &dependent,
            &dependency,
            Relationship::DependsOn,
            ConfidenceLevel::HighConfidence,
        )]);
        let context = ImpactContext::new(&graph);
        let findings = findings(&context, &dependency, Some(ChangeType::Modified), 4);
        assert_eq!(findings.len(), 1, "one dependent is one finding");
        let finding = &findings[0];
        assert_eq!(finding.affected_entity, dependent);
        assert_eq!(finding.changed_entity, dependency);
        assert!(finding.is_direct());
        assert_eq!(finding.hop_depth, 1);
        assert_eq!(finding.path.as_ref().expect("a path").nodes.len(), 2);
        assert_eq!(finding.path.as_ref().expect("a path").steps.len(), 1);
        assert_eq!(finding.change_type, Some(ChangeType::Modified));
        assert!(finding.is_valid(), "{:?}", finding.failures());
        assert!(
            finding.reason.contains("reaches") && finding.reason.contains("1 hop"),
            "the reason must read as a route: {}",
            finding.reason
        );
    }

    #[test]
    fn a_change_to_a_dependent_reaches_nothing_upstream() {
        let dependency = entity(EntityKind::Contract, "C-dependency");
        let dependent = entity(EntityKind::Contract, "C-dependent");
        // `C-dependent DEPENDS_ON C-dependency`.
        let graph = graph(vec![edge(
            &dependent,
            &dependency,
            Relationship::DependsOn,
            ConfidenceLevel::HighConfidence,
        )]);
        let context = ImpactContext::new(&graph);
        // The dependent changed. Its dependency is upstream, so nothing propagates: a
        // change to something that uses a library does not change the library.
        assert!(
            findings(&context, &dependent, None, 4).is_empty(),
            "a change to the dependent reaches nothing upstream"
        );
        // The dependency changed. The dependent is reached, against the arrow.
        let findings = findings(&context, &dependency, None, 4);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].affected_entity, dependent);
        assert_eq!(
            findings[0].path.as_ref().expect("a path").steps[0].direction,
            crate::propagation::StepDirection::Inverted,
            "the change travelled against the arrow, and the step must say so"
        );
    }

    #[test]
    fn an_entity_reachable_only_through_an_excluded_endpoint_is_not_reached() {
        // `C-outer DEPENDS_ON C-inner`. A change to `C-inner` normally reaches
        // `C-outer`, but a deployment that never took effect would have installed a
        // different executable at that address, so the whole branch is out of scope.
        let inner = entity(EntityKind::Contract, "C-inner");
        let outer = entity(EntityKind::Contract, "C-outer");
        let graph = graph(vec![edge(
            &outer,
            &inner,
            Relationship::DependsOn,
            ConfidenceLevel::Verified,
        )]);
        let context = ImpactContext::new(&graph);
        assert_eq!(
            findings(&context, &inner, None, 1).len(),
            1,
            "without the exclusion the dependent is reached"
        );
        let context = context.excluding(inner.clone());
        assert!(context.excludes(&inner));
        assert!(
            findings(&context, &inner, None, 1).is_empty(),
            "an excluded entity is not traversed into"
        );
    }

    #[test]
    fn the_termination_distinguishes_a_bound_from_an_exhausted_graph() {
        // `C-outer DEPENDS_ON C-middle DEPENDS_ON C-leaf`. A change to the deepest
        // dependency travels up the chain, so the top of it has nothing beyond while the
        // middle of it does.
        let outer = entity(EntityKind::Contract, "C-outer");
        let middle = entity(EntityKind::Contract, "C-middle");
        let leaf = entity(EntityKind::Contract, "C-leaf");
        let graph = graph(vec![
            edge(
                &outer,
                &middle,
                Relationship::DependsOn,
                ConfidenceLevel::Verified,
            ),
            edge(
                &middle,
                &leaf,
                Relationship::DependsOn,
                ConfidenceLevel::Verified,
            ),
        ]);
        let context = ImpactContext::new(&graph);
        assert_eq!(
            termination_for(&context, &outer, 1),
            PathTermination::NoPropagatingEdge,
            "nothing depends on the outer contract, so nothing lies beyond it"
        );
        assert_eq!(
            termination_for(&context, &leaf, 1),
            PathTermination::MaxDepthReached,
            "the dependency reaches its dependent, so a one-hop pass stopped at its bound"
        );
        assert_eq!(
            termination_for(&context, &leaf, 4),
            PathTermination::TargetReached,
            "a wider analysis stopped because it only wanted the first level"
        );
        // The exhausted case is a conclusive negative result: the outer contract changed
        // and nothing depends on it, so nothing is affected.
        let exhausted = findings(&context, &outer, None, 1);
        assert!(exhausted.is_empty());
        // The bounded case is not: the pass stopped before it could go further, and the
        // finding says so.
        let bounded = findings(&context, &leaf, None, 1);
        assert_eq!(bounded.len(), 1);
        assert_eq!(bounded[0].affected_entity, middle);
        assert_eq!(bounded[0].truncated, Some(true));
        assert!(!bounded[0].path.as_ref().expect("a path").is_complete());
    }

    #[test]
    fn an_affected_executable_is_classified_as_an_artifact_finding() {
        let artifact = entity(EntityKind::Artifact, &"cd".repeat(32));
        let wasm = entity(EntityKind::Wasm, &"ef".repeat(32));
        // `WASM DERIVED_FROM ARTIFACT` propagates object-to-subject, so a change to the
        // artifact reaches the executable derived from it.
        let graph = graph(vec![edge(
            &wasm,
            &artifact,
            Relationship::DerivedFrom,
            ConfidenceLevel::HighConfidence,
        )]);
        let context = ImpactContext::new(&graph);
        let findings = findings(&context, &artifact, None, 4);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].affected_entity, wasm);
        assert!(findings[0].has(crate::affected::ImpactType::Artifact));
        assert!(!findings[0].has(crate::affected::ImpactType::Contract));
    }

    #[test]
    fn a_finding_records_the_boundary_the_graph_was_assembled_at() {
        let dependency = entity(EntityKind::Contract, "C-dependency");
        let dependent = entity(EntityKind::Contract, "C-dependent");
        let mut graph = graph(vec![edge(
            &dependent,
            &dependency,
            Relationship::DependsOn,
            ConfidenceLevel::HighConfidence,
        )]);
        graph.boundary = Some(amasario_core::ObservationBoundary::new(
            amasario_core::Network::new(
                "testnet",
                amasario_core::NetworkType::Testnet,
                "Test SDF Network ; September 2015",
            )
            .expect("a network"),
            amasario_core::LedgerSequence::new(1_000).expect("a ledger"),
            "2026-01-01T00:00:00Z",
        ));
        let context = ImpactContext::new(&graph);
        let findings = findings(&context, &dependency, None, 4);
        assert_eq!(findings.len(), 1);
        assert!(
            findings[0].boundary.is_some(),
            "a finding is a fact about a boundary and must say which"
        );
        assert_eq!(
            findings[0]
                .boundary
                .as_ref()
                .expect("a boundary")
                .ledger
                .get(),
            1_000
        );
    }

    #[test]
    fn the_finding_cites_exactly_what_its_steps_cite() {
        let dependency = entity(EntityKind::Contract, "C-dependency");
        let dependent = entity(EntityKind::Contract, "C-dependent");
        let graph = graph(vec![edge(
            &dependent,
            &dependency,
            Relationship::DependsOn,
            ConfidenceLevel::HighConfidence,
        )]);
        let context = ImpactContext::new(&graph);
        let findings = findings(&context, &dependency, None, 4);
        let finding = &findings[0];
        let step_citations = path_evidence(&finding.path.as_ref().expect("a path").steps);
        assert_eq!(finding.evidence, step_citations);
        assert_eq!(finding.evidence.len(), 1);
        assert_eq!(
            finding.evidence[0].kind,
            amasario_core::EvidenceType::Transaction
        );
        let _ = Digest::sha256_of(b"x");
    }
}
