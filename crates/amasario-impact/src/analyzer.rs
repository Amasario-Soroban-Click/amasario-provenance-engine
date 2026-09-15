//! The analysis entry point: given a graph and a change, what does it reach.
//!
//! # One traversal, two scopes
//!
//! There is exactly one propagation implementation, [`crate::transitive::propagate`],
//! and the analyzer chooses how far to let it run. [`analyze`] runs it to the caller's
//! bound; [`analyze_direct`] runs the one-hop pass, which exists because a caller that
//! wants only the immediate dependents of a large graph should not have to run a bounded
//! traversal and discard all but its first level. The two are required to agree on which
//! entities are affected at depth one, and the conformance test in `tests/` checks that
//! they do rather than leaving it to be assumed.
//!
//! # Why the deployments are an input
//!
//! `impact/deployment-impact` excludes `UNCONFIRMED`, `FAILED` and `UNKNOWN` deployments
//! from findings, and [`excluded_deployments`] turns a set of deployment records into
//! the exclusion list the traversal needs. The exclusion is applied during traversal
//! rather than to the result, for the reason the rule gives: a change can only affect a
//! deployment that took effect, so the executable a failed deployment would have
//! installed is not the one at that address and nothing behind it can be reached either.
//!
//! # Refusing to guess at the change
//!
//! [`analyze`] takes an optional change type and uses it only to classify the findings.
//! It does not synthesise the zero-hop finding, because that finding has to cite the
//! evidence that the change occurred and a bare change type carries none. A caller with
//! an actual change uses [`ChangeSet`], whose changes carry their evidence and whose
//! [`analyze_changes`] therefore produces the zero-hop finding as well.

use amasario_core::{EntityRef, ObservationBoundary, Result, TruncationReason};
use amasario_dependency::Limits;

use crate::affected::{ChangeType, DeploymentRecord, ImpactFinding, PathTermination};
use crate::change::ChangeSet;
use crate::direct;
use crate::errors::{ImpactFailure, first_failure};
use crate::propagation::ImpactContext;
use crate::transitive;

/// What an impact analysis produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImpactAnalysis {
    /// The entity the analysis started from.
    pub changed: EntityRef,
    /// The change type the findings were classified with, where one was supplied.
    pub change_type: Option<ChangeType>,
    /// The findings, in canonical order: by depth, then affected kind, then identifier.
    pub findings: Vec<ImpactFinding>,
    /// The bounds the analysis ran under.
    pub limits: Limits,
    /// The boundary the graph was assembled at.
    pub boundary: Option<ObservationBoundary>,
    /// Whether propagation stopped before exhausting what is reachable.
    pub truncated: bool,
    /// Why it stopped, when it did.
    pub truncation_reason: Option<TruncationReason>,
    /// How many entities were visited, including the changed one.
    pub nodes_visited: usize,
}

impl ImpactAnalysis {
    /// The findings for entities exactly one hop away.
    #[must_use]
    pub fn direct(&self) -> Vec<&ImpactFinding> {
        self.findings
            .iter()
            .filter(|finding| finding.is_direct())
            .collect()
    }

    /// The findings for entities two or more hops away.
    #[must_use]
    pub fn transitive(&self) -> Vec<&ImpactFinding> {
        self.findings
            .iter()
            .filter(|finding| finding.is_transitive())
            .collect()
    }

    /// The findings for entities three or more hops away.
    #[must_use]
    pub fn multi_hop(&self) -> Vec<&ImpactFinding> {
        self.findings
            .iter()
            .filter(|finding| finding.is_multi_hop())
            .collect()
    }

    /// The findings for entities at exactly one depth.
    #[must_use]
    pub fn at_depth(&self, depth: usize) -> Vec<&ImpactFinding> {
        self.findings
            .iter()
            .filter(|finding| finding.hop_depth == depth)
            .collect()
    }

    /// The findings against affected entities of one kind.
    #[must_use]
    pub fn affected_of_kind(&self, kind: amasario_core::EntityKind) -> Vec<&ImpactFinding> {
        self.findings
            .iter()
            .filter(|finding| finding.affected_entity.kind == kind)
            .collect()
    }

    /// The affected entities, in the order their findings appear.
    #[must_use]
    pub fn affected_entities(&self) -> Vec<&EntityRef> {
        self.findings
            .iter()
            .map(|finding| &finding.affected_entity)
            .collect()
    }

