//! Impact analysis: what a change reaches, and on what evidence.
//!
//! # What this crate answers
//!
//! Given a typed provenance graph and a change to one entity, which other entities could
//! be affected, how far away are they, along which route, and what supports each step of
//! that route. It is the third question Amasario formalizes - after "what does this
//! contract depend on" and "where did its artifact come from" - and it is the one whose
//! answer is most easily overstated, which is why the whole crate is built around
//! refusing to overstate it.
//!
//! # The four things a finding is not allowed to do
//!
//! Each is a rule from `rules/impact/`, and each is enforced in code rather than
//! documented and hoped for:
//!
//! 1. **It cannot invent a direction.** `taxonomies/relationship-types.yaml` declares a
//!    `changePropagation` for every relationship and states that impact "is derived from
//!    `changePropagation`; it is never inferred from the name of the relationship."
//!    [`propagation`] is that derivation. It matters because `DEPENDS_ON` reads
//!    subject-first while a change to its *object* is what reaches its subject: a
//!    name-based implementation propagates the wrong way, and would report that changing
//!    a dependent breaks its dependency.
//! 2. **It cannot traverse a non-propagating relationship.** `OBSERVED_IN` and
//!    `VERIFIED_BY` have `changePropagation: none`, and
//!    [`ImpactFailure::NonPropagatingStep`] makes traversing one a named refusal rather
//!    than a silent skip, so an empty result cannot hide a declined question.
//! 3. **It cannot manufacture confidence.** A path's aggregate is the minimum ordinal
//!    across its steps, and [`ImpactFailure::ConfidenceNotWeakestLink`] refuses a finding
//!    whose aggregate is stronger than its weakest step.
//! 4. **It cannot hide its path.** A finding at any non-zero depth carries the route it
//!    took, with per-step evidence, so a reader who disagrees can name the hop they
//!    dispute instead of rejecting a number.
//!
//! # What a finding is
//!
//! [`ImpactFinding`] is the specification's record: the changed entity, the affected
//! entity, the path, the hop count, the relationship types in path order, the evidence,
//! the confidence, the reason, the change type and the verification state. Its
//! classifications are *derived* - distance terms from the hop depth, an entity term from
//! the affected entity's kind, `CHANGE` from whether a change type was supplied - so a
//! producer cannot attach a term the depth does not support.
//!
//! # The sentence that must not be forgotten
//!
//! `rules/impact/direct-impact.yaml` states the non-goal plainly: "This rule does not
//! claim the affected entity is broken, only that a change may reach it." An affected
//! entity is a claim about correspondence, never about correctness, safety or
//! vulnerability. Nothing in this crate computes or implies the second kind of claim, and
//! `SECURITY.md` says why the engine refuses to.
//!
//! # The smallest useful run
//!
//! ```
//! use amasario_core::{
//!     Basis, Confidence, ConfidenceLevel, DependencyClass, EntityKind, EntityRef, Relationship,
//!     VerificationStatus,
//! };
//! use amasario_dependency::{Dependency, EvidenceRef, Limits};
//! use amasario_graph::Graph;
//! use amasario_impact::{ChangeType, ImpactContext, analyze};
//!
//! let dependency = EntityRef::new(EntityKind::Contract, "C-dependency")?;
//! let dependent = EntityRef::new(EntityKind::Contract, "C-dependent")?;
//! let transaction = "a".repeat(64);
//!
//! let mut graph = Graph::new("example")?;
//! graph.add_edge(Dependency {
//!     subject: dependent.clone(),
//!     object: dependency.clone(),
//!     relationship: Relationship::DependsOn,
//!     classes: vec![DependencyClass::Contract],
//!     basis: Basis::ObservedInvocation,
//!     confidence: Confidence::new(
//!         ConfidenceLevel::HighConfidence,
//!         vec![transaction.clone()],
//!         Vec::new(),
//!     )?,
//!     verification: VerificationStatus::Verified,
//!     evidence: vec![EvidenceRef::new(
//!         amasario_core::EvidenceType::Transaction,
//!         transaction,
//!     )?],
//!     path: Vec::new(),
//!     depth: 0,
//!     reason: "an observed cross-contract call".to_owned(),
//!     observed_at: None,
//! })?;
//! graph.canonicalise();
//!
//! let context = ImpactContext::new(&graph);
//! let analysis = analyze(
//!     &context,
//!     &dependency,
//!     Some(ChangeType::Replaced),
//!     Limits::new(4, 256)?,
//! )?;
//!
//! // A change to the dependency reaches the contract that depends on it, one hop away.
//! assert_eq!(analysis.len(), 1);
//! assert_eq!(analysis.affected_entities(), vec![&dependent]);
//! assert!(analysis.is_conclusive());
//! # Ok::<(), amasario_core::EngineError>(())
//! ```
//!
//! # Reading the result honestly
//!
//! [`ImpactAnalysis::is_conclusive`] is the property a consumer must check before
//! reporting an empty result as "nothing is affected". A bounded traversal that found
//! nothing and a complete traversal that found nothing produce the same finding list and
//! mean opposite things, which is why the bound travels with the analysis and not only
//! with a log line.

pub mod affected;
pub mod analyzer;
pub mod change;
pub mod direct;
pub mod errors;
pub mod propagation;
pub mod transitive;

