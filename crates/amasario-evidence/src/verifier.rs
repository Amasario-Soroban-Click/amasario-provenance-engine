//! Claim evaluation: what a collected set of evidence establishes about one claim.
//!
//! # What this asks that the chain verifier does not
//!
//! `amasario-provenance` verifies a *chain*: it walks source to revision to build to
//! artifact to executable to deployment and returns one outcome for the whole walk. This
//! module answers the narrower and reusable question the rest of the engine asks hundreds
//! of times: given a claim and the evidence collected for it, what does that evidence
//! establish. Every dependency edge, every impact finding and every provenance link cites
//! evidence, and each of those citations has the same three questions - is the evidence
//! present, does it conflict, and which of the claim's components went unchecked.
//!
//! # Why the outcome and the confidence are separate fields
//!
//! `taxonomies/verification-statuses.yaml` draws the distinction the report layer depends
//! on: "confidence describes how much evidence there is, verification describes what that
//! evidence says about the claim." A claim can be `UNVERIFIED` while its evidence is strong
//! - nothing has checked it yet - and a `CONFLICTING` claim can rest on individually
//!   decisive records. Merging the two would make the one status a reader most needs to
//!   see, `CONFLICTING`, expressible only as a low confidence, which is not the same
//!   statement.
//!
//! # Why a claim has to name the classes it requires
//!
//! Without that, "unchecked" and "checked and insufficient" are indistinguishable. A claim
//! that requires `BUILD` evidence and has only `SOURCE` is not unverified: something was
//! checked, and a specific component was not. [`Evaluation::missing`] names them, so a
//! report can say which step to take rather than that the claim is unsupported.
//!
//! # Why the attestation record is built here
//!
//! [`from_attestation`] turns an [`Attestation`] into `ATTESTATION` evidence, and it lives
//! beside the evaluation because the rule it has to satisfy is the one evaluated here: an
//! attestation "supports the specific claim it states and nothing broader". That rule is
//! enforced in two places and both are needed. This function copies the attestation's own
//! words into the record's `claim`, so the record cannot state more than the issuer did;
//! [`crate::collector::EvidenceRecord::failures`] then re-checks the copied claim against
//! `attested_claim`, so a record edited after construction is caught. Building the record
//! anywhere else would separate the constraint from the code that depends on it.

use amasario_core::{Confidence, EvidenceType, Result, VerificationStatus};
use amasario_dependency::EvidenceRef;
use amasario_provenance::Attestation;

use crate::collector::{EvidenceClass, EvidenceRecord, EvidenceRegistry};
use crate::confidence::{assess, basis_for, status_for};
use crate::errors::EvidenceFailure;

/// The shortest a claim's statement may be.
const MINIMUM_STATEMENT_LENGTH: usize = 8;

/// One claim to evaluate against collected evidence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Claim {
    /// The claim's identifier, which evidence cites to declare support.
    pub id: String,
    /// The claim, stated as a sentence.
    pub statement: String,
    /// The classes of evidence the claim needs in order to be established.
    pub requires: Vec<EvidenceType>,
}

impl Claim {
    /// Builds a claim, requiring at least one class.
    ///
    /// A claim that requires nothing is not evaluable: it would come back `VERIFIED` on
    /// any evidence at all, including evidence about something else, which is the shape of
    /// answer this module exists to prevent.
    ///
    /// # Errors
    ///
    /// Returns a failure when the statement is too short to be one, or when no class is
    /// required.
    pub fn new(
        id: impl Into<String>,
        statement: impl Into<String>,
        requires: Vec<EvidenceType>,
    ) -> Result<Self> {
        let statement = statement.into();
        let id = id.into();
        let length = statement.chars().count();
        if length < MINIMUM_STATEMENT_LENGTH {
            return Err(EvidenceFailure::ClaimNotStated {
                evidence: id,
                length,
            }
            .into_error());
        }
        if requires.is_empty() {
            return Err(EvidenceFailure::NoSupportingEvidence {
                claim: id,
                required: "at least one evidence class".to_owned(),
            }
            .into_error());
        }
        Ok(Self {
            id,
            statement,
            requires,
        })
    }