    /// The greatest hop depth any finding reached.
    #[must_use]
    pub fn deepest(&self) -> usize {
        self.findings
            .iter()
            .map(|finding| finding.hop_depth)
            .max()
            .unwrap_or(0)
    }

    /// How many findings the analysis produced.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.findings.len()
    }

    /// Whether the analysis produced no findings.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.findings.is_empty()
    }

    /// Whether an empty result is a negative finding rather than an inconclusive one.
    ///
    /// The single most important property of an analysis: "nothing depends on this" and
    /// "nothing was found before the budget ran out" lead to opposite decisions, and
    /// they are indistinguishable from the finding list alone.
    #[must_use]
    pub const fn is_conclusive(&self) -> bool {
        !self.truncated
    }

    /// Every way any finding fails a rule, all together.
    #[must_use]
    pub fn failures(&self) -> Vec<ImpactFailure> {
        let mut failures: Vec<ImpactFailure> = Vec::new();
        for (index, finding) in self.findings.iter().enumerate() {
            failures.extend(finding.failures());
            // Two findings with the same identifier in one analysis would make a
            // snapshot diff report a duplicate rather than a change, so the identifier's
            // uniqueness is checked where it is relied on.
            if self.findings[..index]
                .iter()
                .any(|earlier| earlier.id == finding.id)
            {
                failures.push(ImpactFailure::DuplicateFindingId {
                    id: finding.id.clone(),
                });
            }
        }
        failures
    }

    /// Validates every finding in the analysis.
    ///
    /// # Errors
    ///
    /// Returns the first failure.
    pub fn validate(&self) -> Result<()> {
        first_failure(self.failures())
    }

    /// A one-line summary, for a report header or a log entry.
    ///
    /// States the truncation because that is the fact a reader must not have to infer:
    /// "12 affected" and "12 affected, bounded at depth 3" are different statements.
    #[must_use]
    pub fn summary(&self) -> String {
        let direct = self.direct().len();
        let transitive = self.transitive().len();
        let bounded = match self.truncation_reason {
            Some(reason) if self.truncated => format!(", bounded because {}", reason.as_str()),
            _ => String::new(),
        };
        format!(
            "a change to {} affects {} entity(ies): {} directly, {} transitively{}",
            self.changed,
            self.len(),
            direct,
            transitive,
            bounded
        )
    }
}

/// The deployments that must not be traversed into.
///
/// A deployment that has not been established as having taken effect is excluded for two
/// reasons, and the second is the one that makes this a traversal input rather than a
/// result filter: the deployment itself cannot be affected, and neither can the entities
/// that would sit behind it, because the executable at that address is not the one the
/// failed deployment would have installed.
#[must_use]
pub fn excluded_deployments(deployments: &[DeploymentRecord]) -> Vec<EntityRef> {
    let mut excluded: Vec<EntityRef> = deployments
        .iter()
        .filter(|record| !record.is_eligible())
        .map(|record| record.entity.clone())
        .collect();
    excluded.sort_by(|left, right| {
        left.kind
            .cmp(&right.kind)
            .then_with(|| left.id.cmp(&right.id))
    });
    excluded.dedup();
    excluded
}

/// Builds the traversal context for a graph and a set of deployment records.
#[must_use]
pub fn context<'a>(
    graph: &'a amasario_graph::Graph,
    deployments: &[DeploymentRecord],
) -> ImpactContext<'a> {
    let mut context = ImpactContext::new(graph);
    for entity in excluded_deployments(deployments) {
        context = context.excluding(entity);
    }
    context
}

/// Analyzes what a change to an entity reaches, within the bounds.
///
/// The findings combine the change's own zero-hop finding, when a change type is
/// supplied, with one finding per entity the bounded propagation reached.
///
/// # Errors
///
/// Returns a dependency error when the bounds are zero, which [`Limits::new`] already
/// rejects, or an impact error when a finding fails a rule. The second cannot arise from
/// this function, because every finding is derived rather than accepted; the check is
/// kept so that a defect in the derivation surfaces here rather than in a report.
pub fn analyze(
    context: &ImpactContext<'_>,
    changed: &EntityRef,
    change_type: Option<ChangeType>,
    limits: Limits,
) -> Result<ImpactAnalysis> {
    let propagation = transitive::propagate(context, changed, limits);
    let mut findings = propagation.findings(change_type);
    findings.sort_by(canonical_order);
    let analysis = ImpactAnalysis {
        changed: changed.clone(),
        change_type,
        findings,
        limits,
        boundary: context.boundary.clone(),
        truncated: propagation.truncated,
        truncation_reason: propagation.truncation_reason,
        nodes_visited: propagation.nodes_visited,
    };
    analysis.validate()?;
    Ok(analysis)
}

