//! The ways a snapshot or a diff can fail to be assertable, and how each is classified.
//!
//! # Why a snapshot needs its own failure model
//!
//! A snapshot is the artefact an analysis is judged by: it is what gets stored, compared
//! and cited later, and it is the only thing a reader has when the network is no longer
//! reachable. `schema/snapshot.schema.json` therefore pins requirements that are not
//! stylistic - a boundary echoed at the top level that must equal the boundary it came
//! from, a content digest that excludes declared volatile fields, an evidence array that
//! is required and non-empty - and every one of them is a variant below.
//!
//! The four that carry the most weight:
//!
//! * [`SnapshotFailure::BoundaryEchoDisagrees`] - `rules/provenance/contract-to-deployment`
//!   requires `network` to equal `boundary.network` and `ledgerBoundary` to equal
//!   `boundary.ledger`. The duplication exists so that a snapshot is self-describing, and
//!   a self-description that disagrees with its own boundary is worse than none: a reader
//!   deciding whether an analysis is current would use the wrong one of the two.
//! * [`SnapshotFailure::ContentDigestDisagrees`] - the digest is what a diff uses to decide
//!   whether anything changed at all, and a digest that does not match the content it
//!   summarises turns that decision into a coin flip.
//! * [`SnapshotFailure::VolatileFieldUndeclared`] - `capturedAt` is excluded from the
//!   digest, and the exclusion is declared in `volatileFields` "so that a consumer can
//!   recompute the digest itself and get the same answer". An undeclared exclusion is a
//!   silent difference between the engine's digest and a consumer's.
//! * [`SnapshotFailure::NoEvidenceRecorded`] - a snapshot with no evidence "can support no
//!   claim", so an empty evidence array is a document that looks authoritative and is not.
//!
//! # Why a diff is refused rather than downgraded
//!
//! `rules/impact/change-impact` states the requirement as a MUST: a diff "MUST be produced
//! by canonical comparison, MUST NOT report a difference caused only by ordering or by a
//! volatile field, MUST classify every difference with a category and a change type, and
//! MUST state a reason for each". A diff is the artefact most likely to be consumed
//! without review - it is what a pipeline alerts on - so a single spurious entry makes the
//! whole comparison untrustworthy. Producing such a diff and labelling it as best-effort
//! would be exactly the failure the rule exists to prevent, which is why the failures here
//! are refusals rather than caveats wherever the rule speaks in MUSTs.

use std::fmt;

use amasario_core::{EngineError, ErrorCategory, Result};

/// A way a snapshot or a diff can fail to be assertable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SnapshotFailure {
    /// A snapshot carries no identifier.
    SnapshotIdMissing,
    /// A snapshot's identifier is not the one its contract and boundary derive.
    SnapshotIdNotStable {
        /// The identifier that was recorded.
        recorded: String,
        /// The identifier the contract and boundary derive.
        derived: String,
    },
    /// The top-level boundary echo disagrees with the boundary it echoes.
    BoundaryEchoDisagrees {
        /// The field that disagrees, as a JSON pointer.
        field: String,
        /// The value recorded at the top level.
        scattered: String,
        /// The value the boundary holds.
        boundary: String,
    },
    /// A snapshot records no evidence.
    NoEvidenceRecorded,
    /// The content digest disagrees with the content it summarises.
    ContentDigestDisagrees {
        /// The digest that was recorded.
        recorded: String,
        /// The digest the content produces.
        computed: String,
    },
    /// A volatile field's exclusion from the content digest was not declared.
    VolatileFieldUndeclared {
        /// The field path that should have been declared.
        field: String,
    },
    /// A declared volatile field is not one this engine excludes.
    VolatileFieldUnknown {
        /// The field path that was declared.
        field: String,
    },
    /// A field that is part of the digest was declared volatile.
    ///
    /// The failure exists because the declaration is a statement about what may safely be
    /// excluded, and a producer excluding the boundary or the contract identity would make
    /// two different analyses compare equal.
    VolatileFieldNotVolatile {
        /// The field path that was declared.
        field: String,
    },
    /// A digest was computed under an algorithm the specification does not define.
    DigestAlgorithmUnsupported {
        /// The algorithm that was used.
        algorithm: String,
    },
    /// A loaded snapshot's sets are not in canonical order.
    ///
    /// Checked on load rather than silently sorted, because canonical order is what makes
    /// two captures of one state compare equal, and re-sorting a document a producer
    /// emitted would hide the producer's disagreement with the specification instead of
    /// reporting it.
    NotCanonicallyOrdered {
        /// The section that is out of order, as a JSON pointer.
        section: String,
    },
    /// A snapshot could not be parsed.
    Malformed {
        /// Where the document came from.
        source: String,
        /// What the parser reported.
        detail: String,
    },
    /// A stored snapshot was produced under a specification version this engine cannot
    /// interpret.
    SpecVersionIncompatible {
        /// The version the snapshot declares.
        found: String,
        /// The version this engine implements.
        supported: String,
    },
    /// A diff compares two snapshots that are not comparable, so its changes are not
    /// meaningful.
    Incomparable {
        /// Why they are not comparable, in the diff's own vocabulary.
        reason: String,
    },
    /// A diff that is not comparable carries changes anyway.
    ChangesWithIncomparable {
        /// How many changes were reported.
        changes: usize,
    },
    /// A diff reports a change whose category is outside the closed vocabulary.
    ChangeCategoryUnknown {
        /// The category that was reported.
        category: String,
    },
    /// A diff's summary disagrees with its own changes array.
    SummaryDisagrees {
        /// The total the summary claims.
        claimed: usize,
        /// The number of entries the changes array holds.
        actual: usize,
    },
    /// A diff entry states no reason.
    ReasonNotStated {
        /// The change's identifier.
        change: String,
    },
    /// Two diff entries carry the same identifier.
    DuplicateChangeId {
        /// The identifier that appeared twice.
        id: String,
    },
    /// A diff produced under a mode that is not canonical was offered as a result.
    ComparisonModeNotCanonical {
        /// The mode that was used.
        mode: String,
    },
    /// The before boundary is later than the after boundary.
    BoundaryOrderInvalid {
        /// The before ledger.
        before: u64,
        /// The after ledger.
        after: u64,
    },
}

