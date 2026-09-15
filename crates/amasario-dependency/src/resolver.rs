//! From candidates to a dependency set, and the checks the set has to satisfy.
//!
//! # Aggregation is conservative in both dimensions
//!
//! Two observations of the same edge are merged, and every choice the merge makes
//! lowers the claim rather than raising it:
//!
//! * the **basis** kept is the strongest one seen, because the strongest basis is
//!   what the observation actually established;
//! * the **confidence** is the weakest of the merged levels, because a chain is only
//!   as strong as its weakest link and the same holds for two readings of one edge;
//! * the **verification status** is the weakest of the merged statuses, by a rule
//!   this module states rather than inheriting: `CONFLICTING` wins over everything, an
//!   edge is `VERIFIED` only when every observation of it was, and anything else is
//!   `UNVERIFIED`.
//!
//! That last rule is deliberately not [`amasario_core::VerificationStatus::combine`].
//! `combine` merges the *components of one claim*, where "some components checked out"
//! is correctly `PARTIALLY_VERIFIED`. Merging two observations of one edge is a
//! different question, and answering it with `combine` would report an edge as
//! partially verified when one reading was verified and the other was not checked at
//! all - which is not a partial check of anything.
//!
//! # Refusals are recorded, not dropped
//!
//! A candidate the classifier refuses becomes an [`Unestablished`] entry carrying the
//! reason. Dropping it would make a refused dependency indistinguishable from one that
//! was never observed, and the whole point of the refusal is that it is informative.

use amasario_core::{
    Basis, Confidence, ConfidenceLevel, DependencyClass, EntityRef, Network, ObservationBoundary,
    Relationship, Result, TruncationReason, VerificationStatus,
};
use serde::{Deserialize, Serialize};

use crate::classifier::{Candidate, Classification, EvidenceRef, classify};

/// A dependency the specification allows to be stated.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Dependency {
    /// The entity that depends.
    pub subject: EntityRef,
    /// The entity depended upon.
    pub object: EntityRef,
    /// The relationship between them.
    pub relationship: Relationship,
    /// The classes the evidence supports, in canonical order. Never empty.
    pub classes: Vec<DependencyClass>,
    /// How the dependency was established.
    pub basis: Basis,
    /// How strongly the evidence supports it, with the citations.
    pub confidence: Confidence,
    /// What the evidence says about it.
    pub verification: VerificationStatus,
    /// The evidence that supports it. Never empty.
    pub evidence: Vec<EvidenceRef>,
    /// The intermediate entities, for a transitive dependency. Empty for a direct one.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub path: Vec<EntityRef>,
    /// How many edges separate the subject from the object.
    pub depth: usize,
    /// Why the dependency exists, in terms of the evidence.
    pub reason: String,
    /// The boundary the dependency was established at, where one was recorded.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observed_at: Option<ObservationBoundary>,
}

impl Dependency {
    fn from_classification(
        candidate: &Candidate,
        classification: Classification,
        depth: usize,
        path: Vec<EntityRef>,
    ) -> Self {
        Self {
            subject: candidate.subject.clone(),
            object: candidate.object.clone(),
            relationship: candidate.relationship,
            classes: classification.classes,
            basis: candidate.basis,
            confidence: classification.confidence,
            verification: classification.verification,
            evidence: candidate.evidence.clone(),
            path,
            depth,
            reason: classification.rationale,
            observed_at: candidate.boundary.clone(),
        }
    }

    /// The network the dependency was established on, where one was recorded.
    #[must_use]
    pub fn network(&self) -> Option<&Network> {
        self.observed_at.as_ref().map(|boundary| &boundary.network)
    }

    /// Adds a class, keeping the canonical order.
    ///
    /// Absent classes are inserted where the taxonomy puts them, so that a set of
    /// dependencies can be compared byte for byte.
    #[must_use]
    pub fn with_class(mut self, class: DependencyClass) -> Self {
        if !self.classes.contains(&class) {
            self.classes.push(class);
            self.classes.sort_by_key(|class| {
                DependencyClass::all()
                    .iter()
                    .position(|candidate| candidate == class)
                    .unwrap_or(usize::MAX)
            });
        }
        self
    }

    /// Whether the dependency was verified.
    #[must_use]
    pub const fn is_verified(&self) -> bool {
        self.verification.is_affirmation()
    }

