//! The ways an evidence record can fail to be citable, and how each is classified.
//!
//! # Why a record can fail at all
//!
//! `taxonomies/evidence-types.yaml` gives every class a `traceableTo` - "the entity or
//! record a reviewer would consult to independently confirm the evidence" - and the
//! schema pins further requirements on individual classes: `TRANSACTION` evidence
//! requires `successful`, `SOURCE` evidence is "traceable to the repository and the
//! exact revision, not merely to the repository", `ARTIFACT` evidence is "traceable to
//! the artifact bytes, since a digest is meaningless without the content it was computed
//! over". Those are not stylistic preferences; each one is the difference between a
//! record a reviewer can check and a record that only looks checkable.
//!
//! Every requirement therefore has a variant here, and a record that fails one is
//! refused rather than stored. The four that carry the most weight:
//!
//! * [`EvidenceFailure::FailedTransactionCitedAsEffect`] - the taxonomy states it
//!   directly: "A transaction that failed MUST NOT be cited as evidence that its
//!   intended effect occurred." A failed transaction is still evidence; what it cannot
//!   be is evidence that something happened.
//! * [`EvidenceFailure::NoteTooLong`] - a note longer than the schema's `maxLength` is
//!   refused, because the engine would otherwise emit a document it cannot read back.
//!
//! A mutable revision is conspicuously absent from this list. A branch name denotes
//! different commits at different times, so a source record resting on one cannot be
//! checked later - but it is still evidence, and
//! `rules/provenance/source-to-build.yaml` states the consequence as a ceiling on the
//! *status* rather than as a defect in the record: "a source revision reached through a
//! mutable ref MUST NOT be reported as VERIFIED". Refusing the record would discard the
//! only evidence that the source was read, so the ceiling is applied in
//! [`crate::confidence`] instead.
//! * [`EvidenceFailure::BoundaryUnrecorded`] - a ledger sequence is only meaningful
//!   against a chain, so a class derived from network observation without a boundary has
//!   recorded a number that means nothing on its own.
//! * [`EvidenceFailure::AttestationClaimMismatch`] - the taxonomy is explicit: "An
//!   attestation supports the specific claim it states and nothing broader; an
//!   attestation of a build MUST NOT be read as an attestation of contract behaviour or
//!   of safety."
//! * [`EvidenceFailure::AttestationNotCitable`] - the same refusal one step earlier:
//!   an attestation whose signature was never checked, or was checked and did not hold,
//!   cannot become evidence at all. Recording it would leave a report unable to tell a
//!   verified attestation from an unchecked one, which is the distinction the class
//!   exists to preserve.
//!
//! # Unrecognised classes are not failures
//!
//! `taxonomies/evidence-types.yaml` declares `openness: open` and
//! `consumersMustHandleUnknown: true`, and the schema spells out the consequence: "An
//! unrecognised term requires the consumer to treat the evidence as UNKNOWN rather than
//! to" reject it. There is therefore no variant for an unknown class. Such a record is
//! stored, marked unrecognised, and permitted to support nothing - which is what treating
//! it as `UNKNOWN` means in practice, and is the honest reading rather than either
//! discarding the record or pretending to interpret it.

use std::fmt;

use amasario_core::{EngineError, ErrorCategory, Result};

