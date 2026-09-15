//! Attestations: third-party claims about a subject, and what they are worth.
//!
//! # An attestation is evidence, not proof
//!
//! An issuer asserting that an artifact corresponds to a source revision is a
//! meaningful input - it is one of the specification's bases, `ATTESTED` - and it is
//! not the same kind of fact as a digest match. A digest match is arithmetic; an
//! attestation is a person or service's statement, and the two differ in what a
//! consumer should do about them.
//!
//! Three consequences follow, and each is enforced rather than documented.
//!
//! **A signature that was not verified makes the attestation unusable as support.**
//! [`SignatureState`] records whether a signature was present, whether it was
//! checked, and whether it held. Only [`SignatureState::PresentVerified`] yields a
//! citable [`Confidence`]; the others are refused, because citing an unchecked
//! signature is indistinguishable from citing a checked one once the record reaches a
//! report.
//!
//! **An attestation never reaches `VERIFIED` confidence on its own.** The ceiling is
//! `HIGH_CONFIDENCE`, matching `Basis::Attested::confidence_ceiling` in
//! `amasario-core`. `VERIFIED` confidence in this engine means the decisive evidence
//! was checked successfully, and a third party's statement is the thing being checked,
//! not the check.
//!
//! **An invalid signature refutes the attestation.** A signature that was checked and
//! did not hold means the issuer's claim is not supported by their own signature; that
//! is a contradiction the verification layer must see, not a weaker attestation.

use std::fmt;
use std::str::FromStr;

use amasario_core::{
    Basis, Confidence, ConfidenceLevel, EngineError, EntityRef, Result, VerificationStatus,
};
use serde::{Deserialize, Serialize};

use crate::errors::ProvenanceFailure;

/// The highest confidence an attestation can support on its own.
///
/// A constant rather than a number written at each use, and deliberately equal to
/// `Basis::Attested::confidence_ceiling()`. An attestation is not `VERIFIED`
/// confidence: that level means the decisive evidence was checked successfully, and a
/// third party's statement is the thing being checked rather than the check.
pub const ATTESTATION_CONFIDENCE_CEILING: ConfidenceLevel = ConfidenceLevel::HighConfidence;

/// What happened to an attestation's signature.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
#[non_exhaustive]
pub enum SignatureState {
    /// The attestation carries no signature.
    NotPresent,
    /// A signature is present and was not checked.
    ///
    /// The common case for a record that was read from a document: the engine can see
    /// the signature and has not verified it, and the distinction between that and a
    /// verified one has to survive into the record.
    PresentUnverified,
    /// A signature was checked and held.
    PresentVerified,
    /// A signature was checked and did not hold.
    ///
    /// A refutation of the attestation, not a weaker attestation: the issuer's own
    /// signature does not support their claim.
    Invalid,
}

impl SignatureState {
    /// The stable wire name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::NotPresent => "NOT_PRESENT",
            Self::PresentUnverified => "PRESENT_UNVERIFIED",
            Self::PresentVerified => "PRESENT_VERIFIED",
            Self::Invalid => "INVALID",
        }
    }

    /// Whether an attestation in this state may be cited as support.
    #[must_use]
    pub const fn is_citable(self) -> bool {
        matches!(self, Self::PresentVerified)
    }

    /// Whether an attestation in this state is refuted.
    #[must_use]
    pub const fn is_refutation(self) -> bool {
        matches!(self, Self::Invalid)
    }

    /// Whether the signature's validity is undetermined.
    #[must_use]
    pub const fn is_inconclusive(self) -> bool {
        matches!(self, Self::NotPresent | Self::PresentUnverified)
    }

    /// The verification status this state implies for the attestation's claim.
    #[must_use]
    pub const fn status(self) -> VerificationStatus {
        match self {
            Self::PresentVerified => VerificationStatus::Verified,
            Self::Invalid => VerificationStatus::Conflicting,
            Self::PresentUnverified | Self::NotPresent => VerificationStatus::Unverified,
        }
    }
}