// Re-exported together because they are used together: a caller building an analysis
// needs the context, the entry points, the finding model and the bounds, and having to
// know which module each lives in would be friction with no benefit.
pub use affected::{
    ChangeType, DeploymentRecord, ImpactFinding, ImpactPath, ImpactType, ImpactTypeFamily,
    PathTermination, aggregate_confidence, path_evidence, required_distance_terms,
    required_entity_term,
};
pub use analyzer::{
    ImpactAnalysis, analyze, analyze_changes, analyze_direct, canonical_order, context,
    excluded_deployments,
};
pub use change::{Change, ChangeSet};
pub use errors::{ImpactFailure, describe as describe_failure, first_failure};
pub use propagation::{
    ImpactContext, ImpactDirection, Step, StepDirection, combine_directions, describe_route,
    non_propagation_reason, propagates, propagates_backward, propagates_forward, step_direction,
};
pub use transitive::{Propagation, Reach, findings as transitive_findings, propagate};

// Re-exported so that a consumer does not have to depend on `amasario-provenance` merely
// to ask whether a deployment can be affected, which is an impact question.
pub use amasario_provenance::DeploymentStatus;

#[cfg(test)]
mod tests {
    use super::*;
    use amasario_core::{
        Basis, ConfidenceLevel, DependencyClass, EntityKind, EntityRef, Relationship,
    };
    use amasario_dependency::{Dependency, EvidenceRef, Limits};
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
    fn the_vocabulary_is_reachable_from_the_crate_root() {
        // A consumer should not have to know which module a concept lives in to name it.
        // A re-export that went missing would otherwise be a breaking change discovered
        // downstream rather than here.
        assert_eq!(ImpactType::Direct.as_str(), "DIRECT");
        assert_eq!(ImpactType::Transitive.family(), ImpactTypeFamily::Distance);
        assert_eq!(StepDirection::Inverted.as_str(), "inverted");
        assert_eq!(ImpactDirection::Dependents.as_str(), "DEPENDENTS");
        assert_eq!(
            PathTermination::NoPropagatingEdge.as_str(),
            "NO_PROPAGATING_EDGE"
        );
        assert_eq!(ChangeType::Replaced.as_str(), "REPLACED");
        assert_eq!(DeploymentStatus::Confirmed.as_str(), "CONFIRMED");
        assert!(
            describe_failure(&ImpactFailure::PathRepeatsEntity {
                entity: "C-a".to_owned(),
            })
            .contains("cycle")
        );
    }

    #[test]
    fn every_finding_in_an_analysis_is_structurally_valid() {
        let a = entity(EntityKind::Contract, "C-a");
        let b = entity(EntityKind::Contract, "C-b");
        let c = entity(EntityKind::Contract, "C-c");
        let graph = graph(vec![
            edge(&b, &a, Relationship::DependsOn, ConfidenceLevel::Verified),
            edge(
                &c,
                &b,
                Relationship::DependsOn,
                ConfidenceLevel::LowConfidence,
            ),
        ]);
        let context = ImpactContext::new(&graph);
        let analysis = analyze(
            &context,
            &a,
            Some(ChangeType::Modified),
            Limits::new(5, 128).expect("bounds"),
        )
        .expect("an analysis");
        analysis.validate().expect("every finding passes its rules");
        assert_eq!(analysis.len(), 2);
        // The deepest finding's aggregate is the weaker of its two links.
        let deep = analysis
            .findings
            .iter()
            .find(|finding| finding.hop_depth == 2)
            .expect("a two-hop finding");
        assert_eq!(deep.confidence.level, ConfidenceLevel::LowConfidence);
    }

    #[test]
    fn a_snapshot_of_an_analysis_is_deterministic() {
        // Two runs over one graph must produce the same findings in the same order, or a
        // snapshot diff would report a reordering as a change.
        let a = entity(EntityKind::Contract, "C-a");
        let b = entity(EntityKind::Contract, "C-b");
        let c = entity(EntityKind::Contract, "C-c");
        let graph = graph(vec![
            edge(&b, &a, Relationship::DependsOn, ConfidenceLevel::Verified),
            edge(&c, &a, Relationship::DependsOn, ConfidenceLevel::Verified),
        ]);
        let context = ImpactContext::new(&graph);
        let limits = Limits::new(4, 64).expect("bounds");
        let first = analyze(&context, &a, None, limits).expect("an analysis");
        let second = analyze(&context, &a, None, limits).expect("an analysis");
        assert_eq!(first, second);
        assert_eq!(
            first
                .findings
                .iter()
                .map(|finding| finding.id.as_str())
                .collect::<Vec<_>>(),
            second
                .findings
                .iter()
                .map(|finding| finding.id.as_str())
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn the_summary_states_how_many_entities_a_change_reaches() {
        let a = entity(EntityKind::Contract, "C-a");
        let b = entity(EntityKind::Contract, "C-b");
        let graph = graph(vec![edge(
            &b,
            &a,
            Relationship::DependsOn,
            ConfidenceLevel::Verified,
        )]);
        let context = ImpactContext::new(&graph);
        let analysis =
            analyze(&context, &a, None, Limits::new(3, 32).expect("bounds")).expect("an analysis");
        assert!(
            analysis
                .summary()
                .contains("affects 1 entity(ies): 1 directly, 0 transitively"),
            "got: {}",
            analysis.summary()
        );
    }
}
