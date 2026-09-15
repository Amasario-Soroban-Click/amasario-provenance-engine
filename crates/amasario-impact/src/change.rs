//! Change-triggered findings: what changed, and what that change reaches.
//!
//! # Why the change is an input and not an inference
//!
//! Everything else in this crate propagates a change that the analysis is told about:
//! the caller names an entity and the traversal reports what it reaches. That covers
//! "if this changes, what breaks", but the question a pipeline actually asks is "this
//! *did* change - what does that reach". The difference is that the second has a
//! change type, and `taxonomies/change-types.yaml` makes the change type load-bearing:
//! `UPGRADED` and `DOWNGRADED` may only be used "where the ordering is established by
//! the referenced ecosystem", and `MODIFIED` must be used otherwise, "because Amasario
//! does not guess ordering". Guessing there would invert the direction of an impact
//! assessment, which is the one error a reader could not detect from the output.
//!
//! # The zero-hop finding
//!
//! A change is itself a finding. `schema/impact.schema.json` notes that `hopDepth` of
//! zero "denotes the changed entity affecting itself, which is only meaningful for a
//! CHANGE-type finding", and [`Change::finding`] produces exactly that: the changed
//! entity as its own affected entity, no path, and `CHANGE` in the classification set
//! with the change type recorded. It is not padding - it is what lets a report say what
//! it was asked about, and what makes a snapshot diff able to distinguish "nothing was
//! affected" from "nothing was analysed".
//!
//! # The set is canonical
//!
//! [`ChangeSet`] sorts and deduplicates its changes on construction. A diff produced
//! from two snapshots can legitimately report the same entity twice - a relationship
//! added and a provenance record modified, say - and the analysis must not depend on the
//! order they arrived in, because finding identifiers are derived from entity and change
//! type rather than from a sequence number.

use amasario_core::{Confidence, ConfidenceLevel, EntityRef, Result};
use amasario_dependency::EvidenceRef;

use crate::affected::{ChangeType, ImpactFinding};
use crate::errors::ImpactFailure;

/// The shortest a change's reason may be before it stops being one.
const MINIMUM_REASON_LENGTH: usize = 8;

/// One entity that changed, with the kind of change and why it was detected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    /// The entity that changed or is assumed to change.
    pub entity: EntityRef,
    /// The kind of change.
    pub change_type: ChangeType,
    /// Why the change was detected, in terms of the evidence.
    pub reason: String,
    /// Evidence that the change occurred. Never empty.
    pub evidence: Vec<EvidenceRef>,
    /// How strongly the evidence supports the change, or `None` when the change was
    /// supplied as an assumption rather than derived from evidence.
    pub confidence: Option<Confidence>,
}

impl Change {
    /// Builds a change, rejecting one with no evidence or no reason.
    ///
    /// An assumed change is legitimate - "what if this contract's executable were
    /// replaced" is the question an operator asks before an upgrade - and the evidence
    /// requirement is not suspended for it. What changes is what the evidence records:
    /// for an assumption it is an observation that the assumption was made at a
    /// boundary, which is the schema's own example of an `UNKNOWN`-level confidence that
    /// still cites a record.
    ///
    /// # Errors
    ///
    /// Returns an impact failure when the evidence list is empty or the reason is too
    /// short to state one.
    pub fn new(
        entity: EntityRef,
        change_type: ChangeType,
        reason: impl Into<String>,
        evidence: Vec<EvidenceRef>,
    ) -> Result<Self> {
        let reason = reason.into();
        if evidence.is_empty() {
            return Err(ImpactFailure::NoEvidenceCited {
                finding: format!("the change to {entity}"),
            }
            .into_error());
        }
        let length = reason.chars().count();
        if length < MINIMUM_REASON_LENGTH {
            return Err(ImpactFailure::ReasonNotStated { length }.into_error());
        }
        Ok(Self {
            entity,
            change_type,
            reason,
            evidence,
            confidence: None,
        })
    }

    /// Records how strongly the change's evidence supports it.
    #[must_use]
    pub fn with_confidence(mut self, confidence: Confidence) -> Self {
        self.confidence = Some(confidence);
        self
    }

    /// Whether the change claims an ordering between the two states.
    ///
    /// `true` for `UPGRADED` and `DOWNGRADED`, which the taxonomy permits only where
    /// "the ordering is established by the referenced ecosystem". Exposed so that a
    /// caller assembling changes from a diff can assert that it *has* such an ordering
    /// rather than assuming the term is available.
    #[must_use]
    pub const fn claims_ordering(&self) -> bool {
        self.change_type.claims_ordering()
    }