/// Analyzes only the entities one hop from a change.
///
/// # Errors
///
/// Returns an impact error when a finding fails a rule, which cannot arise from the
/// derivation here.
pub fn analyze_direct(
    context: &ImpactContext<'_>,
    changed: &EntityRef,
    change_type: Option<ChangeType>,
) -> Result<ImpactAnalysis> {
    let limits = Limits::new(1, usize::MAX)?;
    let mut findings = direct::findings(context, changed, change_type, 1);
    findings.sort_by(canonical_order);
    let truncated = findings.iter().any(|finding| {
        finding
            .path
            .as_ref()
            .and_then(|path| path.termination)
            .is_some_and(PathTermination::is_bound)
    });
    let analysis = ImpactAnalysis {
        changed: changed.clone(),
        change_type,
        findings,
        limits,
        boundary: context.boundary.clone(),
        truncated,
        truncation_reason: truncated.then_some(TruncationReason::MaxDepthReached),
        nodes_visited: 1,
    };
    analysis.validate()?;
    Ok(analysis)
}

/// Analyzes every change in a set, in canonical order.
///
/// Each change produces its own analysis, and each analysis begins with the change's
/// zero-hop finding. That finding is not padding: it is what lets a report say what it
/// was asked about, and what makes "nothing was affected" distinguishable from "nothing
/// was analysed".
///
/// # Errors
///
/// Returns the first failure, which can only arise from a change built outside
/// [`crate::change::Change::new`], or from a finding that fails a rule.
pub fn analyze_changes(
    context: &ImpactContext<'_>,
    set: &ChangeSet,
    limits: Limits,
) -> Result<Vec<ImpactAnalysis>> {
    let mut analyses = Vec::with_capacity(set.len());
    for entity in set.entities() {
        // The entity's changes share one traversal, because the graph does not depend on
        // why the analysis started there. The change type carried on the findings is the
        // first one in canonical order, and each change's own zero-hop finding carries
        // its own.
        let changes = set.for_entity(&entity);
        let change_type = changes.first().map(|change| change.change_type);
        let mut analysis = analyze(context, &entity, change_type, limits)?;
        for change in changes {
            let finding = change.finding()?;
            if !analysis
                .findings
                .iter()
                .any(|existing| existing.id == finding.id)
            {
                analysis.findings.push(finding);
            }
        }
        analysis.findings.sort_by(canonical_order);
        analysis.validate()?;
        analyses.push(analysis);
    }
    Ok(analyses)
}