    /// Whether the dependency may be reported among the observed facts.
    #[must_use]
    pub const fn is_observed(&self) -> bool {
        self.is_verified() && self.confidence.has_support()
    }

    /// The transaction identifiers the dependency cites.
    #[must_use]
    pub fn transactions(&self) -> Vec<&str> {
        self.evidence
            .iter()
            .filter(|evidence| evidence.kind == amasario_core::EvidenceType::Transaction)
            .map(|evidence| evidence.id.as_str())
            .collect()
    }

    /// The key two observations of one edge share.
    ///
    /// Includes the **subject**, which is what makes this an identity rather than a
    /// partial match. Two observations of one edge share their subject, their relationship
    /// and their object; an edge is directed, so sharing only the last two describes two
    /// different edges - `A -> C` and `B -> C` - and treating those as one would silently
    /// discard a real relationship. Observations are collected per contract but resolved
    /// together, so a set routinely holds edges whose subjects differ, and a key that
    /// ignored the subject would merge them.
    const fn edge_key(&self) -> EdgeKey<'_> {
        (
            EntityKindKey(self.subject.kind),
            self.subject.id.as_str(),
            EntityKindKey(self.object.kind),
            self.object.id.as_str(),
            self.relationship.as_str(),
        )
    }
}

/// A candidate that could not become a dependency, and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Unestablished {
    /// The object the candidate pointed at.
    pub object: EntityRef,
    /// The relationship the candidate asserted.
    pub relationship: Relationship,
    /// The basis it rested on.
    pub basis: Basis,
    /// Why it could not become a dependency.
    pub reason: String,
    /// What was observed, where the candidate recorded it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// A cycle the traversal found.
///
/// Reported rather than removed or collapsed, as `dependency/transitive-dependency`
/// requires: a cycle is a fact about the graph, and a consumer that never sees it
/// cannot know the graph was not a tree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Cycle {
    /// The entities on the cycle, in the order they were entered.
    pub entities: Vec<EntityRef>,
    /// The edges that form it, as `subject -RELATIONSHIP-> object` identifiers.
    pub edges: Vec<String>,
}

impl Cycle {
    /// The identifiers of the entities on the cycle.
    #[must_use]
    pub fn entity_ids(&self) -> Vec<&str> {
        self.entities
            .iter()
            .map(|entity| entity.id.as_str())
            .collect()
    }
}

/// Every dependency established for one subject, partitioned and bounded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DependencySet {
    /// The entity the set is about.
    pub subject: EntityRef,
    /// The boundary the analysis was bounded by.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub boundary: Option<ObservationBoundary>,
    /// The depth bound traversal was given.
    pub max_depth: usize,
    /// Dependencies one edge away from the subject.
    pub direct: Vec<Dependency>,
    /// Dependencies further away, each carrying its path.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub transitive: Vec<Dependency>,
    /// Candidates that could not become dependencies, with their reasons.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unestablished: Vec<Unestablished>,
    /// Cycles found while closing the set.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cycles: Vec<Cycle>,
    /// Whether traversal stopped before exhausting what is reachable.
    pub truncated: bool,
    /// Why it stopped, when it did.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub truncation_reason: Option<TruncationReason>,
}

impl DependencySet {
    /// Every dependency, direct first then transitive.
    pub fn all(&self) -> impl Iterator<Item = &Dependency> {
        self.direct.iter().chain(self.transitive.iter())
    }