/// A way an evidence record can fail to be citable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EvidenceFailure {
    /// The record states no claim, or one too short to be a claim.
    ClaimNotStated {
        /// The record's identifier.
        evidence: String,
        /// How many characters the claim holds.
        length: usize,
    },
    /// The record identifies nothing it can be traced back to.
    NotTraceable {
        /// The record's identifier.
        evidence: String,
        /// The class that requires it.
        class: String,
        /// What a reviewer would have to be able to consult.
        traceable_to: String,
    },
    /// The class is derived from network observation and no boundary was recorded.
    BoundaryUnrecorded {
        /// The record's identifier.
        evidence: String,
        /// The class that requires a boundary.
        class: String,
    },
    /// A transaction record does not state whether the transaction succeeded.
    TransactionOutcomeUnrecorded {
        /// The transaction that was cited.
        transaction: String,
    },
    /// A failed transaction was cited as evidence that its effect occurred.
    FailedTransactionCitedAsEffect {
        /// The transaction that failed.
        transaction: String,
        /// The claim it was cited for.
        claim: String,
    },
    /// Content-addressable evidence records no digest.
    DigestUnrecorded {
        /// The record's identifier.
        evidence: String,
        /// The class that requires one.
        class: String,
    },
    /// Content-addressable evidence records no artifact class.
    ArtifactTypeUnrecorded {
        /// The record's identifier.
        evidence: String,
    },
    /// Build evidence records no toolchain identity.
    ToolchainUnrecorded {
        /// The record's identifier.
        evidence: String,
    },
    /// Build evidence records no configuration digest.
    ConfigurationUnrecorded {
        /// The record's identifier.
        evidence: String,
    },
    /// An event record identifies neither its emitting transaction nor its index.
    EventOriginUnrecorded {
        /// The record's identifier.
        evidence: String,
    },
    /// An attestation record does not state which attestation it refers to.
    AttestationUnrecorded {
        /// The record's identifier.
        evidence: String,
    },
    /// An attestation that cannot be cited was offered as evidence.
    AttestationNotCitable {
        /// The attestation's identifier.
        attestation: String,
        /// Who issued it.
        issuer: String,
        /// What happened to its signature.
        state: String,
    },
    /// An attestation was cited for a claim it does not state.
    AttestationClaimMismatch {
        /// The attestation that was cited.
        attestation: String,
        /// The claim that was cited for.
        claim: String,
        /// The claim the attestation actually states.
        attested: String,
    },
    /// An observation record does not state what was observed.
    ObservationNoteUnrecorded {
        /// The record's identifier.
        evidence: String,
    },
    /// A record's note is longer than the schema permits.
    NoteTooLong {
        /// The record's identifier.
        evidence: String,
        /// How many characters the note holds.
        length: usize,
    },
    /// A digest was computed under an algorithm the specification does not define.
    DigestAlgorithmUnsupported {
        /// The record's identifier.
        evidence: String,
        /// The algorithm that was used.
        algorithm: String,
    },
    /// Two records share an identifier.
    DuplicateEvidenceId {
        /// The identifier that appeared twice.
        id: String,
    },
    /// A record states no claim for a class that requires one to be evaluable.
    NoSupportingEvidence {
        /// The claim that was evaluated.
        claim: String,
        /// The classes the claim requires.
        required: String,
    },
    /// A record and its subject disagree about a fact the class makes decisive.
    DecisiveDisagreement {
        /// The record's identifier.
        evidence: String,
        /// The field that disagrees.
        field: String,
        /// What the record states.
        recorded: String,
        /// What the claim states.
        claimed: String,
    },
    /// A record cites a contradiction with a record that is not present.
    ContradictionUnresolvable {
        /// The record that cites the contradiction.
        evidence: String,
        /// The identifier it cites.
        cited: String,
    },
}

impl EvidenceFailure {
    /// The error category every evidence failure reports.
    #[must_use]
    pub const fn category(&self) -> ErrorCategory {
        ErrorCategory::Provenance
    }

    /// Whether this is a structural refusal rather than a gap.
    ///
    /// A refusal means the record cannot be made citable by observing more: a failed
    /// transaction does not become successful, and an attestation does not broaden to
    /// cover a claim it never stated. The distinction is the one a caller acts on.
    #[must_use]
    pub const fn is_refusal(&self) -> bool {
        matches!(
            self,
            Self::FailedTransactionCitedAsEffect { .. }
                | Self::AttestationClaimMismatch { .. }
                | Self::AttestationNotCitable { .. }
                | Self::NoteTooLong { .. }
                | Self::DigestAlgorithmUnsupported { .. }
                | Self::DecisiveDisagreement { .. }
                | Self::DuplicateEvidenceId { .. }
        )
    }

