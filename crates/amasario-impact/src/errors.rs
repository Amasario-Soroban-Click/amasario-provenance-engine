//! The ways an impact finding can fail to be assertable, and how each is classified.
//!
//! # Why every rule becomes a variant here
//!
//! Impact analysis is the layer where the specification's discipline is easiest to
//! lose: the output is a list of entities, and a list is exactly the shape that
//! invites a producer to add a plausible entry without recording why. The rules in
//! `rules/impact/` therefore state mechanical checks - a depth-one finding must carry
//! `DIRECT`, a path must hold one more node than it has steps, an aggregate confidence
//! must equal the weakest link - and every one of those checks is a variant below.
//!
//! The four that carry the most weight:
//!
//! * [`ImpactFailure::NonPropagatingStep`] - `impact/direct-impact`: "An impact finding
//!   MUST NOT traverse a relationship whose declared change propagation is none."
//!   `OBSERVED_IN` and `VERIFIED_BY` relate a fact to the record that supports it, so
//!   traversing them would report a re-observation or a re-verification as a change to
//!   the thing observed.
//! * [`ImpactFailure::DistanceClassificationDisagrees`] - `impact/direct-impact` and
//!   `impact/transitive-impact`: the distance terms are derived from `hopDepth` and are
//!   therefore checkable, which is what stops a deep finding from being published
//!   without its depth being visible.
//! * [`ImpactFailure::ConfidenceNotWeakestLink`] - `impact/transitive-impact`: "Its
//!   aggregated confidence MUST equal the minimum confidence ordinal among its steps."
//!   Without this a long chain would present itself as more certain than its weakest
//!   edge, which is the single most misleading output an impact report can contain.
//! * [`ImpactFailure::DeploymentNotEligible`] - `impact/deployment-impact`: an impact
//!   finding "MUST NOT name a deployment whose status is `UNCONFIRMED`, `FAILED` or
//!   `UNKNOWN`". A change can only affect a deployment that took effect.
//!
//! Each variant quotes a rule rather than expressing a preference, and each is a
//! refusal by name so that a caller cannot accidentally satisfy a requirement by
//! renaming a field.

use std::fmt;

use amasario_core::{EngineError, ErrorCategory, Result};