impl SnapshotFailure {
    /// The error category every snapshot failure reports.
    #[must_use]
    pub const fn category(&self) -> ErrorCategory {
        ErrorCategory::Snapshot
    }

    /// Whether this failure is a refusal rather than a call for more evidence.
    ///
    /// A refusal means the document contradicts a requirement and cannot be repaired by
    /// collecting more: a digest that does not match its content, an identifier that is
    /// not the one the inputs derive, or a summary that disagrees with the array it
    /// summarises. A caller that retried with a wider boundary would get the same answer.
    #[must_use]
    pub const fn is_refusal(&self) -> bool {
        matches!(
            self,
            Self::BoundaryEchoDisagrees { .. }
                | Self::ContentDigestDisagrees { .. }
                | Self::SnapshotIdNotStable { .. }
                | Self::VolatileFieldUndeclared { .. }
                | Self::VolatileFieldUnknown { .. }
                | Self::VolatileFieldNotVolatile { .. }
                | Self::NotCanonicallyOrdered { .. }
                | Self::SummaryDisagrees { .. }
                | Self::DuplicateChangeId { .. }
                | Self::Incomparable { .. }
                | Self::ChangesWithIncomparable { .. }
                | Self::ChangeCategoryUnknown { .. }
                | Self::ComparisonModeNotCanonical { .. }
                | Self::BoundaryOrderInvalid { .. }
                | Self::SpecVersionIncompatible { .. }
                | Self::DigestAlgorithmUnsupported { .. }
        )
    }

    /// Whether the document is incomplete rather than contradicted.
    ///
    /// The two remaining failures are about a gap a producer can close by recording what it
    /// observed: a snapshot with no evidence, and a diff entry with no reason. Neither
    /// becomes true by observing more, and both are fixed by writing down what was already
    /// known - which is why they are gaps rather than refusals.
    #[must_use]
    pub const fn is_completable_by_evidence(&self) -> bool {
        matches!(
            self,
            Self::SnapshotIdMissing
                | Self::NoEvidenceRecorded
                | Self::ReasonNotStated { .. }
                | Self::Malformed { .. }
        )
    }

    /// The engine error this failure becomes.
    ///
    /// Wrapped in [`EngineError::Snapshot`] rather than a generic validation error so that
    /// a caller can tell a snapshot refusal from malformed input. The distinction is the
    /// one a pipeline acts on: a malformed document is a transport or storage problem, and
    /// a content digest that disagrees is a producer that disagreed with the specification.
    #[must_use]
    pub fn into_error(self) -> EngineError {
        EngineError::Snapshot(self.to_string())
    }
}

