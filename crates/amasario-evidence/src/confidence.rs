//! How strongly a set of evidence supports a claim.
//!
//! # A level describes the evidence, never the thing evidenced
//!
//! `taxonomies/confidence-levels.yaml` opens with the invariant that shapes this module:
//! "A confidence level is never a substitute for evidence. Every claim at any level MUST
//! reference the evidence that supports it; the level describes that evidence, it does not
//! replace it." [`assess`] therefore takes records and returns a level together with the
//! citations it was computed from - it cannot be called without evidence, so a level
//! cannot exist without something behind it.
//!
//! "VERIFIED is therefore a statement about evidence completeness and consistency, never
//! about trustworthiness or safety (see SECURITY.md)." Nothing here reads a byte of
//! contract code, and nothing here could: a digest match is the strongest thing this
//! module can observe.
//!
//! # What each level means here, in terms of evidence
//!
//! The taxonomy defines the levels by what the evidence is like; this module implements
//! that as an [`EvidenceBasis`], which is the shape of the support rather than its amount:
//!
//! | basis | the evidence is | level |
//! | --- | --- | --- |
//! | [`EvidenceBasis::Decisive`] | deterministically comparable to the claim, so a check either passes or fails | `VERIFIED` |
//! | [`EvidenceBasis::Authoritative`] | complete for the claim but not re-derivable from the Amasario record | `HIGH_CONFIDENCE` |
//! | [`EvidenceBasis::Directional`] | consistent with the claim without establishing it, such as a branch name where a commit digest was expected | `MEDIUM_CONFIDENCE` |
//! | [`EvidenceBasis::Circumstantial`] | indirect, such as an observation that something was present | `LOW_CONFIDENCE` |
//! | [`EvidenceBasis::Uninterpretable`] | a term this specification version does not define | `UNKNOWN` |
//!
//! # Why the weakest link wins, and why conflict caps
//!
//! `taxonomies/confidence-levels.yaml` mandates `minimum_ordinal` for aggregation: "A
//! chain is only as strong as its weakest link, and a rule that permits any other
//! combination would let an implementation manufacture confidence out of unrelated strong
//! evidence." [`assess`] applies exactly that, and adds the one case the taxonomy states
//! separately: "If any evidence conflicts, the level MUST be lowered." A contradicted
//! claim can therefore never come back as `VERIFIED`, however strong the individual
//! records are.

use amasario_core::{Confidence, ConfidenceLevel, Result, VerificationStatus};
use amasario_provenance::{ATTESTATION_CONFIDENCE_CEILING, RevisionKind};

use crate::collector::{EvidenceRecord, infer_revision_kind};
use crate::errors::EvidenceFailure;

/// The shape of the support a record provides.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum EvidenceBasis {
    /// A term this version does not define: kept, and establishing nothing.
    Uninterpretable,
    /// Indirect evidence that something was present.
    Circumstantial,
    /// Evidence consistent with the claim without establishing it.
    Directional,
    /// Complete for the claim, but not re-derivable from the Amasario record alone.
    Authoritative,
    /// Deterministically comparable to the claim, so the check either passes or fails.
    Decisive,
}

impl EvidenceBasis {
    /// The stable wire name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Uninterpretable => "UNINTERPRETABLE",
            Self::Circumstantial => "CIRCUMSTANTIAL",
            Self::Directional => "DIRECTIONAL",
            Self::Authoritative => "AUTHORITATIVE",
            Self::Decisive => "DECISIVE",
        }
    }

    /// The confidence level the basis supports.
    #[must_use]
    pub const fn level(self) -> ConfidenceLevel {
        match self {
            Self::Decisive => ConfidenceLevel::Verified,
            Self::Authoritative => ATTESTATION_CONFIDENCE_CEILING,
            Self::Directional => ConfidenceLevel::MediumConfidence,
            Self::Circumstantial => ConfidenceLevel::LowConfidence,
            Self::Uninterpretable => ConfidenceLevel::Unknown,
        }
    }

    /// Whether a record of this basis alone establishes its claim.
    #[must_use]
    pub const fn is_decisive(self) -> bool {
        matches!(self, Self::Decisive)
    }

    /// Why the basis is the one it is, in terms of the record's fields.
    ///
    /// Written per basis rather than per class so that the rationale can say what a record
    /// is missing when it is not decisive, which is the sentence a producer needs in order
    /// to raise the level.
    #[must_use]
    pub const fn rationale(self) -> &'static str {
        match self {
            Self::Decisive => {
                "the record is deterministically comparable to the claim, so the check either \
                 passes or fails and can be repeated"
            },
            Self::Authoritative => {
                "the record is complete for the claim and cannot be re-derived from the \
                 Amasario record alone, so it is cited rather than recomputed"
            },
            Self::Directional => {
                "the record is consistent with the claim without establishing it: a mutable \
                 revision is a revision, and only a content-addressed one can be checked again"
            },
            Self::Circumstantial => {
                "the record asserts that something was present and nothing more, which is the \
                 weakest useful evidence and the honest form of an incomplete observation"
            },
            Self::Uninterpretable => {
                "the record's class is not defined at this specification version, so it is \
                 retained and establishes nothing"
            },
        }
    }
}