impl fmt::Display for SignatureState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for SignatureState {
    type Err = EngineError;

    fn from_str(value: &str) -> Result<Self> {
        match value {
            "NOT_PRESENT" => Ok(Self::NotPresent),
            "PRESENT_UNVERIFIED" => Ok(Self::PresentUnverified),
            "PRESENT_VERIFIED" => Ok(Self::PresentVerified),
            "INVALID" => Ok(Self::Invalid),
            other => Err(EngineError::Validation {
                path: "/attestation/signature".to_owned(),
                detail: format!("unrecognised signature state {other:?}"),
            }),
        }
    }
}

/// A third-party claim about a subject.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Attestation {
    /// Who made the claim.
    pub issuer: String,
    /// What the claim is about.
    pub subject: EntityRef,
    /// What is being claimed, in the issuer's words.
    pub claim: String,
    /// What happened to the signature.
    pub signature: SignatureState,
    /// How the signature was checked, when it was.
    ///
    /// Required when the signature was verified. A verification whose method was not
    /// recorded cannot be re-examined, which makes it a claim about a check rather
    /// than a record of one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub verification_method: Option<String>,
    /// When the attestation was issued, as an RFC 3339 timestamp.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub issued_at: Option<String>,
    /// When the attestation expires, as an RFC 3339 timestamp.
    ///
    /// Recorded rather than checked: the engine has no clock of its own, and an
    /// attestation whose expiry it evaluated against a wall clock would make two runs
    /// of the same analysis differ.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<String>,
    /// Identifiers of the evidence records that support the attestation's existence.
    pub evidence: Vec<String>,
}

impl Attestation {
    /// Records an attestation.
    ///
    /// # Errors
    ///
    /// Returns a provenance error when the issuer is empty, and a validation error
    /// when the signature was verified but no verification method was recorded, or
    /// when no evidence is cited.
    pub fn new(
        issuer: impl Into<String>,
        subject: EntityRef,
        claim: impl Into<String>,
        signature: SignatureState,
        verification_method: Option<String>,
        evidence: Vec<String>,
    ) -> Result<Self> {
        let issuer = issuer.into();
        if issuer.is_empty() {
            return Err(ProvenanceFailure::AttestationUnverified {
                issuer,
                detail: "a claim with no issuer names nobody".to_owned(),
            }
            .into_error());
        }
        if signature == SignatureState::PresentVerified && verification_method.is_none() {
            return Err(EngineError::Validation {
                path: "/attestation/verificationMethod".to_owned(),
                detail: "a verified signature must record how it was verified; without the method \
                         the verification cannot be re-examined"
                    .to_owned(),
            });
        }
        if evidence.is_empty() {
            return Err(EngineError::Validation {
                path: "/attestation/evidence".to_owned(),
                detail: "an attestation must cite the evidence of its existence".to_owned(),
            });
        }
        Ok(Self {
            issuer,
            subject,
            claim: claim.into(),
            signature,
            verification_method,
            issued_at: None,
            expires_at: None,
            evidence,
        })
    }

    /// Records an attestation whose signature was checked and held.
    ///
    /// # Errors
    ///
    /// Returns a validation error when no verification method or evidence is given.
    pub fn verified(
        issuer: impl Into<String>,
        subject: EntityRef,
        claim: impl Into<String>,
        verification_method: impl Into<String>,
        evidence: Vec<String>,
    ) -> Result<Self> {
        Self::new(
            issuer,
            subject,
            claim,
            SignatureState::PresentVerified,
            Some(verification_method.into()),
            evidence,
        )
    }

    /// Records when the attestation was issued.
    #[must_use]
    pub fn issued_at(mut self, timestamp: impl Into<String>) -> Self {
        self.issued_at = Some(timestamp.into());
        self
    }