    /// Whether every class the claim requires is present in a set of records.
    #[must_use]
    fn missing_in(&self, records: &[&EvidenceRecord]) -> Vec<EvidenceClass> {
        self.requires
            .iter()
            .filter(|required| {
                !records
                    .iter()
                    .any(|record| record.class.recognised() == Some(**required))
            })
            .map(|required| EvidenceClass::Recognised(*required))
            .collect()
    }

    /// The records in a set that are cited by, or that cite, a conflict elsewhere in the
    /// registry.
    ///
    /// A record involved in an unresolvable contradiction - one whose declared opposite is
    /// absent - counts here, and that is deliberate. The producer declared a conflict; the
    /// engine could not find the other record; reporting the claim as verified because one
    /// side of a declared conflict is missing would be the most damaging outcome available.
    fn conflicted<'a>(
        records: &[&'a EvidenceRecord],
        registry: &EvidenceRegistry,
    ) -> Vec<&'a EvidenceRecord> {
        let mut conflicted: Vec<&EvidenceRecord> = Vec::new();
        for record in records {
            let declares_or_receives = registry
                .contradictions()
                .iter()
                .any(|(left, right)| left.id == record.id || right.id == record.id);
            let declares_unresolvable = registry
                .unresolvable_contradictions()
                .iter()
                .any(|failure| matches!(
                    failure,
                    EvidenceFailure::ContradictionUnresolvable { evidence, .. } if evidence == &record.id
                ));
            let cited_by_another = registry
                .records()
                .iter()
                .any(|other| other.contradicts.iter().any(|cited| cited == &record.id));
            if declares_or_receives || declares_unresolvable || cited_by_another {
                conflicted.push(record);
            }
        }
        conflicted.sort_by(|left, right| left.id.cmp(&right.id));
        conflicted.dedup_by(|left, right| left.id == right.id);
        conflicted
    }
}

/// What an evaluation found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Evaluation {
    /// The claim's identifier.
    pub id: String,
    /// The claim, restated so that a report does not have to look it up.
    pub statement: String,
    /// What the evidence says about the claim.
    pub verification: VerificationStatus,
    /// How much evidence there is, with its citations.
    pub confidence: Confidence,
    /// The records that support the claim.
    pub supporting: Vec<EvidenceRef>,
    /// The records that contradict it.
    pub contradicting: Vec<EvidenceRef>,
    /// The classes the claim required that no record supplied.
    pub missing: Vec<EvidenceClass>,
    /// The evidence class of the strongest record supporting the claim, where any exists.
    pub strongest_basis: Option<&'static str>,
    /// Why the outcome is the one it is.
    pub rationale: String,
}

impl Evaluation {
    /// Whether every class the claim required was present.
    #[must_use]
    pub const fn is_complete(&self) -> bool {
        self.missing.is_empty()
    }

    /// Whether the claim was established.
    ///
    /// `true` only for `VERIFIED`. `PARTIALLY_VERIFIED` is deliberately excluded: a reader
    /// asking this question wants to know whether the claim was fully checked, and the
    /// answer to "was some of it checked" is a different question with a different answer.
    #[must_use]
    pub const fn is_established(&self) -> bool {
        matches!(self.verification, VerificationStatus::Verified)
    }

    /// Whether the evidence in hand contradicts the claim.
    #[must_use]
    pub const fn is_contradicted(&self) -> bool {
        matches!(self.verification, VerificationStatus::Conflicting)
    }

    /// Whether the evaluation reached no conclusion at all.
    #[must_use]
    pub const fn is_inconclusive(&self) -> bool {
        self.verification.is_inconclusive()
    }

    /// The evaluation as one line, for a report.
    #[must_use]
    pub fn render(&self) -> String {
        let missing = if self.missing.is_empty() {
            String::new()
        } else {
            format!(
                ", missing {}",
                self.missing
                    .iter()
                    .map(EvidenceClass::as_str)
                    .collect::<Vec<_>>()
                    .join("+")
            )
        };
        format!(
            "{}: {} at {}{missing}",
            self.id,
            self.verification.as_str(),
            self.confidence.level.as_str()
        )
    }
}