/// A way an impact finding can fail to be assertable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImpactFailure {
    /// The finding cites no evidence, so it is a prediction rather than a finding.
    NoEvidenceCited {
        /// The finding that cites nothing.
        finding: String,
    },
    /// A step of a path cites no evidence.
    StepWithoutEvidence {
        /// The step, rendered.
        step: String,
    },
    /// A step traverses a relationship whose change propagation is none.
    NonPropagatingStep {
        /// The relationship that was traversed.
        relationship: String,
        /// The entity the step started at.
        source: String,
        /// The entity the step ended at.
        target: String,
    },
    /// The impact type set contains no term from the distance family.
    DistanceClassificationMissing {
        /// The hop depth of the finding.
        hop_depth: usize,
    },
    /// The distance terms contradict the hop depth.
    DistanceClassificationDisagrees {
        /// The hop depth of the finding.
        hop_depth: usize,
        /// The terms that were claimed.
        claimed: String,
        /// What the depth requires.
        required: String,
    },
    /// A finding with a non-zero depth carries no path.
    PathAbsent {
        /// The finding whose path is missing.
        finding: String,
        /// The hop depth it claims.
        hop_depth: usize,
    },
    /// A path's node and step counts do not differ by one.
    PathLengthDisagrees {
        /// How many nodes the path holds.
        nodes: usize,
        /// How many steps it holds.
        steps: usize,
    },
    /// A path's own depth disagrees with the finding's.
    PathDepthDisagrees {
        /// The depth the finding claims.
        hop_depth: usize,
        /// The depth the path holds.
        path_depth: usize,
    },
    /// The finding's relationship list disagrees with the path's steps.
    RelationshipListDisagrees {
        /// The relationships the finding declared.
        declared: String,
        /// The relationships the path actually traversed.
        traversed: String,
    },
    /// The impact type set includes `CHANGE` without a change type.
    ChangeTypeMissing {
        /// The finding that failed to name its change.
        finding: String,
    },
    /// The impact type set includes `CHANGE` for a depth that cannot mean it, or a
    /// change type is present while `CHANGE` is absent.
    ChangeClassificationDisagrees {
        /// The hop depth of the finding.
        hop_depth: usize,
        /// Whether `CHANGE` was present in the impact type set.
        claims_change: bool,
        /// Whether a change type was present.
        has_change_type: bool,
    },
    /// The finding's confidence is not the minimum ordinal among its steps.
    ConfidenceNotWeakestLink {
        /// The level the finding reported.
        reported: String,
        /// The level its steps require.
        weakest: String,
    },
    /// The affected entity's kind requires a classification term that is absent.
    EntityClassificationMissing {
        /// The kind of the affected entity.
        kind: String,
        /// The term the kind requires.
        required: String,
    },
    /// The finding names a deployment that cannot be affected by a change.
    DeploymentNotEligible {
        /// The deployment that was named.
        deployment: String,
        /// Its status, from the deployment-statuses vocabulary.
        status: String,
    },
    /// A finding against a deployment did not identify the ledger it was recorded at.
    DeploymentLedgerUnrecorded {
        /// The deployment that was named.
        deployment: String,
    },
    /// The direction the finding reports disagrees with the steps it traversed.
    DirectionDisagrees {
        /// The direction the finding declared.
        declared: String,
        /// The direction the steps derive.
        derived: String,
    },
    /// The finding's reason is too short to be a reason.
    ReasonNotStated {
        /// How many characters it holds.
        length: usize,
    },
    /// The path traverses an entity more than once, so it contains a cycle.
    PathRepeatsEntity {
        /// The entity that appears twice.
        entity: String,
    },
    /// Two findings in one analysis carry the same identifier.
    ///
    /// The identifier is derived from the changed entity, the affected entity and the
    /// change type, so a duplicate means two findings claim the same change reached the
    /// same entity for the same reason. A snapshot diff would report the pair as a
    /// duplicate entry rather than as a change, and a consumer aggregating by identifier
    /// would silently drop one of them.
    DuplicateFindingId {
        /// The identifier that appeared twice.
        id: String,
    },
    /// Propagation stopped early, so the finding set is not an exhaustive one.
    PropagationTruncated {
        /// Why it stopped.
        reason: String,
        /// How many entities were reported before it did.
        reported: usize,
    },
}

impl ImpactFailure {
    /// The error category every impact failure reports.
    #[must_use]
    pub const fn category(&self) -> ErrorCategory {
        ErrorCategory::Impact
    }

    /// Whether this failure is a refusal rather than a call for more evidence.
    ///
    /// A refusal means the claim is not expressible as made - it contradicts a rule
    /// rather than lacking support. The distinction matters to a caller deciding
    /// whether to retry with a wider boundary: no amount of extra observation makes a
    /// non-propagating relationship propagate.
    #[must_use]
    pub const fn is_refusal(&self) -> bool {
        matches!(
            self,
            Self::NonPropagatingStep { .. }
                | Self::DistanceClassificationDisagrees { .. }
                | Self::PathLengthDisagrees { .. }
                | Self::PathDepthDisagrees { .. }
                | Self::RelationshipListDisagrees { .. }
                | Self::ChangeClassificationDisagrees { .. }
                | Self::ConfidenceNotWeakestLink { .. }
                | Self::EntityClassificationMissing { .. }
                | Self::DeploymentNotEligible { .. }
                | Self::PathRepeatsEntity { .. }
                | Self::DirectionDisagrees { .. }
                | Self::DuplicateFindingId { .. }
        )
    }

    /// Whether the claim is incomplete rather than contradicted.
    ///
    /// `true` for the failures that are about a gap a producer can close - by citing the
    /// evidence it has, by showing the route it took, by deriving the classification the
    /// depth requires, or by stating a reason that says something. The distinction from
    /// [`Self::is_refusal`] is the one a caller acts on: a refusal means the claim as made
    /// is not expressible, and a gap means the claim is fine but unfinished.
    #[must_use]
    pub const fn is_completable_by_evidence(&self) -> bool {
        matches!(
            self,
            Self::NoEvidenceCited { .. }
                | Self::StepWithoutEvidence { .. }
                | Self::PathAbsent { .. }
                | Self::ChangeTypeMissing { .. }
                | Self::DeploymentLedgerUnrecorded { .. }
                | Self::DistanceClassificationMissing { .. }
                | Self::ReasonNotStated { .. }
        )
    }