    /// How many dependencies were established.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.direct.len() + self.transitive.len()
    }

    /// Whether no dependency was established.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The dependencies carrying a class.
    #[must_use]
    pub fn with_class(&self, class: DependencyClass) -> Vec<&Dependency> {
        self.all()
            .filter(|dep| dep.classes.contains(&class))
            .collect()
    }

    /// The dependencies that could be reported among the observed facts.
    #[must_use]
    pub fn observed_facts(&self) -> Vec<&Dependency> {
        self.all().filter(|dep| dep.is_observed()).collect()
    }

    /// Checks the invariants `dependency/transitive-dependency` states, so that a set
    /// assembled by hand cannot be serialised as though it were produced by the
    /// engine.
    ///
    /// # Errors
    ///
    /// Returns a dependency error naming the first violated rule.
    pub fn validate(&self) -> Result<()> {
        if self.truncated && self.truncation_reason.is_none() {
            return Err(violation(
                "/dependencySet/truncationReason",
                "a truncated set must say why traversal stopped; a bounded search that does not \
                 say it was bounded invites the reader to conclude the dependency is absent",
            ));
        }
        if !self.truncated && self.truncation_reason.is_some() {
            return Err(violation(
                "/dependencySet/truncated",
                "a truncation reason without truncation would make a complete result look partial",
            ));
        }

        let mut seen: Vec<(
            (&'static str, String, &'static str, String, &'static str),
            &'static str,
        )> = Vec::new();
        for dependency in self.all() {
            if dependency.evidence.is_empty() {
                return Err(violation(
                    "/dependency/evidence",
                    "a dependency must reference at least one evidence record",
                ));
            }
            if dependency.reason.is_empty() {
                return Err(violation(
                    "/dependency/reason",
                    "a dependency must state why it exists, in terms of its evidence",
                ));
            }
            if dependency.classes.is_empty() {
                return Err(violation(
                    "/dependency/classes",
                    "a dependency must carry at least one class",
                ));
            }
            if dependency.classes.contains(&DependencyClass::Transitive) {
                if dependency.path.is_empty() {
                    return Err(violation(
                        "/dependency/path",
                        "a transitive dependency must carry the intermediate entities between its \
                         source and target, because without them it is indistinguishable from a \
                         guess that collapsed two hops into one",
                    ));
                }
                if dependency.depth != dependency.path.len() + 1 {
                    return Err(violation(
                        "/dependency/hopDepth",
                        "a transitive dependency's depth must equal one more than the number of \
                         intermediate entities",
                    ));
                }
            }
            if dependency.classes.contains(&DependencyClass::Direct) && dependency.depth != 0 {
                return Err(violation(
                    "/dependency/hopDepth",
                    "a direct dependency is one edge from the subject and has depth zero",
                ));
            }
            if dependency.classes.contains(&DependencyClass::External)
                && dependency.verification == VerificationStatus::Verified
            {
                return Err(violation(
                    "/dependency/verificationStatus",
                    "an EXTERNAL dependency must not be reported as VERIFIED; its target lies \
                     outside the observable boundary, so there is nothing to check it against",
                ));
            }
            if dependency.classes.contains(&DependencyClass::Runtime)
                && dependency.confidence.level == ConfidenceLevel::Unknown
            {
                return Err(violation(
                    "/dependency/confidence",
                    "a runtime dependency must cite evidence, and a UNKNOWN confidence has none",
                ));
            }

            let partition = if dependency.depth == 0 {
                "direct"
            } else {
                "transitive"
            };
            // The subject is part of the key, for the reason [`Dependency::edge_key`] gives:
            // a pair of endpoints and a relationship without the source does not identify an
            // edge, and a set holding edges from several subjects would otherwise report a
            // false collision between two unrelated ones.
            let key = (
                dependency.subject.kind.as_str(),
                dependency.subject.id.clone(),
                dependency.object.kind.as_str(),
                dependency.object.id.clone(),
                dependency.relationship.as_str(),
            );
            if let Some((_, previous)) = seen.iter().find(|(existing, _)| {
                existing.0 == key.0
                    && existing.1 == key.1
                    && existing.2 == key.2
                    && existing.3 == key.3
                    && existing.4 == key.4
            }) {
                return Err(violation(
                    "/dependencySet",
                    &format!(
                        "the edge {} -> {} appears in both the {previous} and the {partition} \
                         partition; an edge belongs to exactly one of them",
                        dependency.subject.id, dependency.object.id
                    ),
                ));
            }
            seen.push((key, partition));
        }
        Ok(())
    }
}

/// An error for a violated set invariant.
fn violation(path: &str, detail: &str) -> amasario_core::EngineError {
    amasario_core::EngineError::Dependency(format!("{path}: {detail}"))
}

/// The identity of an edge: its subject, its object and the relationship between them.
///
/// A type alias rather than a struct so that ordering and equality are the tuple's, and so
/// that a reader can see at a glance which fields make two observations the same edge.
type EdgeKey<'a> = (EntityKindKey, &'a str, EntityKindKey, &'a str, &'static str);

/// A wrapper that orders entity kinds by their wire name, so that a map of edges has
/// a deterministic order without depending on the enumeration's declaration order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct EntityKindKey(amasario_core::EntityKind);