    /// Whether more evidence could make the claim citable.
    #[must_use]
    pub const fn is_completable_by_evidence(&self) -> bool {
        matches!(
            self,
            Self::ClaimNotStated { .. }
                | Self::NotTraceable { .. }
                | Self::BoundaryUnrecorded { .. }
                | Self::TransactionOutcomeUnrecorded { .. }
                | Self::DigestUnrecorded { .. }
                | Self::ArtifactTypeUnrecorded { .. }
                | Self::ToolchainUnrecorded { .. }
                | Self::ConfigurationUnrecorded { .. }
                | Self::EventOriginUnrecorded { .. }
                | Self::AttestationUnrecorded { .. }
                | Self::ObservationNoteUnrecorded { .. }
                | Self::NoSupportingEvidence { .. }
                | Self::ContradictionUnresolvable { .. }
        )
    }

    /// The engine error this failure becomes.
    #[must_use]
    pub fn into_error(self) -> EngineError {
        EngineError::Provenance(self.to_string())
    }
}

impl fmt::Display for EvidenceFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ClaimNotStated { evidence, length } => write!(
                f,
                "evidence {evidence:?} states a {length}-character claim; a record exists to \
                 support a specific claim, and one that does not state what it supports cannot \
                 be cited by the claim it was collected for"
            ),
            Self::NotTraceable {
                evidence,
                class,
                traceable_to,
            } => write!(
                f,
                "{class} evidence {evidence:?} identifies nothing a reviewer could consult; \
                 evidence of this class must be traceable to {traceable_to}"
            ),
            Self::BoundaryUnrecorded { evidence, class } => write!(
                f,
                "{class} evidence {evidence:?} records no observation boundary; a ledger \
                 sequence is only meaningful against a named chain, so a record without one \
                 states a number that means nothing on its own"
            ),
            Self::TransactionOutcomeUnrecorded { transaction } => write!(
                f,
                "transaction evidence for {transaction} does not state whether the transaction \
                 succeeded; an unknown outcome is not a successful one, and the schema requires \
                 the field for this class for exactly that reason"
            ),
            Self::FailedTransactionCitedAsEffect { transaction, claim } => write!(
                f,
                "transaction {transaction} failed and is cited as evidence that {claim}; a \
                 failed transaction is evidence of the attempt, not of the effect"
            ),
            Self::DigestUnrecorded { evidence, class } => write!(
                f,
                "{class} evidence {evidence:?} records no digest; a content address is \
                 meaningless without the content it was computed over, so this evidence asserts \
                 an identity a reviewer cannot recompute"
            ),
            Self::ArtifactTypeUnrecorded { evidence } => write!(
                f,
                "artifact evidence {evidence:?} does not say which class of artifact the digest \
                 was computed over; a digest of a lockfile and a digest of an executable are both \
                 digests, and the type is what makes the comparison meaningful"
            ),
            Self::ToolchainUnrecorded { evidence } => write!(
                f,
                "build evidence {evidence:?} records no toolchain identity; two builds of one \
                 revision with different compilers produce different artifacts, so a build record \
                 without a toolchain cannot explain the artifact it claims to have produced"
            ),
            Self::ConfigurationUnrecorded { evidence } => write!(
                f,
                "build evidence {evidence:?} records no configuration digest; a build is a \
                 function of its inputs, and configuration is an input"
            ),
            Self::EventOriginUnrecorded { evidence } => write!(
                f,
                "event evidence {evidence:?} records neither the transaction that emitted it nor \
                 its index; an event without its emitting transaction cannot be located, and \
                 without its index it cannot be distinguished from its siblings"
            ),
            Self::AttestationUnrecorded { evidence } => write!(
                f,
                "attestation evidence {evidence:?} does not name the attestation it refers to; \
                 the issuer and the exact claim text are what a reviewer consults, and neither \
                 can be recovered from a flag"
            ),
            Self::AttestationNotCitable {
                attestation,
                issuer,
                state,
            } => write!(
                f,
                "the attestation {attestation} from {issuer} cannot be cited as evidence: its \
                 signature is {state}, and only a verified signature supports a claim"
            ),
            Self::AttestationClaimMismatch {
                attestation,
                claim,
                attested,
            } => write!(
                f,
                "attestation {attestation} was cited for {claim} while it states {attested}; an \
                 attestation supports the specific claim it states and nothing broader"
            ),
            Self::ObservationNoteUnrecorded { evidence } => write!(
                f,
                "observation evidence {evidence:?} does not state what was observed; this class \
                 asserts presence only, so the note is the whole of its content"
            ),
            Self::NoteTooLong { evidence, length } => write!(
                f,
                "evidence {evidence:?} carries a {length}-character note, and the schema bounds a \
                 note at 2048 characters; a document this engine emitted would be refused by a \
                 reader validating it against that schema"
            ),
            Self::DigestAlgorithmUnsupported {
                evidence,
                algorithm,
            } => write!(
                f,
                "evidence {evidence:?} records a {algorithm} digest; the specification defines \
                 SHA-256, and a digest under another algorithm cannot be compared with one it \
                 defines"
            ),
            Self::DuplicateEvidenceId { id } => write!(
                f,
                "evidence identifier {id:?} appears more than once; claims reference evidence by \
                 identifier, so a duplicate would make a citation ambiguous between two records"
            ),
            Self::NoSupportingEvidence { claim, required } => write!(
                f,
                "the claim {claim:?} cites no evidence; it requires {required}, and a claim with \
                 no evidence is an assertion"
            ),
            Self::DecisiveDisagreement {
                evidence,
                field,
                recorded,
                claimed,
            } => write!(
                f,
                "evidence {evidence:?} records {field} as {recorded:?} while the claim states \
                 {claimed:?}; a deterministically comparable record that disagrees is a \
                 contradiction, and reporting it as verified would be the most damaging output \
                 the engine could produce"
            ),
            Self::ContradictionUnresolvable { evidence, cited } => write!(
                f,
                "evidence {evidence:?} cites a contradiction with {cited:?}, which is not present \
                 in the collected set; a conflict names two records, and one of them missing \
                 means the conflict cannot be evaluated"
            ),
        }
    }
}