    /// Whether this failure means propagation stopped before exhaustion.
    ///
    /// Kept as its own predicate because it is not a defect in the finding at all: a
    /// bounded analysis is a legitimate one, and the only requirement is that it says
    /// so. A caller that treats it as an error rather than as a caveat would report a
    /// deliberate bound as a bug.
    #[must_use]
    pub const fn is_bound_not_defect(&self) -> bool {
        matches!(self, Self::PropagationTruncated { .. })
    }

    /// The engine error this failure becomes.
    ///
    /// Wrapped in [`EngineError::Impact`] rather than a generic validation error so
    /// that a caller can tell an impact refusal from malformed input. A report that
    /// collapsed the two would present a rule violation as a parsing problem, which is
    /// the opposite of what the rule is for.
    #[must_use]
    pub fn into_error(self) -> EngineError {
        EngineError::Impact(self.to_string())
    }
}

impl fmt::Display for ImpactFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoEvidenceCited { finding } => write!(
                f,
                "impact finding {finding:?} cites no evidence; a finding without evidence is a \
                 prediction, which is the one thing an impact requirement must not be"
            ),
            Self::StepWithoutEvidence { step } => write!(
                f,
                "impact step {step:?} cites no evidence; a path is only as supported as its \
                 weakest link, and a link with nothing behind it is unsupported"
            ),
            Self::NonPropagatingStep {
                relationship,
                source,
                target,
            } => write!(
                f,
                "step {source} -{relationship}-> {target} traverses a relationship whose change \
                 propagation is none; observational and verification relationships record how a \
                 fact was established, so propagating through one would report a re-observation \
                 or a re-verification as a change to the thing observed"
            ),
            Self::DistanceClassificationMissing { hop_depth } => write!(
                f,
                "impact finding at hop depth {hop_depth} carries no distance classification; \
                 every finding must say whether it is DIRECT or TRANSITIVE, because the depth is \
                 the reader's main guide to how much of the reasoning they must check"
            ),
            Self::DistanceClassificationDisagrees {
                hop_depth,
                claimed,
                required,
            } => write!(
                f,
                "impact finding at hop depth {hop_depth} claims {claimed:?} but the depth requires \
                 {required:?}; the distance terms are derived from the depth, so a disagreement \
                 means one of the two is wrong"
            ),
            Self::PathAbsent { finding, hop_depth } => write!(
                f,
                "impact finding {finding:?} at hop depth {hop_depth} carries no path; an impact \
                 claim whose route is not shown cannot be checked by a reader, which is what \
                 separates a finding from an assertion"
            ),
            Self::PathLengthDisagrees { nodes, steps } => write!(
                f,
                "impact path holds {nodes} nodes and {steps} steps; a path must hold exactly one \
                 more node than it has steps, and a mismatch means a node or a step is missing"
            ),
            Self::PathDepthDisagrees {
                hop_depth,
                path_depth,
            } => write!(
                f,
                "impact finding claims hop depth {hop_depth} while its path holds {path_depth}; \
                 the two describe one traversal and cannot differ"
            ),
            Self::RelationshipListDisagrees {
                declared,
                traversed,
            } => write!(
                f,
                "impact finding declares relationships {declared} but traverses {traversed}; a \
                 consumer aggregating by relationship must reach the same answer as one walking \
                 the path"
            ),
            Self::ChangeTypeMissing { finding } => write!(
                f,
                "impact finding {finding:?} is classified CHANGE without naming a change type; a \
                 change-triggered finding that does not say what changed cannot be acted on"
            ),
            Self::ChangeClassificationDisagrees {
                hop_depth,
                claims_change,
                has_change_type,
            } => write!(
                f,
                "impact finding at hop depth {hop_depth} has claims_change={claims_change} and \
                 has_change_type={has_change_type}; CHANGE means the finding is triggered by a \
                 specific change rather than by propagation, so the two must agree"
            ),
            Self::ConfidenceNotWeakestLink { reported, weakest } => write!(
                f,
                "impact finding reports confidence {reported} while its weakest step is \
                 {weakest}; a chain is only as strong as its weakest link and an aggregate \
                 stronger than one of its inputs manufactures confidence from unrelated evidence"
            ),
            Self::EntityClassificationMissing { kind, required } => write!(
                f,
                "impact finding against a {kind} must include {required} in its impact type set; \
                 without it a reader cannot tell a contract finding from a deployment finding, \
                 and the two lead to different remediation"
            ),
            Self::DeploymentNotEligible { deployment, status } => write!(
                f,
                "impact finding names deployment {deployment:?} whose status is {status}; an \
                 impact finding asserts that a change may reach the affected entity, so a \
                 deployment that has not been established as having taken effect cannot be \
                 affected by anything"
            ),
            Self::DeploymentLedgerUnrecorded { deployment } => write!(
                f,
                "impact finding against deployment {deployment:?} does not identify the ledger it \
                 was recorded at; a deployment is a fact about a chain at a point in its history, \
                 and without the ledger the finding cannot be checked later"
            ),
            Self::DirectionDisagrees { declared, derived } => write!(
                f,
                "impact finding declares direction {declared} while its steps derive {derived}; \
                 direction is not chosen freely but read from each relationship's change \
                 propagation"
            ),
            Self::ReasonNotStated { length } => write!(
                f,
                "impact finding's reason is {length} characters; a reason must state why the \
                 entity is considered affected in terms of the relationships traversed, because \
                 that is what replaces a score"
            ),
            Self::PathRepeatsEntity { entity } => write!(
                f,
                "impact path visits {entity} more than once, so it contains a cycle; a route that \
                 revisits an entity can be shortened, so publishing it pads the result without \
                 saying anything new"
            ),
            Self::DuplicateFindingId { id } => write!(
                f,
                "two findings in one analysis carry identifier {id}; the identifier is derived \
                 from the changed entity, the affected entity and the change type, so a \
                 duplicate means the same claim was asserted twice and a diff could not tell \
                 the second from the first"
            ),
            Self::PropagationTruncated { reason, reported } => write!(
                f,
                "impact propagation stopped after reporting {reported} entities because {reason}; \
                 further affected entities may exist, and a consumer must not read the list as \
                 exhaustive"
            ),
        }
    }
}