/// Evaluates one claim against a registry.
///
/// # Errors
///
/// Returns a failure when the claim cites no evidence at all, because a confidence level
/// cannot exist without evidence and this module will not invent one. A claim with
/// evidence that fails to establish it is not an error: it is an evaluation whose
/// verification is `UNVERIFIED`, `PARTIALLY_VERIFIED` or `UNKNOWN`.
pub fn evaluate(claim: &Claim, registry: &EvidenceRegistry) -> Result<Evaluation> {
    let mut supporting: Vec<&EvidenceRecord> = registry.supporting(&claim.id);
    // Evidence of a required class counts as support for the claim even when its `supports`
    // list does not name it, because a collector that recorded the class the claim requires
    // for the entity the claim is about has done the work: requiring a second declaration
    // would make the `supports` field a formality rather than a statement.
    for required in &claim.requires {
        for record in registry.of_class(*required) {
            if !supporting.iter().any(|existing| existing.id == record.id)
                && record.supports.is_empty()
            {
                supporting.push(record);
            }
        }
    }
    supporting.sort_by(|left, right| left.id.cmp(&right.id));
    supporting.dedup_by(|left, right| left.id == right.id);

    if supporting.is_empty() {
        return Err(EvidenceFailure::NoSupportingEvidence {
            claim: claim.id.clone(),
            required: claim
                .requires
                .iter()
                .map(|class| class.as_str())
                .collect::<Vec<_>>()
                .join(", "),
        }
        .into_error());
    }

    let missing = claim.missing_in(&supporting);
    let contradicting = Claim::conflicted(&supporting, registry);
    let verification = status_for(&supporting, &contradicting, missing.len());
    let confidence = assess(&supporting, &contradicting)?;
    let strongest = supporting
        .iter()
        .map(|record| basis_for(record))
        .max()
        .map(|basis| basis.as_str());

    let mut rationale = format!(
        "{} of {} required class(es) were present across {} record(s), and {} record(s) were \
         found in declared conflict",
        claim.requires.len() - missing.len(),
        claim.requires.len(),
        supporting.len(),
        contradicting.len()
    );
    if !missing.is_empty() {
        rationale = format!(
            "{rationale}; the claim's components that went unchecked are {}",
            missing
                .iter()
                .map(EvidenceClass::as_str)
                .collect::<Vec<_>>()
                .join(", ")
        );
    }

    Ok(Evaluation {
        id: claim.id.clone(),
        statement: claim.statement.clone(),
        verification,
        confidence,
        supporting: supporting
            .iter()
            .map(|record| record.reference())
            .collect::<Result<Vec<_>>>()?,
        contradicting: contradicting
            .iter()
            .map(|record| record.reference())
            .collect::<Result<Vec<_>>>()?,
        missing,
        strongest_basis: strongest,
        rationale,
    })
}

/// A record for an attestation, cited for the claim the attestation itself states.
///
/// The record's claim is the attestation's own words, copied verbatim, and not a summary
/// supplied by the caller. That is the whole point of the function: a caller cannot widen
/// an attestation of a build into an attestation of safety, because it never gets to write
/// the claim. Callers that want to connect the record to one of their own claims do it
/// through [`crate::collector::EvidenceRecord::supporting`] and the [`Claim::id`], which
/// replaces an attestation of the subject with the record that supports a claim about it -
/// a substitution only valid when the attestation states that claim, which
/// [`crate::collector::EvidenceRecord::failures`] checks.
///
/// # Errors
///
/// Returns a failure when the attestation cannot be cited - an absent, unchecked or
/// invalid signature - and a provenance error when the resulting record fails its class.
/// The first is a refusal rather than a gap: an unchecked signature does not become
/// checked by collecting more evidence about the artifact, and recording the attestation
/// as though it were support would leave a report unable to tell the two apart.
pub fn from_attestation(
    attestation: &Attestation,
    id: impl Into<String>,
    observed_at: impl Into<String>,
) -> Result<EvidenceRecord> {
    if !attestation.can_be_cited() {
        return Err(EvidenceFailure::AttestationNotCitable {
            attestation: attestation.id.clone(),
            issuer: attestation.issuer.clone(),
            state: attestation.signature.as_str().to_owned(),
        }
        .into_error());
    }

    let claim = attestation.claim.clone();
    let mut record = EvidenceRecord::draft(
        id,
        EvidenceClass::parse("ATTESTATION"),
        claim.clone(),
        observed_at,
    );
    record.attestation_id = Some(attestation.id.clone());
    // The same text in both fields, because the scope rule compares them. Anything else
    // would be the record claiming something the issuer did not.
    record.attested_claim = Some(claim);
    record.validate()?;
    Ok(record)
}

