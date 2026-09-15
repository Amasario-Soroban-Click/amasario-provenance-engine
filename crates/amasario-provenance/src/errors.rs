//! The failure modes provenance analysis can report, and how each is classified.
//!
//! # Why the distinction inside this module matters most of all
//!
//! Provenance is the part of the engine where an overstatement is most damaging,
//! because its output is what a consumer would use to decide whether a deployed
//! artifact corresponds to a source revision. A wrong `VERIFIED` is worse than no
//! answer: it invites a conclusion the evidence does not support, and it is
//! indistinguishable from a correct one to everyone downstream.
//!
//! [`ProvenanceFailure`] therefore separates three things that a coarser model
//! would merge:
//!
//! * **Unrecorded.** A link in the chain has no record at all. This is the ordinary
//!   state of most contracts, it is not a defect, and it says nothing about whether
//!   the claim is true.
//! * **Unverifiable.** A record exists but cannot be checked - an attestation whose
//!   signature was not verified, a build whose toolchain was not recorded. The
//!   record may be entirely accurate; the engine simply cannot say.
//! * **Contradicted.** Two facts the engine holds cannot both be true: a claimed
//!   source revision that rebuilds to a different digest, an on-chain hash that
//!   disagrees with the module's bytes.
//!
//! Only the third makes a claim false. The first two leave it unknown, and the
//! whole point of the enumeration is that a caller cannot accidentally treat "I
//! could not check" as "I checked and it failed".

use amasario_core::{EngineError, ErrorCategory, Result};

/// A way provenance can fail to be established.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProvenanceFailure {
    /// No source record exists for the claim being made.
    SourceUnrecorded {
        /// What the source record would have described.
        subject: String,
    },
    /// The source record names a revision that cannot identify a source tree.
    ///
    /// A branch or tag name that has not been resolved to a commit is the common
    /// case, and it is a real limitation rather than a technicality: a branch name
    /// denotes different commits at different times, so it cannot establish which
    /// source was built.
    RevisionNotImmutable {
        /// The repository the revision belongs to.
        repository: String,
        /// The revision as recorded.
        revision: String,
        /// Why it does not identify a source tree.
        detail: String,
    },
    /// A recorded revision does not have the form of one.
    RevisionMalformed {
        /// The revision as recorded.
        revision: String,
        /// What about it is malformed.
        detail: String,
    },
    /// A repository URL cannot be interpreted.
    RepositoryMalformed {
        /// The URL as recorded.
        url: String,
        /// What about it is malformed.
        detail: String,
    },
    /// No build record exists for the artifact.
    BuildUnrecorded {
        /// The artifact whose build is missing.
        artifact: String,
    },
    /// The build record does not name the toolchain that produced the artifact.
    ToolchainUnrecorded {
        /// The artifact whose toolchain is missing.
        artifact: String,
    },
    /// No artifact is recorded for an expected digest.
    ArtifactUnrecorded {
        /// The digest that was expected.
        expected: String,
    },
    /// No deployment record exists for the contract.
    DeploymentUnrecorded {
        /// The contract whose deployment is missing.
        contract_id: String,
    },
    /// The artifact's bytes do not hash to the digest the record claims.
    DigestContradiction {
        /// The digest the record claims.
        claimed: String,
        /// The digest the bytes actually produce.
        computed: String,
        /// What the two describe, so a reader knows what disagrees.
        subject: String,
    },
    /// A claim could not be checked because the evidence needed to check it is
    /// absent.
    EvidenceMissing {
        /// The link or claim that could not be checked.
        claim: String,
        /// The kind of evidence that would have been needed.
        required: String,
    },
    /// An attestation's signature was not verified, so the attestation cannot be
    /// cited as support.
    AttestationUnverified {
        /// Who issued the attestation.
        issuer: String,
        /// Why it cannot be cited.
        detail: String,
    },
    /// A chain link would connect entities that the relationship cannot connect.
    ImpossibleLink {
        /// The relationship that was attempted.
        relationship: String,
        /// The kind of the subject.
        subject_kind: String,
        /// The kind of the object.
        object_kind: String,
    },
}