/// Describes a failure as a sentence for a report.
#[must_use]
pub fn describe(failure: &ImpactFailure) -> String {
    failure.to_string()
}

/// Reports the first failure in a list, or succeeds if there are none.
///
/// # Errors
///
/// Returns the first failure converted to an engine error. Taking the first rather
/// than merging them keeps one finding's diagnostic specific; a caller that wants all
/// of them keeps the list and reads it.
pub fn first_failure(failures: Vec<ImpactFailure>) -> Result<()> {
    match failures.into_iter().next() {
        Some(failure) => Err(failure.into_error()),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One of every variant, so that the coverage tests below cannot silently stop
    /// covering a new variant.
    fn every_failure() -> Vec<ImpactFailure> {
        vec![
            ImpactFailure::NoEvidenceCited {
                finding: "impact-1".to_owned(),
            },
            ImpactFailure::StepWithoutEvidence {
                step: "C-a -DEPENDS_ON-> C-b".to_owned(),
            },
            ImpactFailure::NonPropagatingStep {
                relationship: "OBSERVED_IN".to_owned(),
                source: "C-a".to_owned(),
                target: "tx".to_owned(),
            },
            ImpactFailure::DistanceClassificationMissing { hop_depth: 1 },
            ImpactFailure::DistanceClassificationDisagrees {
                hop_depth: 2,
                claimed: "DIRECT".to_owned(),
                required: "TRANSITIVE".to_owned(),
            },
            ImpactFailure::PathAbsent {
                finding: "impact-1".to_owned(),
                hop_depth: 1,
            },
            ImpactFailure::PathLengthDisagrees { nodes: 3, steps: 1 },
            ImpactFailure::PathDepthDisagrees {
                hop_depth: 2,
                path_depth: 1,
            },
            ImpactFailure::RelationshipListDisagrees {
                declared: "DEPENDS_ON".to_owned(),
                traversed: "INVOCATES".to_owned(),
            },
            ImpactFailure::ChangeTypeMissing {
                finding: "impact-1".to_owned(),
            },
            ImpactFailure::ChangeClassificationDisagrees {
                hop_depth: 0,
                claims_change: true,
                has_change_type: false,
            },
            ImpactFailure::ConfidenceNotWeakestLink {
                reported: "VERIFIED".to_owned(),
                weakest: "LOW_CONFIDENCE".to_owned(),
            },
            ImpactFailure::EntityClassificationMissing {
                kind: "CONTRACT".to_owned(),
                required: "CONTRACT".to_owned(),
            },
            ImpactFailure::DeploymentNotEligible {
                deployment: "D-1".to_owned(),
                status: "FAILED".to_owned(),
            },
            ImpactFailure::DeploymentLedgerUnrecorded {
                deployment: "D-1".to_owned(),
            },
            ImpactFailure::DirectionDisagrees {
                declared: "DEPENDENTS".to_owned(),
                derived: "DEPENDENCIES".to_owned(),
            },
            ImpactFailure::ReasonNotStated { length: 3 },
            ImpactFailure::PathRepeatsEntity {
                entity: "C-a".to_owned(),
            },
            ImpactFailure::DuplicateFindingId {
                id: "impact-1".to_owned(),
            },
            ImpactFailure::PropagationTruncated {
                reason: "the depth bound was reached".to_owned(),
                reported: 12,
            },
        ]
    }

    #[test]
    fn every_failure_has_a_category_a_description_and_a_code() {
        let failures = every_failure();
        for failure in &failures {
            assert_eq!(failure.category(), ErrorCategory::Impact);
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
    fn each_failure_is_exactly_one_of_a_refusal_a_gap_or_a_bound() {
        for failure in every_failure() {
            let classifications = usize::from(failure.is_refusal())
                + usize::from(failure.is_completable_by_evidence())
                + usize::from(failure.is_bound_not_defect());
            assert_eq!(
                classifications, 1,
                "{failure:?} is classified {classifications} times, and must be exactly once"
            );
        }
        // The three sets are disjoint by construction, but the interesting half is that
        // the bound is not an error: a caller must not be able to mistake a deliberate
        // limit for a defect.
        assert!(
            ImpactFailure::PropagationTruncated {
                reason: "bound".to_owned(),
                reported: 1,
            }
            .is_bound_not_defect()
        );
        assert!(
            !ImpactFailure::PropagationTruncated {
                reason: "bound".to_owned(),
                reported: 1,
            }
            .is_refusal()
        );
    }

    #[test]
    fn a_non_propagating_step_explains_why_observation_is_not_dependency() {
        let error = ImpactFailure::NonPropagatingStep {
            relationship: "VERIFIED_BY".to_owned(),
            source: "WASM-ab".to_owned(),
            target: "build-1".to_owned(),
        }
        .into_error();
        let message = error.to_string();
        assert!(
            message.contains("change propagation is none"),
            "got: {message}"
        );
        assert!(
            message.contains("re-observation or a re-verification"),
            "got: {message}"
        );
    }

    #[test]
    fn manufacturing_confidence_is_named_as_such() {
        let error = ImpactFailure::ConfidenceNotWeakestLink {
            reported: "VERIFIED".to_owned(),
            weakest: "LOW_CONFIDENCE".to_owned(),
        }
        .into_error();
        let message = error.to_string();
        assert!(message.contains("weakest link"), "got: {message}");
        assert!(
            message.contains("manufactures confidence"),
            "got: {message}"
        );
    }

    #[test]
    fn a_failed_deployment_cannot_be_affected_because_it_never_took_effect() {
        let error = ImpactFailure::DeploymentNotEligible {
            deployment: "D-1".to_owned(),
            status: "FAILED".to_owned(),
        }
        .into_error();
        assert!(error.to_string().contains("taken effect"), "got: {error}");
    }

    #[test]
    fn an_empty_failure_list_succeeds_and_a_populated_one_reports_its_first() {
        first_failure(Vec::new()).expect("nothing to report");
        let error = first_failure(vec![ImpactFailure::ReasonNotStated { length: 2 }])
            .expect_err("a failure must be reported");
        assert!(error.to_string().contains("2 characters"));
    }
}