impl fmt::Display for SnapshotFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SnapshotIdMissing => write!(
                f,
                "the snapshot carries no identifier; an identifier derived from the contract \
                 and the boundary is what lets two captures of one state be recognised as the \
                 same capture rather than as two"
            ),
            Self::SnapshotIdNotStable { recorded, derived } => write!(
                f,
                "the snapshot's identifier is {recorded:?} but its contract and boundary derive \
                 {derived:?}; the identifier is required to be stable given the same contract \
                 and boundary, so a disagreement means the document was edited or the \
                 derivation changed"
            ),
            Self::BoundaryEchoDisagrees {
                field,
                scattered,
                boundary,
            } => write!(
                f,
                "the snapshot records {field} as {scattered:?} while the boundary it came from \
                 holds {boundary:?}; the top-level copy exists so that a snapshot is \
                 self-describing, and a self-description that disagrees with its own boundary \
                 leaves a reader choosing which of two answers to believe"
            ),
            Self::NoEvidenceRecorded => write!(
                f,
                "the snapshot records no evidence; a snapshot with no evidence can support no \
                 claim, so an empty set would be a document that looks authoritative and is not"
            ),
            Self::ContentDigestDisagrees { recorded, computed } => write!(
                f,
                "the snapshot's content digest is {recorded} but its content produces {computed}; \
                 the digest is what a diff uses to decide whether anything changed at all, and \
                 one that does not match its content makes that decision arbitrary"
            ),
            Self::VolatileFieldUndeclared { field } => write!(
                f,
                "{field} is excluded from the content digest but is not declared in \
                 volatileFields; the declaration exists so that a consumer can recompute the \
                 digest and get the same answer, and an undeclared exclusion silently makes the \
                 engine's digest and the consumer's differ"
            ),
            Self::VolatileFieldUnknown { field } => write!(
                f,
                "volatileFields declares {field}, which is not a field this engine excludes from \
                 the content digest; a declaration that does not describe what was done is worse \
                 than no declaration, because a consumer would exclude it and disagree with the \
                 digest it recomputed"
            ),
            Self::VolatileFieldNotVolatile { field } => write!(
                f,
                "volatileFields declares {field}, which is part of the content digest; excluding \
                 it would make two different analyses compare equal, which is the failure the \
                 digest is computed for"
            ),
            Self::DigestAlgorithmUnsupported { algorithm } => write!(
                f,
                "a digest was computed under {algorithm}, and the specification defines SHA-256 \
                 only; a digest in another algorithm cannot be compared with one that is"
            ),
            Self::NotCanonicallyOrdered { section } => write!(
                f,
                "the snapshot's {section} is not in canonical order; canonical order is what \
                 makes two captures of one state compare equal, and re-sorting a document a \
                 producer emitted would hide that producer's disagreement with the \
                 specification instead of reporting it"
            ),
            Self::Malformed { source, detail } => {
                write!(f, "the snapshot at {source} could not be read: {detail}")
            },
            Self::SpecVersionIncompatible { found, supported } => write!(
                f,
                "the snapshot declares specification version {found} and this engine implements \
                 {supported}; the specification requires that a consumer not silently interpret \
                 a version it does not know, because changed semantics would corrupt the \
                 comparison rather than fail it"
            ),
            Self::Incomparable { reason } => write!(
                f,
                "the two snapshots are not comparable because {reason}; an empty diff is \
                 indistinguishable from \"nothing changed\", so an incomparable pair must be \
                 reported as incomparable rather than as unchanged"
            ),
            Self::ChangesWithIncomparable { changes } => write!(
                f,
                "the diff reports that its snapshots are not comparable and lists {changes} \
                 change(s) anyway; a difference computed across incomparable documents describes \
                 the difference between the documents' formats rather than between the states \
                 they describe"
            ),
            Self::ChangeCategoryUnknown { category } => write!(
                f,
                "a change reports category {category}, which is outside the closed enumeration; \
                 the enumeration is closed so that a consumer can subscribe to categories \
                 without parsing free text, and an unrecognised one cannot be subscribed to"
            ),
            Self::SummaryDisagrees { claimed, actual } => write!(
                f,
                "the diff summarises {claimed} change(s) while its changes array holds {actual}; \
                 the summary is recorded so that it cannot disagree with the array, and a \
                 consumer filtering on one while counting the other would reach two answers"
            ),
            Self::ReasonNotStated { change } => write!(
                f,
                "diff entry {change:?} states no reason; a reason is what lets a reviewer reject \
                 an entry that turns out to be noise, which is the only defence an automatically \
                 consumed diff has"
            ),
            Self::DuplicateChangeId { id } => write!(
                f,
                "two diff entries carry identifier {id}; the identifier is deterministic given \
                 the same inputs so that a diff can itself be diffed, and a duplicate means two \
                 differences were reported as one"
            ),
            Self::ComparisonModeNotCanonical { mode } => write!(
                f,
                "the diff was produced in {mode} mode; only canonical comparison is \
                 deterministic, and the other modes are recorded so that an implementation that \
                 compared differently says so rather than presenting the result as canonical"
            ),
            Self::BoundaryOrderInvalid { before, after } => write!(
                f,
                "the before snapshot is bounded at ledger {before} and the after snapshot at \
                 {after}; a diff describes what changed between two states, so the earlier one \
                 must be the before and the comparison cannot be reversed"
            ),
        }
    }
}