/// Turns candidates into a validated dependency set.
///
/// # Errors
///
/// Returns a dependency error when a candidate cannot be constructed into a
/// dependency for a reason other than a refusal, or when the assembled set violates
/// an invariant. Refusals are recorded in [`DependencySet::unestablished`] rather than
/// raised.
pub fn resolve(
    subject: EntityRef,
    boundary: Option<ObservationBoundary>,
    candidates: &[Candidate],
    max_depth: usize,
) -> Result<DependencySet> {
    let mut direct: Vec<Dependency> = Vec::new();
    let mut unestablished = Vec::new();

    for candidate in candidates {
        match classify(candidate) {
            Ok(classification) => {
                direct.push(Dependency::from_classification(
                    candidate,
                    classification,
                    0,
                    Vec::new(),
                ));
            },
            Err(error) => unestablished.push(Unestablished {
                object: candidate.object.clone(),
                relationship: candidate.relationship,
                basis: candidate.basis,
                reason: error.to_string(),
                detail: candidate.detail.clone(),
            }),
        }
    }

    let mut set = DependencySet {
        subject,
        boundary,
        max_depth,
        direct: merge_duplicates(direct),
        transitive: Vec::new(),
        unestablished,
        cycles: Vec::new(),
        truncated: false,
        truncation_reason: None,
    };
    set.direct
        .sort_by(|left, right| left.edge_key().cmp(&right.edge_key()));
    set.unestablished.sort_by(|left, right| {
        left.object
            .id
            .cmp(&right.object.id)
            .then_with(|| left.relationship.as_str().cmp(right.relationship.as_str()))
    });
    set.validate()?;
    Ok(set)
}

/// Merges repeated observations of one edge, preserving determinism and lowering
/// nothing that should be raised.
fn merge_duplicates(dependencies: Vec<Dependency>) -> Vec<Dependency> {
    let mut merged: Vec<Dependency> = Vec::new();
    for dependency in dependencies {
        match merged
            .iter_mut()
            .find(|existing| existing.edge_key() == dependency.edge_key())
        {
            Some(existing) => *existing = merge(existing.clone(), dependency),
            None => merged.push(dependency),
        }
    }
    merged
}

fn merge(mut left: Dependency, right: Dependency) -> Dependency {
    if stronger_basis(right.basis, left.basis) {
        left.basis = right.basis;
        left.reason = right.reason.clone();
    }
    for class in right.classes {
        left = left.with_class(class);
    }
    for evidence in right.evidence {
        if !left.evidence.contains(&evidence) {
            left.evidence.push(evidence);
        }
    }
    let mut contradicting: Vec<String> = left.confidence.contradicting_evidence.clone();
    for entry in right.confidence.contradicting_evidence {
        if !contradicting.contains(&entry) {
            contradicting.push(entry);
        }
    }
    let level = left.confidence.level.weakest(right.confidence.level);
    let evidence: Vec<String> = left.evidence.iter().map(ToString::to_string).collect();
    // Both sides cited evidence, so the merged citation list is non-empty and the
    // constructor cannot fail; the fallback keeps the stronger level rather than
    // dropping the dependency if that ever stops being true.
    left.confidence = Confidence::new(level, evidence, contradicting).unwrap_or(left.confidence);
    left.verification = weakest_status(left.verification, right.verification);
    if left.observed_at.is_none() {
        left.observed_at = right.observed_at;
    }
    left
}

/// Whether `candidate` is a stronger basis than `current`.
fn stronger_basis(candidate: Basis, current: Basis) -> bool {
    let rank = |basis: Basis| {
        Basis::all_strongest_first()
            .iter()
            .position(|known| *known == basis)
            .unwrap_or(usize::MAX)
    };
    rank(candidate) < rank(current)
}