impl ProvenanceFailure {
    /// The category this failure belongs to.
    #[must_use]
    pub const fn category(&self) -> ErrorCategory {
        match self {
            Self::SourceUnrecorded { .. }
            | Self::RevisionNotImmutable { .. }
            | Self::RevisionMalformed { .. }
            | Self::RepositoryMalformed { .. }
            | Self::BuildUnrecorded { .. }
            | Self::ToolchainUnrecorded { .. }
            | Self::ArtifactUnrecorded { .. }
            | Self::DeploymentUnrecorded { .. }
            | Self::EvidenceMissing { .. }
            | Self::ImpossibleLink { .. } => ErrorCategory::Provenance,
            // A contradiction and an unverifiable attestation are both still
            // provenance failures rather than input-validation failures: the input
            // was well formed, and what failed was establishing the claim.
            Self::DigestContradiction { .. } | Self::AttestationUnverified { .. } => {
                ErrorCategory::Provenance
            },
        }
    }

    /// Whether this failure means the engine holds two facts that cannot both be
    /// true.
    ///
    /// The single most important question a consumer asks, and the reason
    /// verification has a `CONFLICTING` status at all. `false` for every other
    /// variant, including the ones where a claim could not be checked: not being able
    /// to check a claim is not evidence against it.
    #[must_use]
    pub const fn is_contradiction(&self) -> bool {
        matches!(self, Self::DigestContradiction { .. })
    }

    /// Whether this failure means a record was absent rather than unusable.
    #[must_use]
    pub const fn is_unrecorded(&self) -> bool {
        matches!(
            self,
            Self::SourceUnrecorded { .. }
                | Self::BuildUnrecorded { .. }
                | Self::ArtifactUnrecorded { .. }
                | Self::DeploymentUnrecorded { .. }
        )
    }

    /// Converts the failure into the engine's structured error.
    #[must_use]
    pub fn into_error(self) -> EngineError {
        match self {
            Self::SourceUnrecorded { subject } => EngineError::Provenance(format!(
                "no source record exists for {subject}; this says nothing about whether a source \
                 exists, only that the engine holds no record of one"
            )),
            Self::RevisionNotImmutable {
                repository,
                revision,
                detail,
            } => EngineError::Provenance(format!(
                "revision {revision:?} of {repository} does not identify a source tree: {detail}. A \
                 revision that moves cannot establish which source was built"
            )),
            Self::RevisionMalformed { revision, detail } => {
                EngineError::Provenance(format!("revision {revision:?} is malformed: {detail}"))
            },
            Self::RepositoryMalformed { url, detail } => {
                EngineError::Provenance(format!("repository URL {url:?} is malformed: {detail}"))
            },
            Self::BuildUnrecorded { artifact } => EngineError::Provenance(format!(
                "no build record exists for {artifact}; the artifact cannot be attributed to a build"
            )),
            Self::ToolchainUnrecorded { artifact } => EngineError::Provenance(format!(
                "the build record for {artifact} does not name its toolchain, so the artifact \
                 cannot be reproduced from the record alone"
            )),
            Self::ArtifactUnrecorded { expected } => {
                EngineError::Provenance(format!("no artifact is recorded for digest {expected}"))
            },
            Self::DeploymentUnrecorded { contract_id } => EngineError::Provenance(format!(
                "no deployment record exists for contract {contract_id}"
            )),
            Self::DigestContradiction {
                claimed,
                computed,
                subject,
            } => EngineError::Provenance(format!(
                "{subject} claims digest {claimed}, but the bytes hash to {computed}; these two \
                 observations cannot both be correct"
            )),
            Self::EvidenceMissing { claim, required } => EngineError::Provenance(format!(
                "{claim} could not be checked because no {required} evidence is recorded; this \
                 leaves the claim unknown rather than refuted"
            )),
            Self::AttestationUnverified { issuer, detail } => EngineError::Provenance(format!(
                "the attestation from {issuer} cannot be cited: {detail}"
            )),
            Self::ImpossibleLink {
                relationship,
                subject_kind,
                object_kind,
            } => EngineError::Provenance(format!(
                "{relationship} cannot connect a {subject_kind} to a {object_kind}; the \
                 relationship's semantics do not permit those endpoint kinds"
            )),
        }
    }