/// The basis a record provides, derived from its class and its fields.
///
/// Every arm is a statement about checkability rather than about importance. A source
/// record on an immutable revision is decisive because the revision identifies one tree;
/// the same record on a branch name is directional because the branch denotes different
/// trees at different times. A build record is authoritative rather than decisive because
/// Amasario can read the toolchain it claims and cannot re-run it.
#[must_use]
pub fn basis_for(record: &EvidenceRecord) -> EvidenceBasis {
    use amasario_core::EvidenceType;

    let Some(class) = record.class.recognised() else {
        return EvidenceBasis::Uninterpretable;
    };
    match class {
        EvidenceType::Source => {
            let kind = record
                .revision_kind
                .unwrap_or_else(|| infer_revision_kind(record.revision.as_deref().unwrap_or("")));
            match kind {
                RevisionKind::Commit => EvidenceBasis::Decisive,
                RevisionKind::Branch | RevisionKind::Tag => EvidenceBasis::Directional,
            }
        },
        // A build record is the engine's own account of how an artifact was produced. It
        // is complete for that claim and cannot be reproduced from the record alone, which
        // is the taxonomy's own definition of high confidence.
        EvidenceType::Build => EvidenceBasis::Authoritative,
        // A digest over content is the one thing that either matches or does not.
        EvidenceType::Artifact | EvidenceType::Wasm => EvidenceBasis::Decisive,
        EvidenceType::Deployment => match (record.transaction.is_some(), record.successful) {
            (true, Some(true)) if record.boundary.is_some() => EvidenceBasis::Decisive,
            // Without a successful outcome in a named boundary the record describes an
            // attempt, which is consistent with the effect without establishing it.
            _ => EvidenceBasis::Directional,
        },
        EvidenceType::Transaction => match record.successful {
            Some(true) if record.boundary.is_some() => EvidenceBasis::Decisive,
            // A failed transaction is decisive evidence of the failure and cannot be
            // decisive evidence of an effect, so it is recorded as directional support for
            // whatever claim it was collected for.
            _ => EvidenceBasis::Directional,
        },
        // An emitted event is an observation of what a contract did during one execution.
        // It is authoritative rather than decisive because nothing recomputes it.
        EvidenceType::Event => EvidenceBasis::Authoritative,
        // An attestation is a third party's statement: complete for the claim it states,
        // and the statement itself is what a reviewer checks rather than a computation.
        EvidenceType::Attestation => EvidenceBasis::Authoritative,
        EvidenceType::Observation => EvidenceBasis::Circumstantial,
        _ => EvidenceBasis::Uninterpretable,
    }
}

