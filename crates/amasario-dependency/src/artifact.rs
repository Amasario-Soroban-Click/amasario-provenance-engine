//! Dependencies on artifacts, executables and targets beyond the boundary.
//!
//! # The rule this module implements
//!
//! `dependency/artifact-dependency` states it in one sentence: a dependency whose
//! target is an artifact or executable MUST be established "from a digest match or
//! from a build input declared by the subject", and MUST NOT be established "from
//! artifact size, media type, module name, version string or any other metadata
//! similarity".
//!
//! That is why this module exposes no constructor taking a size, a media type or a
//! name. The only way to establish an artifact dependency here is
//! [`establish_from_digest`], which requires two [`Digest`]es, and the reason is the
//! second half of the rule's rationale: a dependency asserted from metadata records no
//! digest, so no later observation can ever refute it. The engine would be recording
//! something unfalsifiable, and an unfalsifiable dependency is indistinguishable from
//! a correct one to every consumer downstream.
//!
//! # Comparison is three-valued, and so is the outcome
//!
//! Two digests under different algorithms are not equal and not unequal; they cannot
//! be compared at all. [`ArtifactDependencyOutcome`] carries that case separately from
//! a contradiction, because turning an inability to compare into a mismatch would
//! report a contract as refuted on the strength of a comparison that was never made.
//!
//! # The boundary is not a failure
//!
//! An `EXTERNAL` dependency is not a dependency that could not be established - it is
//! one whose target lies outside what Amasario can inspect, which is a durable fact
//! about the deployment rather than a shortcoming of the analysis. [`establish_external`]
//! therefore requires the observation boundary, and the class it produces can never be
//! reported as verified: there is nothing to check it against.

use amasario_core::{
    Basis, Digest, EntityKind, EntityRef, EvidenceType, ObservationBoundary, Relationship, Result,
};
use serde::{Deserialize, Serialize};

use crate::classifier::{Candidate, EvidenceRef};

/// What comparing two digests established about an artifact dependency.
///
/// Modelled as an enum rather than a `Result` because two of the three cases are
/// findings rather than failures, and the third is not a finding at all. A caller that
/// wants an error on contradiction can match on the variant; a caller recording a
/// snapshot wants all three.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE", tag = "outcome")]
pub enum ArtifactDependencyOutcome {
    /// The digests agree, so the dependency is established.
    Established(Box<Candidate>),
    /// The digests disagree, which contradicts the dependency claim.
    Contradicted {
        /// The digest the claim rested on.
        claimed: String,
        /// The digest the bytes actually produced.
        computed: String,
        /// What the two describe, so a reader knows what disagrees.
        subject: String,
    },
    /// The digests cannot be compared, because they were computed under different
    /// algorithms.
    Incomparable {
        /// The algorithm the claim used.
        claimed_algorithm: String,
        /// The algorithm that was computed.
        computed_algorithm: String,
    },
}

impl ArtifactDependencyOutcome {
    /// The candidate, when the dependency was established.
    #[must_use]
    pub fn candidate(&self) -> Option<&Candidate> {
        match self {
            Self::Established(candidate) => Some(candidate),
            Self::Contradicted { .. } | Self::Incomparable { .. } => None,
        }
    }

    /// Whether the comparison contradicted the claim.
    ///
    /// The only outcome that makes a claim false. Stated as a method so that a caller
    /// does not treat an inability to compare as a refutation, which is the mistake
    /// this type exists to prevent.
    #[must_use]
    pub const fn is_contradiction(&self) -> bool {
        matches!(self, Self::Contradicted { .. })
    }

    /// Whether the comparison left the claim undetermined.
    #[must_use]
    pub const fn is_inconclusive(&self) -> bool {
        matches!(self, Self::Incomparable { .. })
    }
}