    /// Whether the failure leaves the claim's truth undetermined.
    ///
    /// True for everything except a contradiction, and stated as a method so that
    /// verification asks one question rather than matching on variants in several
    /// places where one could be forgotten.
    #[must_use]
    pub const fn leaves_claim_undetermined(&self) -> bool {
        !self.is_contradiction()
    }
}

/// A one-line description of what went wrong, without the identifiers.
#[must_use]
pub const fn describe(failure: &ProvenanceFailure) -> &'static str {
    match failure {
        ProvenanceFailure::SourceUnrecorded { .. } => "no source record exists",
        ProvenanceFailure::RevisionNotImmutable { .. } => {
            "the revision does not identify a source tree"
        },
        ProvenanceFailure::RevisionMalformed { .. } => "the revision is malformed",
        ProvenanceFailure::RepositoryMalformed { .. } => "the repository URL is malformed",
        ProvenanceFailure::BuildUnrecorded { .. } => "no build record exists",
        ProvenanceFailure::ToolchainUnrecorded { .. } => "the toolchain is unrecorded",
        ProvenanceFailure::ArtifactUnrecorded { .. } => "no artifact is recorded",
        ProvenanceFailure::DeploymentUnrecorded { .. } => "no deployment record exists",
        ProvenanceFailure::DigestContradiction { .. } => "digests contradict each other",
        ProvenanceFailure::EvidenceMissing { .. } => "evidence needed to check the claim is absent",
        ProvenanceFailure::AttestationUnverified { .. } => "the attestation is unverified",
        ProvenanceFailure::ImpossibleLink { .. } => {
            "the relationship cannot connect those entities"
        },
    }
}