/// Assesses the confidence a set of supporting records supports.
///
/// Returns the minimum ordinal across the records' bases, capped below `VERIFIED` when any
/// record in the set is in declared conflict.
///
/// # Errors
///
/// Returns a failure when the supporting set is empty. The schema requires confidence to
/// cite evidence at every level including `UNKNOWN`, so a level computed over nothing is
/// not expressible rather than merely discouraged: "we do not know" is itself evidenced.
pub fn assess(
    supporting: &[&EvidenceRecord],
    contradicting: &[&EvidenceRecord],
) -> Result<Confidence> {
    if supporting.is_empty() {
        return Err(EvidenceFailure::NoSupportingEvidence {
            claim: "the claim being assessed".to_owned(),
            required: "at least one evidence record".to_owned(),
        }
        .into_error());
    }

    let mut level =
        ConfidenceLevel::weakest_of(supporting.iter().map(|record| basis_for(record).level()));
    let mut rationale = format!(
        "minimum ordinal across {} evidence record(s), as taxonomies/confidence-levels.yaml \
         requires; the weakest is {}: {}",
        supporting.len(),
        level.as_str(),
        supporting
            .iter()
            .map(|record| basis_for(record))
            .min()
            .unwrap_or(EvidenceBasis::Uninterpretable)
            .rationale()
    );
    if !contradicting.is_empty() {
        // The taxonomy: "If any evidence conflicts, the level MUST be lowered and the
        // verification status MUST be CONFLICTING rather than VERIFIED." The level is
        // lowered to the highest level below `VERIFIED`, and the contradiction is recorded
        // in its own field so that it is machine-detectable rather than reading as a
        // sentence in the rationale.
        let capped = level.weakest(ConfidenceLevel::HighConfidence);
        if capped != level {
            rationale = format!(
                "{}; capped at {} because {} record(s) declare a contradiction, and a \
                 contradicted claim must never be reported at the level that means the check \
                 passed",
                rationale,
                capped.as_str(),
                contradicting.len()
            );
        }
        level = capped;
        return Confidence::new(
            level,
            supporting.iter().map(|record| record.id.clone()).collect(),
            contradicting
                .iter()
                .map(|record| record.id.clone())
                .collect(),
        )
        .map(|confidence| confidence.with_rationale(rationale));
    }

    Confidence::new(
        level,
        supporting.iter().map(|record| record.id.clone()).collect(),
        Vec::new(),
    )
    .map(|confidence| confidence.with_rationale(rationale))
}