    /// The zero-hop finding for this change.
    ///
    /// The affected entity is the changed one, which is what `hopDepth` zero means. The
    /// confidence is the change's own, or `UNKNOWN` with the change's evidence when the
    /// change recorded none: the schema requires a level and requires it to cite the
    /// record that establishes the absence, so "we were told this changed and have not
    /// checked" is representable and is distinguishable from "nothing was attempted".
    ///
    /// # Errors
    ///
    /// Returns an impact failure when the change carries no evidence, which
    /// [`Self::new`] already prevents; the check is repeated because a `Change` can be
    /// built by a caller that constructs the struct directly.
    pub fn finding(&self) -> Result<ImpactFinding> {
        let confidence = match &self.confidence {
            Some(confidence) => confidence.clone(),
            None => Confidence::new(
                ConfidenceLevel::Unknown,
                self.evidence
                    .iter()
                    .map(|citation| citation.id.clone())
                    .collect(),
                Vec::new(),
            )?
            .with_rationale(
                "the change was reported rather than derived, so its level describes the record \
                 that reported it",
            ),
        };
        ImpactFinding::new(
            self.entity.clone(),
            self.entity.clone(),
            0,
            None,
            Some(self.change_type),
            self.evidence.clone(),
            confidence,
            format!(
                "{} was {} at the observed boundary: {}",
                self.entity,
                self.change_type.as_str(),
                self.reason
            ),
        )
    }
}

/// A canonical set of changes to analyse.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangeSet {
    /// The changes, sorted and deduplicated on construction.
    pub changes: Vec<Change>,
}

impl ChangeSet {
    /// Builds a set, sorting and deduplicating its changes.
    ///
    /// The total order is the entity kind, then the entity identifier, then the change
    /// type name. Two changes with the same entity and change type are the same change
    /// as far as the analysis is concerned, so the first in that order is kept and the
    /// rest are dropped - keeping both would produce two findings with the same
    /// identifier, which a snapshot diff would report as a duplicate rather than as a
    /// change.
    #[must_use]
    pub fn new(changes: Vec<Change>) -> Self {
        let mut changes = changes;
        changes.sort_by(|left, right| {
            left.entity
                .kind
                .cmp(&right.entity.kind)
                .then_with(|| left.entity.id.cmp(&right.entity.id))
                .then_with(|| left.change_type.cmp(&right.change_type))
        });
        changes.dedup_by(|left, right| {
            left.entity == right.entity && left.change_type == right.change_type
        });
        Self { changes }
    }

    /// The entities that changed, canonical and deduplicated.
    ///
    /// Distinct from the changes themselves because two change types against one entity
    /// are one traversal: the graph does not depend on why the analysis started there.
    #[must_use]
    pub fn entities(&self) -> Vec<EntityRef> {
        let mut entities: Vec<EntityRef> = self
            .changes
            .iter()
            .map(|change| change.entity.clone())
            .collect();
        entities.dedup();
        entities
    }

    /// The changes affecting one entity.
    #[must_use]
    pub fn for_entity(&self, entity: &EntityRef) -> Vec<&Change> {
        self.changes
            .iter()
            .filter(|change| change.entity == *entity)
            .collect()
    }