/// The canonical order for a finding set.
///
/// By hop depth, then affected kind, then affected identifier, then identifier. Stated
/// as a function rather than left to each call site, because two runs over one graph must
/// produce the same bytes and an order that depended on discovery would not.
pub fn canonical_order(left: &ImpactFinding, right: &ImpactFinding) -> std::cmp::Ordering {
    left.hop_depth
        .cmp(&right.hop_depth)
        .then_with(|| left.affected_entity.kind.cmp(&right.affected_entity.kind))
        .then_with(|| left.affected_entity.id.cmp(&right.affected_entity.id))
        .then_with(|| left.change_type.cmp(&right.change_type))
        .then_with(|| left.id.cmp(&right.id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use amasario_core::{
        Basis, Confidence, ConfidenceLevel, DependencyClass, EntityKind, LedgerSequence, Network,
        NetworkType, Relationship,
    };
    use amasario_dependency::{Dependency, EvidenceRef};
    use amasario_graph::Graph;
    use amasario_provenance::DeploymentStatus;

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
            confidence: Confidence::new(level, vec!["a".repeat(64)], Vec::new())
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

    fn chain() -> Graph {
        let a = entity(EntityKind::Contract, "C-a");
        let b = entity(EntityKind::Contract, "C-b");
        let c = entity(EntityKind::Contract, "C-c");
        graph(vec![
            edge(&b, &a, Relationship::DependsOn, ConfidenceLevel::Verified),
            edge(&c, &b, Relationship::DependsOn, ConfidenceLevel::Verified),
        ])
    }

    fn limits(depth: usize) -> Limits {
        Limits::new(depth, 512).expect("bounded limits")
    }

    fn ledger(value: u32) -> LedgerSequence {
        LedgerSequence::new(value).expect("a real ledger")
    }

    #[test]
    fn an_analysis_separates_direct_from_transitive_findings() {
        let graph = chain();
        let context = ImpactContext::new(&graph);
        let analysis = analyze(
            &context,
            &entity(EntityKind::Contract, "C-a"),
            Some(ChangeType::Modified),
            limits(6),
        )
        .expect("an analysis");
        assert_eq!(analysis.len(), 2);
        assert_eq!(analysis.direct().len(), 1);
        assert_eq!(analysis.transitive().len(), 1);
        assert!(analysis.multi_hop().is_empty());
        assert_eq!(analysis.deepest(), 2);
        assert!(analysis.is_conclusive());
        assert!(analysis.summary().contains("1 directly, 1 transitively"));
        // Canonical order: the nearer entity first.
        assert_eq!(
            analysis.affected_entities(),
            vec![
                &entity(EntityKind::Contract, "C-b"),
                &entity(EntityKind::Contract, "C-c")
            ]
        );
    }

    #[test]
    fn the_one_hop_pass_agrees_with_a_one_hop_bound() {
        // Two implementations of the same question must not disagree, so the agreement
        // is checked rather than assumed.
        let graph = chain();
        let context = ImpactContext::new(&graph);
        let changed = entity(EntityKind::Contract, "C-a");
        let direct_only = analyze_direct(&context, &changed, None).expect("a direct analysis");
        let bounded = analyze(&context, &changed, None, limits(1)).expect("a bounded analysis");
        let mut direct_entities: Vec<&str> = direct_only
            .affected_entities()
            .into_iter()
            .map(|entity| entity.id.as_str())
            .collect();
        let mut bounded_entities: Vec<&str> = bounded
            .affected_entities()
            .into_iter()
            .map(|entity| entity.id.as_str())
            .collect();
        direct_entities.sort_unstable();
        bounded_entities.sort_unstable();
        assert_eq!(direct_entities, bounded_entities);
        assert_eq!(direct_only.len(), 1);
        // Both report the bound, which is what makes their emptiness conclusive in
        // neither case.
        assert!(direct_only.truncated);
        assert!(bounded.truncated);
        assert_eq!(
            direct_only.truncation_reason,
            Some(TruncationReason::MaxDepthReached)
        );
    }

    #[test]
    fn an_ineligible_deployment_is_excluded_from_traversal() {
        let deployment = entity(EntityKind::Deployment, "D-1");
        let contract = entity(EntityKind::Contract, "C-a");
        let graph = graph(vec![edge(
            &contract,
            &deployment,
            Relationship::DependsOn,
            ConfidenceLevel::Verified,
        )]);
        let failed =
            DeploymentRecord::new(deployment.clone(), ledger(1_000), DeploymentStatus::Failed)
                .expect("a deployment record");
        let context = context(&graph, std::slice::from_ref(&failed));
        assert_eq!(
            excluded_deployments(std::slice::from_ref(&failed)),
            vec![deployment.clone()]
        );
        let analysis = analyze(&context, &deployment, None, limits(4)).expect("an analysis");
        assert!(
            analysis.is_empty(),
            "a change cannot reach a deployment that never took effect"
        );
        assert!(
            analysis.is_conclusive(),
            "the exclusion is a decision, not a budget"
        );
        // The same graph without the record does reach the dependent, which is what
        // makes the exclusion the reason for the empty result.
        let open = ImpactContext::new(&graph);
        let analysis = analyze(&open, &deployment, None, limits(4)).expect("an analysis");
        assert_eq!(analysis.len(), 1);
        assert_eq!(analysis.affected_entities(), vec![&contract]);
    }

    #[test]
    fn an_eligible_deployment_remains_reachable() {
        let deployment = entity(EntityKind::Deployment, "D-1");
        let contract = entity(EntityKind::Contract, "C-a");
        let graph = graph(vec![edge(
            &contract,
            &deployment,
            Relationship::DependsOn,
            ConfidenceLevel::Verified,
        )]);
        let confirmed = DeploymentRecord::new(
            deployment.clone(),
            ledger(1_000),
            DeploymentStatus::Confirmed,
        )
        .expect("a deployment record");
        let context = context(&graph, &[confirmed]);
        let analysis = analyze(&context, &deployment, None, limits(4)).expect("an analysis");
        assert_eq!(analysis.len(), 1);
        assert_eq!(analysis.affected_entities(), vec![&contract]);
    }

    #[test]
    fn a_change_set_produces_the_zero_hop_finding_and_one_traversal_per_entity() {
        let graph = chain();
        let context = ImpactContext::new(&graph);
        let set = ChangeSet::new(vec![
            crate::change::Change::new(
                entity(EntityKind::Contract, "C-a"),
                ChangeType::Replaced,
                "the contract's executable was replaced on testnet",
                vec![
                    EvidenceRef::new(amasario_core::EvidenceType::Transaction, "b".repeat(64))
                        .expect("a citation"),
                ],
            )
            .expect("a change"),
        ]);
        let analyses = analyze_changes(&context, &set, limits(6)).expect("analyses");
        assert_eq!(analyses.len(), 1, "one entity is one traversal");
        let analysis = &analyses[0];
        assert_eq!(
            analysis.len(),
            3,
            "the zero-hop change finding plus the two reached entities"
        );
        let zero_hop = analysis.at_depth(0);
        assert_eq!(zero_hop.len(), 1);
        assert_eq!(zero_hop[0].changed_entity, zero_hop[0].affected_entity);
        assert!(zero_hop[0].has(crate::affected::ImpactType::Change));
        analysis.validate().expect("every finding is valid");
    }

    #[test]
    fn an_analysis_with_no_reachable_entity_is_a_conclusive_negative_result() {
        let a = entity(EntityKind::Contract, "C-a");
        let b = entity(EntityKind::Contract, "C-b");
        let graph = graph(vec![edge(
            &a,
            &b,
            Relationship::DependsOn,
            ConfidenceLevel::Verified,
        )]);
        let context = ImpactContext::new(&graph);
        let analysis = analyze(&context, &a, None, limits(4)).expect("an analysis");
        assert!(analysis.is_empty());
        assert!(analysis.is_conclusive());
        assert!(analysis.summary().contains("affects 0 entity(ies)"));
        assert_eq!(analysis.deepest(), 0);
    }

    #[test]
    fn a_bounded_analysis_states_its_bound_in_its_summary() {
        let graph = chain();
        let context = ImpactContext::new(&graph);
        let analysis = analyze(
            &context,
            &entity(EntityKind::Contract, "C-a"),
            None,
            limits(1),
        )
        .expect("an analysis");
        assert!(analysis.truncated);
        assert!(!analysis.is_conclusive());
        assert!(
            analysis
                .summary()
                .contains("bounded because MAX_DEPTH_REACHED"),
            "got: {}",
            analysis.summary()
        );
    }

    #[test]
    fn findings_of_one_kind_can_be_asked_for_together() {
        let a = entity(EntityKind::Contract, "C-a");
        let b = entity(EntityKind::Contract, "C-b");
        let wasm = entity(EntityKind::Wasm, &"cd".repeat(32));
        let graph = graph(vec![
            edge(&b, &a, Relationship::DependsOn, ConfidenceLevel::Verified),
            edge(
                &b,
                &wasm,
                Relationship::DependsOn,
                ConfidenceLevel::Verified,
            ),
        ]);
        let context = ImpactContext::new(&graph);
        // A change to the executable reaches the contract that depends on it, because
        // `DEPENDS_ON` propagates object-to-subject.
        let analysis = analyze(&context, &wasm, None, limits(4)).expect("an analysis");
        assert_eq!(analysis.affected_of_kind(EntityKind::Contract).len(), 1);
        assert!(
            analysis.affected_of_kind(EntityKind::Wasm).is_empty(),
            "a change to the executable does not reach another executable"
        );
    }

    #[test]
    fn the_context_carries_the_boundary_the_graph_was_assembled_at() {
        let mut graph = chain();
        graph.boundary = Some(ObservationBoundary::new(
            Network::new(
                "testnet",
                NetworkType::Testnet,
                "Test SDF Network ; September 2015",
            )
            .expect("a network"),
            ledger(1_000),
            "2026-01-01T00:00:00Z",
        ));
        let context = ImpactContext::new(&graph);
        let analysis = analyze(
            &context,
            &entity(EntityKind::Contract, "C-a"),
            None,
            limits(4),
        )
        .expect("an analysis");
        assert_eq!(
            analysis.boundary.as_ref().expect("a boundary").ledger.get(),
            1_000
        );
        for finding in &analysis.findings {
            assert!(
                finding.boundary.is_some(),
                "every finding is a fact about a boundary"
            );
        }
    }
}