/// The verification status a set of records supports for a claim.
///
/// Independent of the confidence level by design: confidence describes how much evidence
/// there is and verification describes what it says. A claim can be `UNVERIFIED` at any
/// level, and a `CONFLICTING` claim can carry strong individual records.
///
/// The mapping, stated because the taxonomy leaves the boundary between `VERIFIED` and
/// `PARTIALLY_VERIFIED` to the producer:
///
/// * a declared contradiction is `CONFLICTING`, and takes precedence over everything;
/// * every required class present with at least one decisive record is `VERIFIED`;
/// * every required class present without a decisive record is `PARTIALLY_VERIFIED`,
///   because each component was checked by whoever produced it and none was re-checked
///   here;
/// * some required classes missing is `PARTIALLY_VERIFIED`, which is the taxonomy's own
///   example - "the source revision matched but the build toolchain identity could not be
///   established";
/// * no supporting record at all is `UNVERIFIED`, which "makes no statement about whether
///   the claim is true";
/// * nothing but uninterpretable records is `UNKNOWN`, which is "distinct from
///   `UNVERIFIED`, which means the claim is understood but unchecked".
#[must_use]
pub fn status_for(
    supporting: &[&EvidenceRecord],
    contradicting: &[&EvidenceRecord],
    missing_required: usize,
) -> VerificationStatus {
    if !contradicting.is_empty() {
        return VerificationStatus::Conflicting;
    }
    if supporting.is_empty() {
        return VerificationStatus::Unverified;
    }
    if supporting
        .iter()
        .all(|record| basis_for(record) == EvidenceBasis::Uninterpretable)
    {
        return VerificationStatus::Unknown;
    }
    let any_decisive = supporting
        .iter()
        .any(|record| basis_for(record).is_decisive());
    if missing_required > 0 {
        return VerificationStatus::PartiallyVerified;
    }
    if any_decisive {
        VerificationStatus::Verified
    } else {
        VerificationStatus::PartiallyVerified
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collector::{EvidenceClass, EvidenceRegistry};
    use amasario_core::{Digest, DigestAlgorithm, EvidenceType, LedgerSequence, TransactionHash};
    use amasario_provenance::ArtifactType;

    fn draft(class: &str, claim: &str) -> EvidenceRecord {
        EvidenceRecord::draft(
            "e-1",
            EvidenceClass::parse(class),
            claim,
            "2026-01-01T00:00:00Z",
        )
    }

    fn boundary() -> amasario_core::ObservationBoundary {
        amasario_core::ObservationBoundary::new(
            amasario_core::Network::new(
                "testnet",
                amasario_core::NetworkType::Testnet,
                "Test SDF Network ; September 2015",
            )
            .expect("a network"),
            LedgerSequence::new(1_000).expect("a real ledger"),
            "2026-01-01T00:00:00Z",
        )
    }

    fn artifact_record() -> EvidenceRecord {
        let mut record = draft("ARTIFACT", "the artifact content hashes to this digest");
        record.digest = Some(Digest::sha256_of(b"x"));
        record.artifact_type = Some(ArtifactType::Wasm);
        record
    }

    fn observation_record() -> EvidenceRecord {
        let mut record = draft("OBSERVATION", "the contract was present at this boundary");
        record.observation_note = Some("the contract's executable was readable".to_owned());
        record
    }

    #[test]
    fn a_digest_is_decisive_and_an_observation_is_circumstantial() {
        assert_eq!(basis_for(&artifact_record()), EvidenceBasis::Decisive);
        assert_eq!(
            basis_for(&observation_record()),
            EvidenceBasis::Circumstantial
        );
        assert_eq!(EvidenceBasis::Decisive.level(), ConfidenceLevel::Verified);
        assert_eq!(
            EvidenceBasis::Circumstantial.level(),
            ConfidenceLevel::LowConfidence
        );
    }

    #[test]
    fn a_source_record_is_decisive_only_on_an_immutable_revision() {
        let mut record = draft("SOURCE", "the artifact was built from this revision");
        record.repository = Some("https://example.invalid/r".to_owned());
        record.revision = Some("a".repeat(40));
        assert_eq!(basis_for(&record), EvidenceBasis::Decisive);
        record.revision = Some("main".to_owned());
        record.revision_kind = None;
        assert_eq!(basis_for(&record), EvidenceBasis::Directional);
        record.revision_kind = Some(RevisionKind::Tag);
        assert_eq!(
            basis_for(&record),
            EvidenceBasis::Directional,
            "a tag is mutable in practice and cannot be re-checked"
        );
    }

    #[test]
    fn an_attestation_is_authoritative_and_never_verified_confidence() {
        let mut record = draft(
            "ATTESTATION",
            "the artifact was built from revision abc1234",
        );
        record.attestation_id = Some("att-1".to_owned());
        record.attested_claim = Some("the artifact was built from revision abc1234".to_owned());
        assert_eq!(basis_for(&record), EvidenceBasis::Authoritative);
        assert_eq!(
            basis_for(&record).level(),
            ATTESTATION_CONFIDENCE_CEILING,
            "the ceiling is a statement about the evidence, not about the issuer"
        );
        assert_ne!(basis_for(&record).level(), ConfidenceLevel::Verified);
    }

    #[test]
    fn a_deployment_is_decisive_only_with_a_successful_outcome_in_a_named_boundary() {
        let mut record = draft(
            "DEPLOYMENT",
            "the executable became the contract at this address",
        );
        record.transaction =
            Some(TransactionHash::new("ab".repeat(32)).expect("a transaction hash"));
        assert_eq!(
            basis_for(&record),
            EvidenceBasis::Directional,
            "an attempt is consistent with the effect without establishing it"
        );
        record.successful = Some(true);
        assert_eq!(basis_for(&record), EvidenceBasis::Directional);
        record.boundary = Some(boundary());
        assert_eq!(basis_for(&record), EvidenceBasis::Decisive);
    }

    #[test]
    fn an_unrecognised_class_is_uninterpretable() {
        let mut record = EvidenceRecord::draft(
            "e-1",
            EvidenceClass::parse("FUTURE_CLASS"),
            "a claim from a producer this version does not know",
            "2026-01-01T00:00:00Z",
        );
        record.observation_note = Some("unused".to_owned());
        assert_eq!(basis_for(&record), EvidenceBasis::Uninterpretable);
        assert_eq!(basis_for(&record).level(), ConfidenceLevel::Unknown);
    }

    #[test]
    fn the_aggregate_is_the_weakest_link_and_says_which_one_it_was() {
        let strong = artifact_record();
        let weak = observation_record();
        let confidence = assess(&[&strong, &weak], &[]).expect("a confidence");
        assert_eq!(confidence.level, ConfidenceLevel::LowConfidence);
        let rationale = confidence.rationale.expect("a rationale");
        assert!(rationale.contains("minimum ordinal"), "got: {rationale}");
        assert!(rationale.contains("weakest"), "got: {rationale}");
        assert_eq!(confidence.evidence.len(), 2);
        assert!(confidence.contradicting_evidence.is_empty());
    }

    #[test]
    fn a_contradicted_claim_can_never_be_verified_confidence() {
        let strong = artifact_record();
        let mut other = artifact_record();
        other.id = "e-2".to_owned();
        other.contradicts.push("e-1".to_owned());
        // Two decisive records would otherwise aggregate to VERIFIED.
        assert_eq!(
            assess(&[&strong, &other], &[]).expect("a confidence").level,
            ConfidenceLevel::Verified
        );
        let confidence = assess(&[&strong], &[&other]).expect("a confidence");
        assert_ne!(confidence.level, ConfidenceLevel::Verified);
        assert_eq!(confidence.contradicting_evidence, vec!["e-2".to_owned()]);
        assert!(
            confidence
                .rationale
                .as_deref()
                .is_some_and(|rationale| rationale.contains("capped")),
            "the lowering must be explained"
        );
        assert!(confidence.is_contradicted());
    }

    #[test]
    fn a_level_over_no_evidence_is_not_expressible() {
        let error = assess(&[], &[]).expect_err("confidence never replaces evidence");
        assert!(error.to_string().contains("cites no evidence"));
    }

    #[test]
    fn the_verification_status_is_independent_of_the_level() {
        let strong = artifact_record();
        let weak = observation_record();
        assert_eq!(status_for(&[&strong], &[], 0), VerificationStatus::Verified);
        assert_eq!(
            status_for(&[&strong], &[], 1),
            VerificationStatus::PartiallyVerified,
            "one component unchecked is a partial verification, not a failure"
        );
        assert_eq!(
            status_for(&[&weak], &[], 0),
            VerificationStatus::PartiallyVerified,
            "an observation checks nothing decisively"
        );
        assert_eq!(status_for(&[], &[], 2), VerificationStatus::Unverified);
        assert_eq!(
            status_for(&[&strong], &[&weak], 0),
            VerificationStatus::Conflicting
        );
        assert!(
            !status_for(&[&strong], &[&weak], 0).is_affirmation(),
            "a conflict is never an affirmation, whatever the evidence looks like"
        );
    }

    #[test]
    fn nothing_but_uninterpretable_records_is_unknown_rather_than_unverified() {
        let mut foreign = EvidenceRecord::draft(
            "e-1",
            EvidenceClass::parse("FUTURE_CLASS"),
            "a claim from a producer this version does not know",
            "2026-01-01T00:00:00Z",
        );
        foreign.observation_note = Some("unused".to_owned());
        assert_eq!(status_for(&[&foreign], &[], 1), VerificationStatus::Unknown);
        assert!(status_for(&[&foreign], &[], 1).is_inconclusive());
    }

    #[test]
    fn the_registrys_own_classes_are_assessable() {
        // The independence of the modules is checked by handing `assess` a record that
        // came through the registry rather than one built inline, so a field that only the
        // registry sets cannot be missed.
        let mut registry = EvidenceRegistry::new();
        registry.add(artifact_record()).expect("stored");
        let record = registry.get("e-1").expect("the record");
        assert_eq!(basis_for(record), EvidenceBasis::Decisive);
        assert_eq!(
            basis_for(record).level().ordinal(),
            ConfidenceLevel::Verified.ordinal()
        );
        assert_eq!(
            EvidenceBasis::Uninterpretable.level().ordinal(),
            0,
            "UNKNOWN is the taxonomy's ordinal zero, not an absence of a value"
        );
        let _ = DigestAlgorithm::Sha256;
        let _ = EvidenceType::Artifact;
    }
}
