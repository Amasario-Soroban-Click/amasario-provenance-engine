//! The direct partition: dependencies one edge from the subject.
//!
//! `dependency/direct-dependency` requires every dependency to state how it was
//! established and to reference at least one evidence record, and forbids asserting
//! one from a shared name, a mutual mention, a shared ecosystem, a shared operator or
//! similar metadata. All five of those are refused by
//! [`crate::classifier::classify`] before a candidate can reach this partition, and
//! this module's job is the smaller, sharper one: to name which dependencies are one
//! edge away and to say how many of them were established rather than refused.
//!
//! # Why the summary counts refusals separately
//!
//! A count of direct dependencies answers a different question from a count of
//! observations. If three cross-contract calls were observed and one was reverted,
//! the honest statement is "two direct dependencies, one refused" - not "two
//! dependencies", which reads as though nothing was seen, and not "three", which
//! reports an attempt as a fact.

use amasario_core::{DependencyClass, EntityRef, Result};

use crate::resolver::{Dependency, DependencySet};

/// The dependencies one edge from the subject, with the `DIRECT` class added.
///
/// Returns clones rather than references because the class is added here: a
/// dependency's partition is a property of where it sits in the set, not of the
/// observation that produced it, and recording `DIRECT` on the edge itself would mean
/// the same edge carried a different class in two different sets.
#[must_use]
pub fn direct_dependencies(set: &DependencySet) -> Vec<Dependency> {
    set.direct
        .iter()
        .cloned()
        .map(|dependency| dependency.with_class(DependencyClass::Direct))
        .collect()
}

/// Whether a dependency sits in the direct partition.
///
/// Depth, not class membership, is the authority: a set assembled by hand can carry
/// the wrong class, and the depth is what traversal actually did.
#[must_use]
pub const fn is_direct(dependency: &Dependency) -> bool {
    dependency.depth == 0
}

/// What the direct partition contains.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DirectSummary {
    /// How many direct dependencies were established.
    pub edges: usize,
    /// How many of them may be reported among the observed facts.
    pub observed: usize,
    /// How many rest on an inference rather than an observation.
    pub inferred: usize,
    /// How many candidates were refused, and so are not dependencies at all.
    pub refused: usize,
}

impl DirectSummary {
    /// Whether any refusal occurred.
    ///
    /// A caller uses this to decide whether the analysis was clean. A non-zero count
    /// is not an error: it means something was seen and could not be established,
    /// which is exactly what the report has to say out loud.
    #[must_use]
    pub const fn has_refusals(&self) -> bool {
        self.refused > 0
    }
}

impl std::fmt::Display for DirectSummary {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} direct ({} observed, {} inferred), {} refused",
            self.edges, self.observed, self.inferred, self.refused
        )
    }
}

/// Summarises the direct partition, counting refusals separately from findings.
#[must_use]
pub fn summarise_direct(set: &DependencySet) -> DirectSummary {
    let direct = direct_dependencies(set);
    let observed = direct
        .iter()
        .filter(|dependency| dependency.is_observed())
        .count();
    DirectSummary {
        edges: direct.len(),
        observed,
        inferred: direct.len() - observed,
        refused: set.unestablished.len(),
    }
}

/// The entities the subject depends on directly, in a deterministic order.
#[must_use]
pub fn direct_targets(set: &DependencySet) -> Vec<&EntityRef> {
    set.direct
        .iter()
        .map(|dependency| &dependency.object)
        .collect()
}

/// Every direct dependency that may be reported among the observed facts.
///
/// The gate `dependency/contract-dependency` names: an inferred relationship must not
/// appear there, which is why this is a function rather than a filter a caller writes.
#[must_use]
pub fn observed_direct(set: &DependencySet) -> Vec<Dependency> {
    direct_dependencies(set)
        .into_iter()
        .filter(Dependency::is_observed)
        .collect()
}