/// Describes a failure as a sentence for a report.
#[must_use]
pub fn describe(failure: &EvidenceFailure) -> String {
    failure.to_string()
}

/// Reports the first failure in a list, or succeeds if there are none.
///
/// # Errors
///
/// Returns the first failure converted to an engine error.
pub fn first_failure(failures: Vec<EvidenceFailure>) -> Result<()> {
    match failures.into_iter().next() {
        Some(failure) => Err(failure.into_error()),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One of every variant, so the coverage tests below cannot silently stop covering a
    /// new one.
    fn every_failure() -> Vec<EvidenceFailure> {
        vec![
            EvidenceFailure::ClaimNotStated {
                evidence: "e-1".to_owned(),
                length: 2,
            },
            EvidenceFailure::NotTraceable {
                evidence: "e-1".to_owned(),
                class: "SOURCE".to_owned(),
                traceable_to: "the repository at the recorded revision".to_owned(),
            },
            EvidenceFailure::BoundaryUnrecorded {
                evidence: "e-1".to_owned(),
                class: "WASM".to_owned(),
            },
            EvidenceFailure::TransactionOutcomeUnrecorded {
                transaction: "ab".repeat(32),
            },
            EvidenceFailure::FailedTransactionCitedAsEffect {
                transaction: "ab".repeat(32),
                claim: "the contract was deployed".to_owned(),
            },
            EvidenceFailure::DigestUnrecorded {
                evidence: "e-1".to_owned(),
                class: "ARTIFACT".to_owned(),
            },
            EvidenceFailure::ArtifactTypeUnrecorded {
                evidence: "e-1".to_owned(),
            },
            EvidenceFailure::ToolchainUnrecorded {
                evidence: "e-1".to_owned(),
            },
            EvidenceFailure::ConfigurationUnrecorded {
                evidence: "e-1".to_owned(),
            },
            EvidenceFailure::EventOriginUnrecorded {
                evidence: "e-1".to_owned(),
            },
            EvidenceFailure::AttestationUnrecorded {
                evidence: "e-1".to_owned(),
            },
            EvidenceFailure::AttestationNotCitable {
                attestation: "att-1".to_owned(),
                issuer: "an issuer".to_owned(),
                state: "PRESENT_UNVERIFIED".to_owned(),
            },
            EvidenceFailure::AttestationClaimMismatch {
                attestation: "att-1".to_owned(),
                claim: "the artifact is safe".to_owned(),
                attested: "the artifact was built from revision abc".to_owned(),
            },
            EvidenceFailure::ObservationNoteUnrecorded {
                evidence: "e-1".to_owned(),
            },
            EvidenceFailure::NoteTooLong {
                evidence: "e-1".to_owned(),
                length: 2_049,
            },
            EvidenceFailure::DigestAlgorithmUnsupported {
                evidence: "e-1".to_owned(),
                algorithm: "SHA-512".to_owned(),
            },
            EvidenceFailure::DuplicateEvidenceId {
                id: "e-1".to_owned(),
            },
            EvidenceFailure::NoSupportingEvidence {
                claim: "c-1".to_owned(),
                required: "ARTIFACT, BUILD".to_owned(),
            },
            EvidenceFailure::DecisiveDisagreement {
                evidence: "e-1".to_owned(),
                field: "digest".to_owned(),
                recorded: "ab".to_owned(),
                claimed: "cd".to_owned(),
            },
            EvidenceFailure::ContradictionUnresolvable {
                evidence: "e-1".to_owned(),
                cited: "e-2".to_owned(),
            },
        ]
    }

    #[test]
    fn every_failure_has_a_category_a_description_and_a_code() {
        let failures = every_failure();
        for failure in &failures {
            assert_eq!(failure.category(), ErrorCategory::Provenance);
            assert_eq!(
                failure.category(),
                failure.clone().into_error().category(),
                "{failure:?} disagrees with its own error"
            );
            assert!(!describe(failure).is_empty());
            assert!(!failure.clone().into_error().code().is_empty());
        }
        assert_eq!(failures.len(), 20, "every variant is covered by this test");
    }

    #[test]
    fn each_failure_is_either_a_refusal_or_a_gap() {
        for failure in every_failure() {
            assert_ne!(
                failure.is_refusal(),
                failure.is_completable_by_evidence(),
                "{failure:?} is not classified exactly once"
            );
        }
    }

    #[test]
    fn a_failed_transaction_says_what_it_cannot_be_evidence_of() {
        let error = EvidenceFailure::FailedTransactionCitedAsEffect {
            transaction: "ab".to_owned(),
            claim: "the contract was deployed".to_owned(),
        }
        .into_error();
        let message = error.to_string();
        assert!(
            message.contains("evidence of the attempt"),
            "got: {message}"
        );
        assert!(message.contains("not of the effect"), "got: {message}");
    }

    #[test]
    fn a_note_over_the_schema_maximum_says_what_the_bound_is() {
        let error = EvidenceFailure::NoteTooLong {
            evidence: "e-1".to_owned(),
            length: 4_096,
        }
        .into_error();
        let message = error.to_string();
        assert!(message.contains("4096"), "got: {message}");
        assert!(message.contains("2048"), "got: {message}");
    }

    #[test]
    fn an_uncitable_attestation_says_which_state_stopped_it() {
        let error = EvidenceFailure::AttestationNotCitable {
            attestation: "att-1".to_owned(),
            issuer: "an issuer".to_owned(),
            state: "PRESENT_UNVERIFIED".to_owned(),
        }
        .into_error();
        let message = error.to_string();
        assert!(message.contains("att-1"), "got: {message}");
        assert!(message.contains("an issuer"), "got: {message}");
        assert!(
            message.contains("PRESENT_UNVERIFIED"),
            "the refusal must name the state, because 'not citable' is not actionable: {message}"
        );
    }

    #[test]
    fn an_attestation_refusal_quotes_the_scope_rule() {
        let error = EvidenceFailure::AttestationClaimMismatch {
            attestation: "att-1".to_owned(),
            claim: "the artifact is safe".to_owned(),
            attested: "the artifact was built from revision abc".to_owned(),
        }
        .into_error();
        assert!(
            error
                .to_string()
                .contains("specific claim it states and nothing broader"),
            "got: {error}"
        );
    }

    #[test]
    fn an_empty_failure_list_succeeds_and_a_populated_one_reports_its_first() {
        first_failure(Vec::new()).expect("nothing to report");
        let error = first_failure(vec![EvidenceFailure::DuplicateEvidenceId {
            id: "e-1".to_owned(),
        }])
        .expect_err("a failure must be reported");
        assert!(error.to_string().contains("more than once"));
    }
}