/// The weaker of two verification statuses, by the rule this module states.
///
/// `CONFLICTING` wins over everything, `VERIFIED` requires every observation of the
/// edge to have been verified, and anything else is `UNVERIFIED`. `PARTIALLY_VERIFIED`
/// is never produced here: it describes a claim with several components, and an edge
/// with two readings has one claim.
#[must_use]
pub const fn weakest_status(
    left: VerificationStatus,
    right: VerificationStatus,
) -> VerificationStatus {
    if left.is_refutation() || right.is_refutation() {
        return VerificationStatus::Conflicting;
    }
    if left.is_affirmation() && right.is_affirmation() {
        return VerificationStatus::Verified;
    }
    VerificationStatus::Unverified
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::classifier::EvidenceRef;
    use amasario_core::{
        EntityKind, EvidenceType, LedgerSequence, Network, NetworkType, Relationship,
    };

    fn entity(kind: EntityKind, id: &str) -> EntityRef {
        EntityRef::new(kind, id).expect("a reference")
    }

    fn boundary() -> ObservationBoundary {
        ObservationBoundary {
            network: Network::new(
                "testnet",
                NetworkType::Testnet,
                "Test SDF Network ; September 2015",
            )
            .expect("a network"),
            ledger: LedgerSequence::new(3_000).expect("a ledger"),
            observed_at: "2026-09-15T00:00:00Z".to_owned(),
            spec_version: None,
        }
    }

    /// A cross-contract call from a named subject, for the cases where the subject is
    /// what the test is about.
    fn call(subject: &str, callee: &str, transaction: &str) -> Candidate {
        Candidate::new(
            entity(EntityKind::Contract, subject),
            entity(EntityKind::Contract, callee),
            Relationship::Invocates,
            Basis::ObservedInvocation,
            vec![EvidenceRef::new(EvidenceType::Transaction, transaction).expect("a citation")],
        )
        .expect("a candidate")
        .observed_at(boundary())
        .with_outcome(Some(true))
    }

    fn invocation(callee: &str, transaction: &str) -> Candidate {
        Candidate::new(
            entity(EntityKind::Contract, "C-subject"),
            entity(EntityKind::Contract, callee),
            Relationship::Invocates,
            Basis::ObservedInvocation,
            vec![EvidenceRef::new(EvidenceType::Transaction, transaction).expect("a citation")],
        )
        .expect("a candidate")
        .observed_at(boundary())
        .with_outcome(Some(true))
    }

    #[test]
    fn a_resolved_set_partitions_every_edge_once_and_validates() {
        let set = resolve(
            entity(EntityKind::Contract, "C-subject"),
            Some(boundary()),
            &[
                invocation("C-a", &"a".repeat(64)),
                invocation("C-b", &"b".repeat(64)),
            ],
            5,
        )
        .expect("resolves");
        assert_eq!(set.len(), 2);
        assert!(set.transitive.is_empty());
        assert_eq!(set.max_depth, 5);
        assert!(!set.truncated);
        assert!(set.unestablished.is_empty());
        set.validate().expect("the set satisfies its invariants");
        assert_eq!(set.observed_facts().len(), 2);
        assert_eq!(set.direct[0].object.id, "C-a");
        assert_eq!(set.direct[0].depth, 0);
        assert!(set.direct[0].is_verified());
        assert_eq!(set.direct[0].transactions(), vec!["a".repeat(64).as_str()]);
    }

    #[test]
    fn a_refused_candidate_is_recorded_rather_than_dropped() {
        let failed = invocation("C-c", &"c".repeat(64)).with_outcome(Some(false));
        let set = resolve(
            entity(EntityKind::Contract, "C-subject"),
            Some(boundary()),
            &[failed],
            5,
        )
        .expect("resolves");
        assert!(set.is_empty());
        assert_eq!(set.unestablished.len(), 1);
        assert!(
            set.unestablished[0].reason.contains("RUNTIME"),
            "got: {}",
            set.unestablished[0].reason
        );
    }

    #[test]
    fn repeated_observations_merge_conservatively() {
        // Two observations of one edge: one edge in the set, both citations kept.
        let first = invocation("C-a", &"a".repeat(64));
        let mut second = invocation("C-a", &"b".repeat(64));
        second.basis = Basis::ObservedEvent;

        let set = resolve(
            entity(EntityKind::Contract, "C-subject"),
            Some(boundary()),
            &[first, second],
            5,
        )
        .expect("resolves");
        assert_eq!(set.direct.len(), 1, "one edge, however many observations");
        let merged = &set.direct[0];
        assert_eq!(merged.basis, Basis::ObservedInvocation);
        assert_eq!(merged.transactions().len(), 2, "both citations survive");
        assert_eq!(merged.confidence.level, ConfidenceLevel::Verified);

        // A declaration cannot establish runtime use at all, so it is refused rather
        // than merged into the stronger reading's shadow.
        let mut declared = invocation("C-a", &"c".repeat(64));
        declared.basis = Basis::DeclaredManifest;
        let declared = declared.observed_at(boundary());
        let alone = resolve(
            entity(EntityKind::Contract, "C-subject"),
            Some(boundary()),
            &[declared],
            5,
        )
        .expect("resolves");
        assert_eq!(alone.unestablished.len(), 1);
        assert!(alone.is_empty());
    }

    #[test]
    fn a_merged_edge_keeps_the_strongest_basis_and_the_weakest_confidence() {
        // Both dimensions move downward, and they move differently: the basis says how
        // the edge was established (the best reading available), while the confidence
        // aggregates every reading, so one weaker reading lowers the whole claim.
        let declared = Candidate::new(
            entity(EntityKind::Contract, "C-subject"),
            entity(EntityKind::Package, "soroban-sdk"),
            Relationship::DependsOn,
            Basis::DeclaredManifest,
            vec![EvidenceRef::new(EvidenceType::Source, "manifest-1").expect("a citation")],
        )
        .expect("a candidate")
        .observed_at(boundary());
        let resolved = Candidate::new(
            entity(EntityKind::Contract, "C-subject"),
            entity(EntityKind::Package, "soroban-sdk"),
            Relationship::DependsOn,
            Basis::ResolvedLockfile,
            vec![EvidenceRef::new(EvidenceType::Build, "lock-1").expect("a citation")],
        )
        .expect("a candidate")
        .observed_at(boundary());

        let set = resolve(
            entity(EntityKind::Contract, "C-subject"),
            Some(boundary()),
            &[declared, resolved],
            5,
        )
        .expect("resolves");
        assert_eq!(set.direct.len(), 1);
        let merged = &set.direct[0];
        assert_eq!(merged.basis, Basis::ResolvedLockfile);
        assert_eq!(merged.confidence.level, ConfidenceLevel::MediumConfidence);
        assert_eq!(merged.evidence.len(), 2);
    }

    #[test]
    fn two_edges_sharing_an_object_are_two_edges() {
        // `C-a -> C-c` and `C-b -> C-c` share their object and their relationship and are
        // two different edges. An edge is directed, so a key that omitted the subject would
        // collapse them and silently discard whichever arrived second - and observations
        // are collected per contract but resolved together, so this is the ordinary case
        // rather than a contrived one.
        let set = resolve(
            entity(EntityKind::Contract, "C-a"),
            Some(boundary()),
            &[
                call("C-a", "C-c", &"a".repeat(64)),
                call("C-b", "C-c", &"b".repeat(64)),
            ],
            5,
        )
        .expect("resolves");
        assert_eq!(
            set.direct.len(),
            2,
            "both edges survive: {:?}",
            set.direct
                .iter()
                .map(|dependency| format!("{} -> {}", dependency.subject.id, dependency.object.id))
                .collect::<Vec<_>>()
        );
        assert_eq!(set.direct[0].subject.id, "C-a");
        assert_eq!(set.direct[1].subject.id, "C-b");
        assert_eq!(set.direct[0].object.id, "C-c");
        assert_eq!(set.direct[1].object.id, "C-c");
        set.validate()
            .expect("two edges with different subjects do not collide");
    }

    #[test]
    fn two_observations_of_one_directed_edge_still_merge() {
        // The complement of the test above: widening the key must not stop two readings of
        // the same edge from being merged, or a repeated observation would be reported
        // twice.
        let set = resolve(
            entity(EntityKind::Contract, "C-a"),
            Some(boundary()),
            &[
                call("C-a", "C-c", &"a".repeat(64)),
                call("C-a", "C-c", &"b".repeat(64)),
            ],
            5,
        )
        .expect("resolves");
        assert_eq!(set.direct.len(), 1, "one edge, however many observations");
        assert_eq!(
            set.direct[0].transactions().len(),
            2,
            "both citations survive"
        );
    }

    #[test]
    fn the_weakest_status_rule_is_not_partially_verified() {
        assert_eq!(
            weakest_status(VerificationStatus::Verified, VerificationStatus::Unverified),
            VerificationStatus::Unverified
        );
        assert_eq!(
            weakest_status(VerificationStatus::Verified, VerificationStatus::Verified),
            VerificationStatus::Verified
        );
        assert_eq!(
            weakest_status(
                VerificationStatus::Verified,
                VerificationStatus::Conflicting
            ),
            VerificationStatus::Conflicting
        );
        assert_eq!(
            weakest_status(VerificationStatus::Unverified, VerificationStatus::Unknown),
            VerificationStatus::Unverified
        );
    }

    #[test]
    fn a_truncated_set_must_say_why_and_an_untruncated_one_must_not() {
        let mut set = resolve(
            entity(EntityKind::Contract, "C-subject"),
            Some(boundary()),
            &[invocation("C-a", &"a".repeat(64))],
            5,
        )
        .expect("resolves");

        set.truncated = true;
        set.validate()
            .expect_err("a bounded search must say it was bounded");

        set.truncation_reason = Some(TruncationReason::MaxDepthReached);
        set.validate().expect("a complete statement validates");

        set.truncated = false;
        set.validate()
            .expect_err("a reason without truncation misrepresents a complete result");
    }

    #[test]
    fn an_edge_in_both_partitions_is_refused() {
        let mut set = resolve(
            entity(EntityKind::Contract, "C-subject"),
            Some(boundary()),
            &[invocation("C-a", &"a".repeat(64))],
            5,
        )
        .expect("resolves");
        let mut duplicated = set.direct[0].clone();
        duplicated.classes = vec![DependencyClass::Transitive, DependencyClass::Contract];
        duplicated.path = vec![entity(EntityKind::Contract, "C-middle")];
        duplicated.depth = 2;
        set.transitive.push(duplicated);
        let error = set.validate().expect_err("one edge, one partition");
        assert!(error.to_string().contains("exactly one"), "got: {error}");
    }

    #[test]
    fn a_transitive_dependency_without_a_path_is_refused() {
        let mut set = resolve(
            entity(EntityKind::Contract, "C-subject"),
            Some(boundary()),
            &[invocation("C-a", &"a".repeat(64))],
            5,
        )
        .expect("resolves");
        set.transitive.push(Dependency {
            classes: vec![DependencyClass::Transitive, DependencyClass::Contract],
            depth: 2,
            ..set.direct[0].clone()
        });
        let error = set.validate().expect_err("a path is required");
        assert!(
            error.to_string().contains("indistinguishable from a guess"),
            "got: {error}"
        );
    }

    #[test]
    fn an_external_dependency_cannot_be_verified_even_by_hand() {
        let candidate = Candidate::new(
            entity(EntityKind::Artifact, "artifact-1"),
            entity(EntityKind::Source, "https://example.invalid/r@abc"),
            Relationship::DependsOn,
            Basis::DeclaredManifest,
            vec![EvidenceRef::new(EvidenceType::Source, "src-1").expect("a citation")],
        )
        .expect("a candidate")
        .observed_at(boundary());
        let set = resolve(
            entity(EntityKind::Artifact, "artifact-1"),
            Some(boundary()),
            &[candidate],
            0,
        )
        .expect("resolves");
        let external = set.with_class(DependencyClass::External);
        assert_eq!(external.len(), 1);
        assert!(!external[0].is_verified());

        let mut forged = set;
        forged.direct[0].verification = VerificationStatus::Verified;
        let error = forged.validate().expect_err("EXTERNAL is not verifiable");
        assert!(
            error.to_string().contains("nothing to check"),
            "got: {error}"
        );
    }

    #[test]
    fn classes_are_added_in_canonical_order() {
        let set = resolve(
            entity(EntityKind::Contract, "C-subject"),
            Some(boundary()),
            &[invocation("C-a", &"a".repeat(64))],
            5,
        )
        .expect("resolves");
        let dependency = set.direct[0].clone().with_class(DependencyClass::Direct);
        let canonical: Vec<DependencyClass> = DependencyClass::all()
            .iter()
            .copied()
            .filter(|class| dependency.classes.contains(class))
            .collect();
        assert_eq!(dependency.classes, canonical);
        assert_eq!(dependency.classes.len(), 3);
    }

    #[test]
    fn resolution_is_deterministic() {
        let candidates = [
            invocation("C-zeta", &"1".repeat(64)),
            invocation("C-alpha", &"2".repeat(64)),
        ];
        let first = resolve(
            entity(EntityKind::Contract, "C-subject"),
            Some(boundary()),
            &candidates,
            5,
        )
        .expect("resolves");
        let second = resolve(
            entity(EntityKind::Contract, "C-subject"),
            Some(boundary()),
            &candidates,
            5,
        )
        .expect("resolves");
        assert_eq!(first, second);
        assert_eq!(first.direct[0].object.id, "C-alpha");
        assert_eq!(first.direct[1].object.id, "C-zeta");
        assert_eq!(
            serde_json::to_string(&first).expect("serialises"),
            serde_json::to_string(&second).expect("serialises")
        );
    }
}