/// Checks that every direct dependency in the set agrees with its partition.
///
/// # Errors
///
/// Returns a dependency error when a dependency in the direct partition carries a
/// depth other than zero, which would mean the set was assembled rather than
/// traversed.
pub fn assert_partition_is_sound(set: &DependencySet) -> Result<()> {
    for dependency in &set.direct {
        if dependency.depth != 0 {
            return Err(amasario_core::EngineError::Dependency(format!(
                "the direct partition holds an edge to {} at depth {}; a direct dependency is one \
                 edge from the subject",
                dependency.object.id, dependency.depth
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::classifier::{Candidate, EvidenceRef};
    use amasario_core::{
        Basis, EntityKind, EntityRef as Ref, EvidenceType, LedgerSequence, Network, NetworkType,
        ObservationBoundary, Relationship,
    };

    fn boundary() -> ObservationBoundary {
        ObservationBoundary {
            network: Network::new(
                "testnet",
                NetworkType::Testnet,
                "Test SDF Network ; September 2015",
            )
            .expect("a network"),
            ledger: LedgerSequence::new(4_000).expect("a ledger"),
            observed_at: "2026-09-15T00:00:00Z".to_owned(),
            spec_version: None,
        }
    }

    fn invocation(callee: &str, successful: Option<bool>) -> Candidate {
        Candidate::new(
            Ref::new(EntityKind::Contract, "C-subject").expect("a reference"),
            Ref::new(EntityKind::Contract, callee).expect("a reference"),
            Relationship::Invocates,
            Basis::ObservedInvocation,
            vec![EvidenceRef::new(EvidenceType::Transaction, "a".repeat(64)).expect("a citation")],
        )
        .expect("a candidate")
        .observed_at(boundary())
        .with_outcome(successful)
    }

    fn set(candidates: &[Candidate]) -> DependencySet {
        crate::resolver::resolve(
            Ref::new(EntityKind::Contract, "C-subject").expect("a reference"),
            Some(boundary()),
            candidates,
            5,
        )
        .expect("resolves")
    }

    #[test]
    fn direct_dependencies_carry_the_direct_class_and_no_others_are_returned() {
        let resolved = set(&[invocation("C-a", Some(true))]);
        let direct = direct_dependencies(&resolved);
        assert_eq!(direct.len(), 1);
        assert!(direct[0].classes.contains(&DependencyClass::Direct));
        assert!(is_direct(&direct[0]));
        let canonical: Vec<DependencyClass> = DependencyClass::all()
            .iter()
            .copied()
            .filter(|class| direct[0].classes.contains(class))
            .collect();
        assert_eq!(direct[0].classes, canonical);
    }

    #[test]
    fn the_summary_counts_a_refusal_separately_from_a_finding() {
        // The distinction the summary exists for: a reverted call was observed and
        // could not be established, which is not the same as nothing being seen.
        let resolved = set(&[
            invocation("C-a", Some(true)),
            invocation("C-b", Some(false)),
        ]);
        let summary = summarise_direct(&resolved);
        assert_eq!(summary.edges, 1);
        assert_eq!(summary.observed, 1);
        assert_eq!(summary.inferred, 0);
        assert_eq!(summary.refused, 1);
        assert!(summary.has_refusals());
        assert!(summary.to_string().contains("1 refused"));
        assert_eq!(direct_targets(&resolved).len(), 1);
    }

    #[test]
    fn an_inferred_dependency_is_counted_but_is_not_an_observed_fact() {
        // The citation is the artifact whose interface was compared: a CONTRACT class
        // accepts transaction, event, source or artifact evidence, and an Observation
        // citation is not one of them.
        let inferred = Candidate::new(
            Ref::new(EntityKind::Contract, "C-subject").expect("a reference"),
            Ref::new(EntityKind::Contract, "C-similar").expect("a reference"),
            Relationship::DependsOn,
            Basis::InferredInterface,
            vec![EvidenceRef::new(EvidenceType::Artifact, "artifact-1").expect("a citation")],
        )
        .expect("a candidate")
        .observed_at(boundary());
        let resolved = set(&[inferred]);
        let summary = summarise_direct(&resolved);
        assert_eq!(summary.edges, 1);
        assert_eq!(summary.inferred, 1);
        assert_eq!(summary.observed, 0);
        assert!(observed_direct(&resolved).is_empty());
        assert_eq!(direct_dependencies(&resolved).len(), 1);
    }

    #[test]
    fn a_partition_assembled_by_hand_is_refused() {
        let mut resolved = set(&[invocation("C-a", Some(true))]);
        resolved.direct[0].depth = 3;
        let error = assert_partition_is_sound(&resolved).expect_err("depth must agree");
        assert!(error.to_string().contains("one edge from the subject"));
    }

    #[test]
    fn an_empty_partition_summarises_to_zero_rather_than_to_silence() {
        let resolved = set(&[]);
        let summary = summarise_direct(&resolved);
        assert_eq!(summary.edges, 0);
        assert_eq!(summary.refused, 0);
        assert!(!summary.has_refusals());
        assert!(observed_direct(&resolved).is_empty());
    }
}