/// Turns the first of several failures into a `Result`, or succeeds when the list is
/// empty.
///
/// # Errors
///
/// Returns the first failure's error when `failures` is non-empty.
pub fn first_failure(failures: Vec<ProvenanceFailure>) -> Result<()> {
    match failures.into_iter().next() {
        Some(failure) => Err(failure.into_error()),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn every_failure() -> Vec<ProvenanceFailure> {
        vec![
            ProvenanceFailure::SourceUnrecorded {
                subject: "C-x".to_owned(),
            },
            ProvenanceFailure::RevisionNotImmutable {
                repository: "https://example.invalid/r".to_owned(),
                revision: "main".to_owned(),
                detail: "a branch name moves".to_owned(),
            },
            ProvenanceFailure::RevisionMalformed {
                revision: "zz".to_owned(),
                detail: "not hexadecimal".to_owned(),
            },
            ProvenanceFailure::RepositoryMalformed {
                url: "not a url".to_owned(),
                detail: "no scheme".to_owned(),
            },
            ProvenanceFailure::BuildUnrecorded {
                artifact: "wasm".to_owned(),
            },
            ProvenanceFailure::ToolchainUnrecorded {
                artifact: "wasm".to_owned(),
            },
            ProvenanceFailure::ArtifactUnrecorded {
                expected: "a".repeat(64),
            },
            ProvenanceFailure::DeploymentUnrecorded {
                contract_id: "C-x".to_owned(),
            },
            ProvenanceFailure::DigestContradiction {
                claimed: "a".repeat(64),
                computed: "b".repeat(64),
                subject: "the deployed module".to_owned(),
            },
            ProvenanceFailure::EvidenceMissing {
                claim: "source-to-build".to_owned(),
                required: "BUILD".to_owned(),
            },
            ProvenanceFailure::AttestationUnverified {
                issuer: "an issuer".to_owned(),
                detail: "no signature".to_owned(),
            },
            ProvenanceFailure::ImpossibleLink {
                relationship: "BUILT_FROM".to_owned(),
                subject_kind: "BUILD".to_owned(),
                object_kind: "DEPLOYMENT".to_owned(),
            },
        ]
    }

    #[test]
    fn only_a_contradiction_makes_a_claim_false() {
        // The distinction the module exists for: not being able to check a claim is
        // not evidence against it, and an implementation that treated the two alike
        // would report an unverified contract as a refuted one.
        for failure in every_failure() {
            assert_eq!(
                failure.is_contradiction(),
                matches!(failure, ProvenanceFailure::DigestContradiction { .. }),
                "{failure:?} misclassified"
            );
            assert_eq!(
                failure.leaves_claim_undetermined(),
                !failure.is_contradiction(),
                "{failure:?} misclassified"
            );
        }
    }

    #[test]
    fn an_unrecorded_link_is_not_a_contradiction() {
        let failure = ProvenanceFailure::SourceUnrecorded {
            subject: "contract C".to_owned(),
        };
        assert!(failure.is_unrecorded());
        assert!(!failure.is_contradiction());
        assert_eq!(failure.category(), ErrorCategory::Provenance);
        let message = failure.into_error().to_string();
        assert!(
            message.contains("says nothing about whether a source exists"),
            "the error must not read as a refutation: {message}"
        );
    }

    #[test]
    fn an_unverifiable_attestation_says_the_claim_is_unknown_not_false() {
        let failure = ProvenanceFailure::AttestationUnverified {
            issuer: "an issuer".to_owned(),
            detail: "the signature was not verified".to_owned(),
        };
        assert!(!failure.is_contradiction());
        assert!(failure.leaves_claim_undetermined());
        assert!(
            failure
                .clone()
                .into_error()
                .to_string()
                .contains("cannot be cited")
        );
        assert_eq!(failure.category(), ErrorCategory::Provenance);
    }

    #[test]
    fn missing_evidence_leaves_the_claim_unknown() {
        let error = ProvenanceFailure::EvidenceMissing {
            claim: "build-to-artifact".to_owned(),
            required: "BUILD".to_owned(),
        }
        .into_error();
        let message = error.to_string();
        assert!(
            message.contains("unknown rather than refuted"),
            "got: {message}"
        );
    }

    #[test]
    fn a_revision_that_moves_explains_why_it_cannot_establish_provenance() {
        let error = ProvenanceFailure::RevisionNotImmutable {
            repository: "https://example.invalid/r".to_owned(),
            revision: "main".to_owned(),
            detail: "a branch name moves".to_owned(),
        }
        .into_error();
        assert!(
            error
                .to_string()
                .contains("cannot establish which source was built"),
            "got: {error}"
        );
    }

    #[test]
    fn every_failure_has_a_category_a_description_and_a_code() {
        let failures = every_failure();
        for failure in &failures {
            assert!(
                ErrorCategory::all().contains(&failure.category()),
                "{failure:?} has a category outside the specification's enumeration"
            );
            assert!(!describe(failure).is_empty());
            assert!(!failure.clone().into_error().code().is_empty());
        }
        assert_eq!(failures.len(), 12, "every variant is covered by this test");
    }

    #[test]
    fn an_empty_failure_list_succeeds_and_a_populated_one_reports_its_first() {
        first_failure(Vec::new()).expect("nothing to report");
        let error = first_failure(vec![ProvenanceFailure::BuildUnrecorded {
            artifact: "wasm".to_owned(),
        }])
        .expect_err("a failure must be reported");
        assert!(error.to_string().contains("no build record"));
    }
}