    /// Records when the attestation expires.
    #[must_use]
    pub fn expires_at(mut self, timestamp: impl Into<String>) -> Self {
        self.expires_at = Some(timestamp.into());
        self
    }

    /// The basis an attestation rests on.
    ///
    /// Always `ATTESTED`, and stated here rather than at each use so that an
    /// attestation cannot be recorded under a basis that would overstate it.
    #[must_use]
    pub const fn basis(&self) -> Basis {
        Basis::Attested
    }

    /// Whether this attestation may be cited as support for a claim.
    #[must_use]
    pub const fn can_be_cited(&self) -> bool {
        self.signature.is_citable()
    }

    /// The verification status this attestation supports.
    #[must_use]
    pub const fn status(&self) -> VerificationStatus {
        self.signature.status()
    }

    /// The confidence this attestation supports.
    ///
    /// # Errors
    ///
    /// Returns a provenance error when the attestation cannot be cited, which is the
    /// case for every signature state but [`SignatureState::PresentVerified`]. The
    /// refusal happens here rather than at the reporting layer so that no code path
    /// can turn an unchecked signature into cited support.
    pub fn as_confidence(&self) -> Result<Confidence> {
        if !self.can_be_cited() {
            return Err(ProvenanceFailure::AttestationUnverified {
                issuer: self.issuer.clone(),
                detail: match self.signature {
                    SignatureState::NotPresent => "the attestation carries no signature".to_owned(),
                    SignatureState::PresentUnverified => {
                        "the signature was not verified, so citing the attestation would be \
                         indistinguishable from citing a verified one"
                            .to_owned()
                    },
                    SignatureState::Invalid => {
                        "the signature was checked and did not hold, so the attestation is refuted \
                         rather than merely unverified"
                            .to_owned()
                    },
                    SignatureState::PresentVerified => {
                        "unreachable: a verified signature is citable".to_owned()
                    },
                },
            }
            .into_error());
        }

        // The ceiling is applied rather than assumed: a caller cannot produce a
        // VERIFIED confidence from an attestation, whatever it passes in.
        Confidence::new(
            ATTESTATION_CONFIDENCE_CEILING,
            self.evidence.clone(),
            Vec::new(),
        )
        .map(|confidence| {
            confidence.with_rationale(format!(
                "attested by {} using {}",
                self.issuer,
                self.verification_method
                    .as_deref()
                    .unwrap_or("an unrecorded method")
            ))
        })
    }

    /// Checks the attestation's own invariants.
    ///
    /// # Errors
    ///
    /// Returns a provenance error when the issuer is empty, and a validation error
    /// when the signature was verified without a recorded method or when no evidence
    /// is cited.
    pub fn validate(&self) -> Result<()> {
        if self.issuer.is_empty() {
            return Err(ProvenanceFailure::AttestationUnverified {
                issuer: self.issuer.clone(),
                detail: "a claim with no issuer names nobody".to_owned(),
            }
            .into_error());
        }
        if self.signature == SignatureState::PresentVerified && self.verification_method.is_none() {
            return Err(EngineError::Validation {
                path: "/attestation/verificationMethod".to_owned(),
                detail: "a verified signature must record how it was verified".to_owned(),
            });
        }
        if self.evidence.is_empty() {
            return Err(EngineError::Validation {
                path: "/attestation/evidence".to_owned(),
                detail: "an attestation must cite the evidence of its existence".to_owned(),
            });
        }
        Ok(())
    }
}