/// Establishes a dependency on an artifact or executable from a digest match.
///
/// # Errors
///
/// Returns a dependency error when a candidate cannot be constructed, which happens
/// when the citation is empty or the endpoints the relationship permits do not include
/// these kinds.
pub fn establish_from_digest(
    subject: EntityRef,
    artifact: EntityRef,
    claimed: &Digest,
    computed: &Digest,
    evidence: EvidenceRef,
    boundary: &ObservationBoundary,
    detail: impl Into<String>,
) -> Result<ArtifactDependencyOutcome> {
    if claimed.algorithm() != computed.algorithm() {
        return Ok(ArtifactDependencyOutcome::Incomparable {
            claimed_algorithm: claimed.algorithm().as_str().to_owned(),
            computed_algorithm: computed.algorithm().as_str().to_owned(),
        });
    }
    if !claimed.matches(computed) {
        return Ok(ArtifactDependencyOutcome::Contradicted {
            claimed: claimed.value().to_owned(),
            computed: computed.value().to_owned(),
            subject: artifact.to_string(),
        });
    }
    let candidate = Candidate::new(
        subject,
        artifact,
        Relationship::DependsOn,
        // A digest match is exactly the basis the rule names, so it is the basis the
        // candidate carries rather than a caller-supplied one.
        Basis::EmbeddedDigest,
        vec![evidence],
    )?
    .observed_at(boundary.clone())
    .explained_by(detail);
    Ok(ArtifactDependencyOutcome::Established(Box::new(candidate)))
}

/// Establishes that a target lies outside the observable boundary.
///
/// The dependency is real and durable; what is unavailable is the ability to check it,
/// which is why the boundary is recorded with it and why the resulting class can never
/// be reported as verified.
///
/// # Errors
///
/// Returns a dependency error when a candidate cannot be constructed.
pub fn establish_external(
    subject: EntityRef,
    target: EntityRef,
    evidence: EvidenceRef,
    boundary: &ObservationBoundary,
    detail: impl Into<String>,
) -> Result<Candidate> {
    Candidate::new(
        subject,
        target,
        Relationship::DependsOn,
        Basis::DeclaredManifest,
        vec![
            evidence,
            EvidenceRef::new(EvidenceType::Observation, boundary.observed_at.clone())?,
        ],
    )
    .map(|candidate| candidate.observed_at(boundary.clone()).explained_by(detail))
}

/// Whether a reference is an artifact or executable, and so subject to the
/// digest requirement.
#[must_use]
pub const fn is_artifact_target(entity: &EntityRef) -> bool {
    matches!(entity.kind, EntityKind::Artifact | EntityKind::Wasm)
}

#[cfg(test)]
mod tests {
    use super::*;
    use amasario_core::{DigestAlgorithm, LedgerSequence, Network, NetworkType};

    fn boundary() -> ObservationBoundary {
        ObservationBoundary {
            network: Network::new(
                "testnet",
                NetworkType::Testnet,
                "Test SDF Network ; September 2015",
            )
            .expect("a network"),
            ledger: LedgerSequence::new(5_000).expect("a ledger"),
            observed_at: "2026-09-15T00:00:00Z".to_owned(),
            spec_version: None,
        }
    }

    fn subject() -> EntityRef {
        EntityRef::new(EntityKind::Contract, "C-subject").expect("a reference")
    }

    fn artifact() -> EntityRef {
        EntityRef::new(EntityKind::Wasm, "the module").expect("a reference")
    }

    fn citation() -> EvidenceRef {
        EvidenceRef::new(EvidenceType::Artifact, "artifact-1").expect("a citation")
    }

    fn digest(seed: u8) -> Digest {
        Digest::sha256_of(&[seed])
    }

    #[test]
    fn matching_digests_establish_a_dependency_on_a_digest_basis() {
        let outcome = establish_from_digest(
            subject(),
            artifact(),
            &digest(1),
            &digest(1),
            citation(),
            &boundary(),
            "the module's bytes match the recorded digest",
        )
        .expect("establishable");
        let candidate = outcome.candidate().expect("established");
        assert_eq!(candidate.basis, Basis::EmbeddedDigest);
        assert_eq!(candidate.relationship, Relationship::DependsOn);
        assert!(candidate.object.kind == EntityKind::Wasm);
        assert!(!outcome.is_contradiction());
        assert!(!outcome.is_inconclusive());

        // And it classifies, which is what makes it usable rather than merely built.
        let classification = crate::classifier::classify(candidate).expect("classifiable");
        assert!(classification.has(amasario_core::DependencyClass::Wasm));
    }