/// Evaluates every claim against a registry, in the order the claims were given.
///
/// # Errors
///
/// Returns the first failure. A claim that cites no evidence fails rather than being
/// skipped, because a skipped claim and a claim with no evidence are the same thing to a
/// reader and only one of them is the truth.
pub fn evaluate_all(claims: &[Claim], registry: &EvidenceRegistry) -> Result<Vec<Evaluation>> {
    let mut evaluations = Vec::with_capacity(claims.len());
    for claim in claims {
        evaluations.push(evaluate(claim, registry)?);
    }
    Ok(evaluations)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::artifact;
    use crate::collector::EvidenceClass as Class;
    use amasario_core::{Digest, LedgerSequence, Network, NetworkType};
    use amasario_provenance::ArtifactType;

    fn boundary() -> amasario_core::ObservationBoundary {
        amasario_core::ObservationBoundary::new(
            Network::new(
                "testnet",
                NetworkType::Testnet,
                "Test SDF Network ; September 2015",
            )
            .expect("a network"),
            LedgerSequence::new(1_000).expect("a real ledger"),
            "2026-01-01T00:00:00Z",
        )
    }

    fn observation(id: &str, note: &str) -> EvidenceRecord {
        let mut record = EvidenceRecord::draft(
            id,
            Class::parse("OBSERVATION"),
            "something was observed at this boundary",
            "2026-01-01T00:00:00Z",
        );
        record.observation_note = Some(note.to_owned());
        record
    }

    fn registry_with(records: Vec<EvidenceRecord>) -> EvidenceRegistry {
        let mut registry = EvidenceRegistry::new();
        registry.extend(records).expect("the records are citable");
        registry
    }

    fn claim(requires: Vec<EvidenceType>) -> Claim {
        Claim::new(
            "c-1",
            "the artifact at this digest is the one the contract hosts",
            requires,
        )
        .expect("a claim")
    }

    #[test]
    fn a_claim_that_requires_nothing_is_refused() {
        let error = Claim::new("c-1", "the contract hosts the artifact", Vec::new())
            .expect_err("a claim requiring nothing would be verified by anything");
        assert!(error.to_string().contains("at least one evidence class"));
        let error = Claim::new("c-1", "short", vec![EvidenceType::Artifact])
            .expect_err("a five-character statement is not a claim");
        assert!(error.to_string().contains("5-character claim"));
    }

    #[test]
    fn a_decisive_record_over_the_required_class_verifies_the_claim() {
        let digest = Digest::sha256_of(b"module");
        let mut support = artifact::from_artifact(
            digest,
            ArtifactType::Wasm,
            "e-artifact",
            "2026-01-01T00:00:00Z",
        )
        .expect("a record");
        support.supports.push("c-1".to_owned());
        let registry = registry_with(vec![support]);
        let evaluation =
            evaluate(&claim(vec![EvidenceType::Artifact]), &registry).expect("an evaluation");
        assert_eq!(evaluation.verification, VerificationStatus::Verified);
        assert_eq!(
            evaluation.confidence.level,
            amasario_core::ConfidenceLevel::Verified
        );
        assert!(evaluation.is_established());
        assert!(evaluation.is_complete());
        assert_eq!(evaluation.supporting.len(), 1);
        assert_eq!(evaluation.strongest_basis, Some("DECISIVE"));
        assert!(evaluation.render().contains("VERIFIED"));
    }

    fn source_record(id: &str) -> EvidenceRecord {
        let mut record = EvidenceRecord::draft(
            id,
            Class::parse("SOURCE"),
            "the artifact was built from this repository at this revision",
            "2026-01-01T00:00:00Z",
        );
        record.repository = Some("https://github.com/example/contract".to_owned());
        record.revision = Some("a".repeat(40));
        record
    }

    fn build_record(id: &str) -> EvidenceRecord {
        let mut record = EvidenceRecord::draft(
            id,
            Class::parse("BUILD"),
            "the artifact was produced by this recorded build",
            "2026-01-01T00:00:00Z",
        );
        record.toolchain = Some("rustc 1.93.0".to_owned());
        record.configuration_digest = Some(Digest::sha256_of(b"config"));
        record
    }

    #[test]
    fn a_missing_class_makes_the_claim_partially_verified_and_names_what_is_missing() {
        // The taxonomy's own example: the revision matched and the build toolchain could
        // not be established.
        let registry = registry_with(vec![source_record("e-1")]);
        let evaluation = evaluate(
            &claim(vec![EvidenceType::Source, EvidenceType::Build]),
            &registry,
        )
        .expect("an evaluation");
        assert_eq!(
            evaluation.verification,
            VerificationStatus::PartiallyVerified
        );
        assert!(!evaluation.is_established());
        assert_eq!(evaluation.missing.len(), 1);
        assert_eq!(evaluation.missing[0].as_str(), "BUILD");
        assert!(evaluation.rationale.contains("went unchecked"));
        assert!(evaluation.render().contains("missing BUILD"));
        // Supplying the missing class completes the check.
        let registry = registry_with(vec![source_record("e-1"), build_record("e-2")]);
        let evaluation = evaluate(
            &claim(vec![EvidenceType::Source, EvidenceType::Build]),
            &registry,
        )
        .expect("an evaluation");
        assert!(evaluation.is_complete());
        assert_eq!(evaluation.verification, VerificationStatus::Verified);
    }

    #[test]
    fn a_claim_with_no_evidence_at_all_fails_rather_than_being_verified_by_nothing() {
        let registry = registry_with(vec![observation("e-1", "an unrelated observation")]);
        let error = evaluate(&claim(vec![EvidenceType::Artifact]), &registry)
            .expect_err("no evidence for the claim is a failure, not an empty evaluation");
        let message = error.to_string();
        assert!(message.contains("c-1"), "got: {message}");
        assert!(message.contains("ARTIFACT"), "got: {message}");
    }

    #[test]
    fn a_declared_conflict_produces_conflicting_and_a_lowered_confidence() {
        let digest = Digest::sha256_of(b"module");
        let mut first =
            artifact::from_artifact(digest, ArtifactType::Wasm, "e-1", "2026-01-01T00:00:00Z")
                .expect("a record");
        first.supports.push("c-1".to_owned());
        let mut second = artifact::from_artifact(
            Digest::sha256_of(b"other module"),
            ArtifactType::Wasm,
            "e-2",
            "2026-01-01T00:00:00Z",
        )
        .expect("a record");
        second.supports.push("c-1".to_owned());
        second.contradicts.push("e-1".to_owned());
        let registry = registry_with(vec![first, second]);
        let evaluation =
            evaluate(&claim(vec![EvidenceType::Artifact]), &registry).expect("an evaluation");
        assert_eq!(evaluation.verification, VerificationStatus::Conflicting);
        assert!(evaluation.is_contradicted());
        assert!(!evaluation.is_established());
        assert_ne!(
            evaluation.confidence.level,
            amasario_core::ConfidenceLevel::Verified,
            "a contradicted claim is never reported at the level that means the check passed"
        );
        assert_eq!(evaluation.contradicting.len(), 2);
        assert!(evaluation.confidence.is_contradicted());
    }

    #[test]
    fn a_conflict_whose_other_side_is_absent_is_still_reported_as_a_conflict() {
        let mut support = observation("e-1", "the contract was present at the boundary");
        support.supports.push("c-1".to_owned());
        support.contradicts.push("e-missing".to_owned());
        let registry = registry_with(vec![support]);
        let evaluation =
            evaluate(&claim(vec![EvidenceType::Observation]), &registry).expect("an evaluation");
        assert_eq!(
            evaluation.verification,
            VerificationStatus::Conflicting,
            "a declared conflict that cannot be evaluated must not come back verified"
        );
    }

    #[test]
    fn an_uninterpretable_record_makes_the_claim_unknown_rather_than_unverified() {
        let mut foreign = EvidenceRecord::draft(
            "e-1",
            Class::parse("FUTURE_CLASS"),
            "a claim from a producer this version does not know",
            "2026-01-01T00:00:00Z",
        );
        foreign.supports.push("c-1".to_owned());
        let mut registry = EvidenceRegistry::new();
        registry
            .add(foreign)
            .expect("stored despite being unrecognised");
        // The claim requires a class the record does not supply, so the class is missing.
        let evaluation =
            evaluate(&claim(vec![EvidenceType::Observation]), &registry).expect("an evaluation");
        assert_eq!(evaluation.verification, VerificationStatus::Unknown);
        assert!(evaluation.is_inconclusive());
        assert_eq!(evaluation.strongest_basis, Some("UNINTERPRETABLE"));
    }

    #[test]
    fn every_claim_in_a_list_is_evaluated_in_order() {
        let digest = Digest::sha256_of(b"module");
        let mut support =
            artifact::from_artifact(digest, ArtifactType::Wasm, "e-1", "2026-01-01T00:00:00Z")
                .expect("a record");
        support.supports.push("c-1".to_owned());
        // The build record names no claim, so it is associated with the claim that requires
        // its class, which is the case a collector produces when it records what it found
        // before it knows what will be asked of it.
        let registry = registry_with(vec![support, build_record("e-2")]);
        let claims = vec![
            claim(vec![EvidenceType::Artifact]),
            Claim::new(
                "c-2",
                "the build recorded the toolchain that produced the artifact",
                vec![EvidenceType::Build],
            )
            .expect("a claim"),
        ];
        let evaluations = evaluate_all(&claims, &registry).expect("evaluations");
        assert_eq!(evaluations.len(), 2);
        assert_eq!(evaluations[0].id, "c-1");
        // The artifact record is a digest at a known class, so it is deterministically
        // comparable to the claim and the claim is VERIFIED.
        assert_eq!(evaluations[0].verification, VerificationStatus::Verified);
        assert_eq!(evaluations[1].id, "c-2");
        assert_eq!(evaluations[1].strongest_basis, Some("AUTHORITATIVE"));
        // The build record is authoritative and not decisive: nothing in the Amasario
        // record re-derives the toolchain that ran. `taxonomies/verification-statuses.yaml`
        // reserves VERIFIED for a claim with "at least one evidence record that is
        // deterministically comparable to the claim", so a build record supports
        // HIGH_CONFIDENCE and PARTIALLY_VERIFIED rather than VERIFIED.
        assert_eq!(
            evaluations[1].verification,
            VerificationStatus::PartiallyVerified
        );
        assert_eq!(
            evaluations[1].confidence.level,
            amasario_core::ConfidenceLevel::HighConfidence
        );
        assert!(!evaluations[1].is_established());
        assert!(evaluations[1].is_complete(), "nothing was missing");
    }

    #[test]
    fn a_claim_with_no_evidence_makes_the_whole_evaluation_fail() {
        let digest = Digest::sha256_of(b"module");
        let mut support =
            artifact::from_artifact(digest, ArtifactType::Wasm, "e-1", "2026-01-01T00:00:00Z")
                .expect("a record");
        support.supports.push("c-1".to_owned());
        let registry = registry_with(vec![support]);
        let claims = vec![
            claim(vec![EvidenceType::Artifact]),
            Claim::new(
                "c-2",
                "the build recorded the toolchain that produced the artifact",
                vec![EvidenceType::Build],
            )
            .expect("a claim"),
        ];
        // Nothing in the registry is build evidence, so the second claim cannot be
        // evaluated at all, and that is reported rather than skipped.
        let error = evaluate_all(&claims, &registry)
            .expect_err("a claim with no evidence fails rather than being skipped");
        assert!(error.to_string().contains("c-2"), "got: {error}");
        let _ = boundary();
    }
}