/// The attestation that best supports a claim, among several.
///
/// Prefers a verified signature, then the earliest issue date among equals so that the
/// choice is deterministic rather than dependent on the order the records arrived in.
/// Returns `None` when nothing can be cited, which is the honest answer rather than
/// the first record.
#[must_use]
pub fn best_citable(attestations: &[Attestation]) -> Option<&Attestation> {
    attestations
        .iter()
        .filter(|attestation| attestation.can_be_cited())
        .min_by(|left, right| {
            left.issued_at
                .as_deref()
                .unwrap_or("")
                .cmp(right.issued_at.as_deref().unwrap_or(""))
                .then_with(|| left.issuer.cmp(&right.issuer))
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use amasario_core::EntityKind;

    fn subject() -> EntityRef {
        EntityRef::new(EntityKind::Artifact, "a".repeat(64)).expect("a reference")
    }

    fn verified() -> Attestation {
        Attestation::verified(
            "an issuer",
            subject(),
            "this artifact was built from revision 9f2c1e0",
            "sigstore",
            vec!["e".to_owned()],
        )
        .expect("a verified attestation")
    }

    #[test]
    fn a_verified_signature_is_the_only_citable_state() {
        // Citing an unchecked signature is indistinguishable from citing a checked one
        // once the record reaches a report.
        assert!(SignatureState::PresentVerified.is_citable());
        assert!(!SignatureState::PresentUnverified.is_citable());
        assert!(!SignatureState::NotPresent.is_citable());
        assert!(!SignatureState::Invalid.is_citable());
    }

    #[test]
    fn an_invalid_signature_refutes_rather_than_weakens_the_attestation() {
        assert!(SignatureState::Invalid.is_refutation());
        assert_eq!(
            SignatureState::Invalid.status(),
            VerificationStatus::Conflicting
        );
        assert!(!SignatureState::PresentVerified.is_refutation());
        assert_eq!(
            SignatureState::PresentVerified.status(),
            VerificationStatus::Verified
        );
    }

    #[test]
    fn an_unchecked_signature_leaves_the_claim_undetermined() {
        for state in [
            SignatureState::NotPresent,
            SignatureState::PresentUnverified,
        ] {
            assert!(state.is_inconclusive());
            assert_eq!(state.status(), VerificationStatus::Unverified);
            assert!(!state.is_refutation());
        }
    }

    #[test]
    fn signature_states_round_trip_through_their_wire_names() {
        for state in [
            SignatureState::NotPresent,
            SignatureState::PresentUnverified,
            SignatureState::PresentVerified,
            SignatureState::Invalid,
        ] {
            assert_eq!(
                SignatureState::from_str(state.as_str()).expect("round trip"),
                state
            );
        }
        SignatureState::from_str("TRUSTED").expect_err("an unknown state is rejected");
    }

    #[test]
    fn an_attestation_can_never_reach_verified_confidence() {
        // VERIFIED confidence means the decisive evidence was checked successfully,
        // and a third party's statement is the thing being checked rather than the
        // check.
        let confidence = verified().as_confidence().expect("a citable attestation");
        assert_eq!(confidence.level, ATTESTATION_CONFIDENCE_CEILING);
        assert_eq!(confidence.level, ConfidenceLevel::HighConfidence);
        assert_ne!(confidence.level, ConfidenceLevel::Verified);
        assert!(confidence.has_support());
        assert!(
            confidence
                .rationale
                .as_deref()
                .expect("a rationale")
                .contains("an issuer")
        );
        // And it matches the ceiling the core crate declares for this basis.
        assert_eq!(
            verified().basis().confidence_ceiling(),
            ATTESTATION_CONFIDENCE_CEILING
        );
    }

    #[test]
    fn an_uncitable_attestation_cannot_produce_a_confidence() {
        // The refusal happens here rather than at the reporting layer so that no code
        // path can turn an unchecked signature into cited support.
        for state in [
            SignatureState::NotPresent,
            SignatureState::PresentUnverified,
            SignatureState::Invalid,
        ] {
            let attestation = Attestation::new(
                "an issuer",
                subject(),
                "a claim",
                state,
                None,
                vec!["e".to_owned()],
            )
            .expect("a valid record");
            assert!(!attestation.can_be_cited());
            let error = attestation
                .as_confidence()
                .expect_err("an uncitable attestation has no confidence");
            assert!(
                error.to_string().contains("cannot be cited"),
                "got: {error}"
            );
        }
    }

    #[test]
    fn the_refusal_for_an_invalid_signature_says_it_is_refuted() {
        let attestation = Attestation::new(
            "an issuer",
            subject(),
            "a claim",
            SignatureState::Invalid,
            None,
            vec!["e".to_owned()],
        )
        .expect("a valid record");
        let error = attestation
            .as_confidence()
            .expect_err("a refuted attestation is not support");
        assert!(
            error
                .to_string()
                .contains("refuted rather than merely unverified"),
            "got: {error}"
        );
    }

    #[test]
    fn a_verified_signature_must_record_how_it_was_verified() {
        // A verification whose method was not recorded cannot be re-examined, which
        // makes it a claim about a check rather than a record of one.
        let error = Attestation::new(
            "an issuer",
            subject(),
            "a claim",
            SignatureState::PresentVerified,
            None,
            vec!["e".to_owned()],
        )
        .expect_err("no method");
        assert!(error.to_string().contains("how it was verified"));
    }

    #[test]
    fn an_attestation_must_name_an_issuer_and_cite_evidence() {
        Attestation::new(
            "",
            subject(),
            "a claim",
            SignatureState::NotPresent,
            None,
            vec!["e".to_owned()],
        )
        .expect_err("a claim with no issuer names nobody");

        Attestation::new(
            "an issuer",
            subject(),
            "a claim",
            SignatureState::NotPresent,
            None,
            Vec::new(),
        )
        .expect_err("no evidence of the attestation's existence");
    }

    #[test]
    fn the_attestation_records_what_it_was_about_and_in_the_issuers_words() {
        let attestation = verified();
        assert_eq!(attestation.basis(), Basis::Attested);
        assert_eq!(attestation.subject, subject());
        assert!(attestation.claim.contains("built from revision"));
        assert_eq!(attestation.verification_method.as_deref(), Some("sigstore"));
        assert_eq!(attestation.status(), VerificationStatus::Verified);
    }

    #[test]
    fn the_best_citable_attestation_prefers_the_earliest_and_is_deterministic() {
        let early = verified().issued_at("2026-01-01T00:00:00Z");
        let late = verified().issued_at("2026-06-01T00:00:00Z");
        let unchecked = Attestation::new(
            "another issuer",
            subject(),
            "a claim",
            SignatureState::PresentUnverified,
            None,
            vec!["e".to_owned()],
        )
        .expect("a valid record");

        // Bound to a local so the borrow outlives the call: the function returns a
        // reference into the slice, not a copy of what it chose.
        let candidates = [late.clone(), unchecked.clone(), early.clone()];
        let chosen = best_citable(&candidates).expect("two are citable");
        assert_eq!(chosen, &early);

        // The order the records arrive in does not change the answer.
        assert_eq!(
            best_citable(&[early.clone(), late, unchecked.clone()]),
            Some(&early)
        );
        assert_eq!(
            best_citable(&[unchecked]),
            None,
            "nothing citable is the honest answer, not the first record"
        );
        assert_eq!(best_citable(&[]), None);
    }

    #[test]
    fn an_attestation_round_trips_through_json() {
        let original = verified()
            .issued_at("2026-09-15T00:00:00Z")
            .expires_at("2027-09-15T00:00:00Z");
        let json = serde_json::to_string(&original).expect("serialises");
        let restored: Attestation = serde_json::from_str(&json).expect("deserialises");
        assert_eq!(restored, original);
        restored.validate().expect("self-consistent");
        assert_eq!(
            restored.as_confidence().expect("citable").level,
            ATTESTATION_CONFIDENCE_CEILING
        );
    }

    #[test]
    fn a_deserialised_attestation_is_caught_by_validation() {
        let mut attestation = verified();
        attestation.validate().expect("self-consistent");
        attestation.verification_method = None;
        attestation
            .validate()
            .expect_err("a verified signature without a method is not a record of a check");
    }
}