    /// How many changes the set holds.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.changes.len()
    }

    /// Whether the set holds no changes.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.changes.is_empty()
    }

    /// The zero-hop finding for every change, in canonical order.
    ///
    /// # Errors
    ///
    /// Returns the first impact failure, which can only arise from a `Change` that was
    /// built without going through [`Change::new`].
    pub fn self_findings(&self) -> Result<Vec<ImpactFinding>> {
        let mut findings = Vec::with_capacity(self.changes.len());
        for change in &self.changes {
            findings.push(change.finding()?);
        }
        Ok(findings)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use amasario_core::{EntityKind, EvidenceType};

    fn entity(kind: EntityKind, id: &str) -> EntityRef {
        EntityRef::new(kind, id).expect("a non-empty identifier")
    }

    fn citation(id: &str) -> EvidenceRef {
        EvidenceRef::new(EvidenceType::Transaction, id).expect("a citation")
    }

    fn change(entity_ref: EntityRef, change_type: ChangeType, reason: &str) -> Change {
        Change::new(entity_ref, change_type, reason, vec![citation("tx-1")])
            .expect("a valid change")
    }

    #[test]
    fn a_change_without_evidence_is_refused() {
        let error = Change::new(
            entity(EntityKind::Contract, "C-a"),
            ChangeType::Modified,
            "the contract was redeployed",
            Vec::new(),
        )
        .expect_err("a change with no evidence must be refused");
        assert!(error.to_string().contains("cites no evidence"));
    }

    #[test]
    fn a_change_needs_a_reason_that_states_something() {
        let error = Change::new(
            entity(EntityKind::Contract, "C-a"),
            ChangeType::Modified,
            "yes",
            vec![citation("tx-1")],
        )
        .expect_err("a two-character reason is not a reason");
        assert!(error.to_string().contains("3 characters"));
    }

    #[test]
    fn the_zero_hop_finding_matches_what_a_zero_hop_depth_means() {
        let finding = change(
            entity(EntityKind::Wasm, &"ab".repeat(32)),
            ChangeType::Replaced,
            "the executable at this digest was replaced on testnet",
        )
        .finding()
        .expect("a zero-hop finding");
        assert_eq!(finding.hop_depth, 0);
        assert!(finding.path.is_none());
        assert!(finding.has(crate::affected::ImpactType::Change));
        assert!(finding.has(crate::affected::ImpactType::Artifact));
        assert_eq!(finding.changed_entity, finding.affected_entity);
        assert_eq!(finding.change_type, Some(ChangeType::Replaced));
        assert!(finding.is_valid(), "{:?}", finding.failures());
    }

    #[test]
    fn a_reported_rather_than_derived_change_is_pinned_at_unknown() {
        let finding = change(
            entity(EntityKind::Contract, "C-a"),
            ChangeType::Modified,
            "the operator reported the contract was reinstalled",
        )
        .finding()
        .expect("a zero-hop finding");
        assert_eq!(finding.confidence.level, ConfidenceLevel::Unknown);
        assert!(
            !finding.confidence.evidence.is_empty(),
            "UNKNOWN confidence still cites the record that establishes it"
        );
        assert!(
            finding
                .confidence
                .rationale
                .as_deref()
                .is_some_and(|rationale| rationale.contains("reported rather than derived"))
        );
    }

    #[test]
    fn only_the_two_ordering_terms_claim_an_ordering() {
        assert!(
            change(
                entity(EntityKind::Contract, "C-a"),
                ChangeType::Upgraded,
                "the contract moved to a later revision"
            )
            .claims_ordering()
        );
        assert!(
            !change(
                entity(EntityKind::Contract, "C-a"),
                ChangeType::Replaced,
                "the contract's executable was replaced"
            )
            .claims_ordering(),
            "REPLACED asserts no ordering between the two states"
        );
    }

    #[test]
    fn a_change_set_is_canonical_however_it_was_assembled() {
        let first = ChangeSet::new(vec![
            change(
                entity(EntityKind::Contract, "C-b"),
                ChangeType::Modified,
                "the second contract was reconfigured",
            ),
            change(
                entity(EntityKind::Contract, "C-a"),
                ChangeType::Added,
                "the first contract appeared at this address",
            ),
            change(
                entity(EntityKind::Contract, "C-a"),
                ChangeType::Added,
                "the first contract appeared at this address",
            ),
        ]);
        let second = ChangeSet::new(vec![
            change(
                entity(EntityKind::Contract, "C-a"),
                ChangeType::Added,
                "the first contract appeared at this address",
            ),
            change(
                entity(EntityKind::Contract, "C-b"),
                ChangeType::Modified,
                "the second contract was reconfigured",
            ),
        ]);
        assert_eq!(
            first, second,
            "assembly order must not survive construction"
        );
        assert_eq!(first.len(), 2, "the duplicate third change is dropped");
        assert_eq!(first.entities().len(), 2);
        assert_eq!(
            first.for_entity(&entity(EntityKind::Contract, "C-a")).len(),
            1
        );
    }

    #[test]
    fn two_change_types_against_one_entity_are_one_traversal_but_two_findings() {
        let set = ChangeSet::new(vec![
            change(
                entity(EntityKind::Contract, "C-a"),
                ChangeType::Added,
                "the contract appeared",
            ),
            change(
                entity(EntityKind::Contract, "C-a"),
                ChangeType::Replaced,
                "the contract's executable was replaced",
            ),
        ]);
        assert_eq!(set.len(), 2);
        assert_eq!(
            set.entities().len(),
            1,
            "the graph does not depend on why the analysis started at an entity"
        );
        let findings = set.self_findings().expect("two findings");
        assert_eq!(findings.len(), 2);
        assert_ne!(
            findings[0].id, findings[1].id,
            "two changes are two findings, so a diff can report the second as new"
        );
    }

    #[test]
    fn an_empty_set_is_empty_rather_than_invalid() {
        let set = ChangeSet::new(Vec::new());
        assert!(set.is_empty());
        assert_eq!(set.len(), 0);
        assert!(set.entities().is_empty());
        assert!(set.self_findings().expect("nothing to report").is_empty());
    }
}