    #[test]
    fn differing_digests_contradict_the_claim_rather_than_refusing_it() {
        let outcome = establish_from_digest(
            subject(),
            artifact(),
            &digest(1),
            &digest(2),
            citation(),
            &boundary(),
            "the recorded digest does not match the bytes",
        )
        .expect("a contradiction is a finding");
        assert!(outcome.is_contradiction());
        assert!(outcome.candidate().is_none());
        match outcome {
            ArtifactDependencyOutcome::Contradicted {
                claimed, computed, ..
            } => {
                assert_ne!(claimed, computed);
            },
            other => panic!("expected a contradiction, got {other:?}"),
        }
    }

    #[test]
    fn digests_under_different_algorithms_are_incomparable_rather_than_different() {
        // Turning an inability to compare into a mismatch would report a contract as
        // refuted on the strength of a comparison that was never made.
        let sha512 = Digest::new(DigestAlgorithm::Sha512, &"a".repeat(128)).expect("a digest");
        let outcome = establish_from_digest(
            subject(),
            artifact(),
            &digest(1),
            &sha512,
            citation(),
            &boundary(),
            "compared against a differently computed digest",
        )
        .expect("an incomparable pair is a finding");
        assert!(outcome.is_inconclusive());
        assert!(!outcome.is_contradiction());
        assert!(outcome.candidate().is_none());
    }

    #[test]
    fn there_is_no_constructor_that_accepts_metadata() {
        // The rule's prohibition, expressed as a test: a candidate built from a
        // configured endpoint is refused by the classifier, so no caller can reach a
        // WASM dependency through metadata even by construction.
        let metadata = Candidate::new(
            subject(),
            EntityRef::new(EntityKind::Wasm, "module-name-lookalike").expect("a reference"),
            Relationship::DependsOn,
            Basis::ConfiguredEndpoint,
            vec![citation()],
        )
        .expect("a constructible candidate")
        .observed_at(boundary());
        let error = crate::classifier::classify(&metadata)
            .expect_err("metadata cannot establish an artifact dependency");
        assert!(
            error.to_string().contains("nothing to re-compute"),
            "got: {error}"
        );
    }

    #[test]
    fn an_external_target_records_the_boundary_and_cannot_be_verified() {
        let candidate = establish_external(
            subject(),
            EntityRef::new(EntityKind::Source, "https://example.invalid/r@abc")
                .expect("a reference"),
            EvidenceRef::new(EvidenceType::Source, "src-1").expect("a citation"),
            &boundary(),
            "the subject declares a source outside the observed network",
        )
        .expect("establishable");
        let classification = crate::classifier::classify(&candidate).expect("classifiable");
        assert_eq!(
            classification.classes,
            vec![amasario_core::DependencyClass::External]
        );
        assert_eq!(
            classification.verification,
            amasario_core::VerificationStatus::Unverified
        );
        assert!(!classification.is_observed());
        assert!(candidate.boundary.is_some());
    }

    #[test]
    fn an_artifact_target_is_recognised_by_its_kind() {
        assert!(is_artifact_target(&artifact()));
        assert!(is_artifact_target(
            &EntityRef::new(EntityKind::Artifact, "a").expect("a reference")
        ));
        assert!(!is_artifact_target(&subject()));
    }

    #[test]
    fn establishing_is_deterministic() {
        let first = establish_from_digest(
            subject(),
            artifact(),
            &digest(1),
            &digest(1),
            citation(),
            &boundary(),
            "match",
        )
        .expect("establishable");
        let second = establish_from_digest(
            subject(),
            artifact(),
            &digest(1),
            &digest(1),
            citation(),
            &boundary(),
            "match",
        )
        .expect("establishable");
        assert_eq!(first, second);
    }
}