/// Reports the first failure in a list, or succeeds if there are none.
///
/// # Errors
///
/// Returns the first failure converted to an engine error. Taking the first rather than
/// merging them keeps one document's diagnostic specific; a caller that wants every
/// failure keeps the list and reads it.
pub fn first_failure(failures: Vec<SnapshotFailure>) -> Result<()> {
    match failures.into_iter().next() {
        Some(failure) => Err(failure.into_error()),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One of every variant, so that the coverage tests below cannot silently stop
    /// covering a new one.
    fn every_failure() -> Vec<SnapshotFailure> {
        vec![
            SnapshotFailure::SnapshotIdMissing,
            SnapshotFailure::SnapshotIdNotStable {
                recorded: "snapshot-a".to_owned(),
                derived: "snapshot-b".to_owned(),
            },
            SnapshotFailure::BoundaryEchoDisagrees {
                field: "/network".to_owned(),
                scattered: "testnet".to_owned(),
                boundary: "mainnet".to_owned(),
            },
            SnapshotFailure::NoEvidenceRecorded,
            SnapshotFailure::ContentDigestDisagrees {
                recorded: "sha256:00".to_owned(),
                computed: "sha256:11".to_owned(),
            },
            SnapshotFailure::VolatileFieldUndeclared {
                field: "/capturedAt".to_owned(),
            },
            SnapshotFailure::VolatileFieldUnknown {
                field: "/nodes".to_owned(),
            },
            SnapshotFailure::VolatileFieldNotVolatile {
                field: "/contract".to_owned(),
            },
            SnapshotFailure::DigestAlgorithmUnsupported {
                algorithm: "SHA-512".to_owned(),
            },
            SnapshotFailure::NotCanonicallyOrdered {
                section: "/evidence".to_owned(),
            },
            SnapshotFailure::Malformed {
                source: "snapshot.json".to_owned(),
                detail: "expected value at line 1".to_owned(),
            },
            SnapshotFailure::SpecVersionIncompatible {
                found: "2.0.0".to_owned(),
                supported: "1.0.0".to_owned(),
            },
            SnapshotFailure::Incomparable {
                reason: "NETWORK_MISMATCH".to_owned(),
            },
            SnapshotFailure::ChangesWithIncomparable { changes: 3 },
            SnapshotFailure::ChangeCategoryUnknown {
                category: "SOMETHING_ELSE".to_owned(),
            },
            SnapshotFailure::SummaryDisagrees {
                claimed: 4,
                actual: 3,
            },
            SnapshotFailure::ReasonNotStated {
                change: "change-1".to_owned(),
            },
            SnapshotFailure::DuplicateChangeId {
                id: "change-1".to_owned(),
            },
            SnapshotFailure::ComparisonModeNotCanonical {
                mode: "ORDER_SENSITIVE".to_owned(),
            },
            SnapshotFailure::BoundaryOrderInvalid {
                before: 2_000,
                after: 1_000,
            },
        ]
    }

    #[test]
    fn every_failure_has_a_category_a_description_and_a_code() {
        let failures = every_failure();
        for failure in &failures {
            assert_eq!(failure.category(), ErrorCategory::Snapshot);
            assert_eq!(
                failure.category(),
                failure.clone().into_error().category(),
                "{failure:?} disagrees with its own error"
            );
            assert!(!failure.to_string().is_empty());
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
    fn a_boundary_echo_that_disagrees_names_both_values() {
        let error = SnapshotFailure::BoundaryEchoDisagrees {
            field: "/ledgerBoundary".to_owned(),
            scattered: "1000".to_owned(),
            boundary: "1001".to_owned(),
        }
        .into_error();
        let message = error.to_string();
        assert!(message.contains("/ledgerBoundary"), "got: {message}");
        assert!(message.contains("1000"), "got: {message}");
        assert!(message.contains("1001"), "got: {message}");
    }

    #[test]
    fn a_digest_disagreement_is_a_refusal_rather_than_a_gap() {
        // Nothing a producer can observe changes the digest of content it has already
        // written, so a caller must not retry.
        assert!(
            SnapshotFailure::ContentDigestDisagrees {
                recorded: "sha256:00".to_owned(),
                computed: "sha256:11".to_owned(),
            }
            .is_refusal()
        );
    }

    #[test]
    fn an_empty_failure_list_succeeds_and_a_populated_one_reports_its_first() {
        first_failure(Vec::new()).expect("nothing to report");
        let error = first_failure(vec![SnapshotFailure::ReasonNotStated {
            change: "change-1".to_owned(),
        }])
        .expect_err("a failure must be reported");
        assert!(error.to_string().contains("change-1"));
    }
}
