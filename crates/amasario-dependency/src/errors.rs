//! The ways a dependency can fail to be established, and how each is classified.
//!
//! # Why the failure model is the load-bearing part
//!
//! The specification forbids inferring a dependency from anything except evidence:
//! not from two projects mentioning each other, not from two contracts existing in the
//! same ecosystem, not from similar metadata. That prohibition is only real if the
//! code can refuse, so every variant here is a refusal by name. Together they mean a
//! dependency cannot enter the graph wearing a basis it did not earn.
//!
//! Each variant quotes a rule rather than expressing a preference. The four that carry
//! the most weight:
//!
//! * [`DependencyFailure::NetworkUnrecorded`] - `dependency/contract-dependency`:
//!   "A dependency of class CONTRACT MUST name the network its target was observed
//!   on." A contract address is only meaningful with a network, so a dependency
//!   without one resolves to nothing a reader could check.
//! * [`DependencyFailure::TransactionNotRecordedAsSuccessful`] - the same rule:
//!   "When the basis is an observed invocation, the underlying transaction MUST be
//!   recorded as successful." An unknown outcome is not a successful one.
//! * [`DependencyFailure::BasisCannotEstablishArtifactDependency`] -
//!   `dependency/artifact-dependency`: a dependency on an artifact or executable MUST
//!   rest on a digest match or a declared build input. Size, media type, module name
//!   and version string are exactly the signals that coincide by chance, and a
//!   dependency recorded from them can never be refuted, because no digest was
//!   recorded to re-compute.
//! * [`DependencyFailure::BoundaryUnrecorded`] - the same rule: an `EXTERNAL`
//!   dependency MUST record that its target lies outside the observable boundary. An
//!   entity Amasario cannot inspect is not an entity that failed inspection.

use amasario_core::{EngineError, ErrorCategory, Result};

/// A way a dependency claim can fail to be established.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DependencyFailure {
    /// The claim cites no evidence at all.
    NoEvidenceCited {
        /// The subject of the claim.
        subject: String,
        /// The object of the claim.
        object: String,
    },
    /// The basis cannot establish the classes that were claimed.
    ///
    /// Interface similarity is the case this exists for: the specification permits it
    /// to support a `CONTRACT` dependency at low confidence and nothing else, because
    /// two contracts sharing an interface shape are common and completely unrelated.
    BasisCannotEstablishDependency {
        /// The subject of the claim.
        subject: String,
        /// The object of the claim.
        object: String,
        /// The basis the claim rests on.
        basis: String,
        /// The classes that were claimed.
        classes: String,
        /// What the basis can establish instead.
        detail: String,
    },
    /// The cited evidence does not include a kind the class requires.
    EvidenceKindNotPermitted {
        /// The subject of the claim.
        subject: String,
        /// The object of the claim.
        object: String,
        /// The class that was claimed.
        class: String,
        /// The kinds that were cited.
        cited: String,
        /// The kinds the class accepts.
        permitted: String,
    },
    /// The relationship cannot express a dependency at all.
    RelationshipNotADependency {
        /// The relationship that was used.
        relationship: String,
        /// The kind of the object.
        object_kind: String,
        /// Why it does not express a dependency.
        detail: String,
    },
    /// The subject and the object are the same entity.
    SelfDependency {
        /// The entity that was both subject and object.
        entity: String,
    },
    /// The relationship does not permit the pair of endpoint kinds.
    EndpointsNotPermitted {
        /// The relationship that was used.
        relationship: String,
        /// The kind of the subject.
        subject_kind: String,
        /// The kind of the object.
        object_kind: String,
    },
    /// The class requires an observation and the basis is not one.
    ObservationRequired {
        /// The subject of the claim.
        subject: String,
        /// The object of the claim.
        object: String,
        /// The class that requires the observation.
        class: String,
        /// The basis the claim rested on.
        basis: String,
    },
    /// A class that needs a network was claimed without one.
    NetworkUnrecorded {
        /// The subject of the claim.
        subject: String,
        /// The object of the claim.
        object: String,
        /// The class that needs the network.
        class: String,
    },
    /// A class that requires the observation boundary to be recorded.
    BoundaryUnrecorded {
        /// The subject of the claim.
        subject: String,
        /// The object of the claim.
        object: String,
        /// The class that needs the boundary.
        class: String,
    },
    /// A runtime claim rests on a transaction whose outcome was not recorded as
    /// successful.
    TransactionNotRecordedAsSuccessful {
        /// The subject of the claim.
        subject: String,
        /// The object of the claim.
        object: String,
        /// What the transaction's outcome was recorded as.
        outcome: String,
    },
    /// An artifact or executable dependency rests on something other than a digest.
    BasisCannotEstablishArtifactDependency {
        /// The subject of the claim.
        subject: String,
        /// The object of the claim.
        object: String,
        /// The basis the claim rests on.
        basis: String,
    },
    /// The class cannot be verified, so claiming it was is unsupported.
    UnverifiableClass {
        /// The subject of the claim.
        subject: String,
        /// The object of the claim.
        object: String,
        /// The class that cannot be verified.
        class: String,
    },
    /// A contradiction was claimed without any contradicting evidence.
    ContradictionUnsupported {
        /// The subject of the claim.
        subject: String,
        /// The object of the claim.
        object: String,
    },
}

impl DependencyFailure {
    /// The category this failure belongs to.
    #[must_use]
    pub const fn category(&self) -> ErrorCategory {
        // Every variant is a dependency failure, including the ones that read like
        // input validation: the input was well formed, and what failed was
        // establishing that the subject depends on the object. Classifying them as
        // validation errors would suggest the caller could have known in advance,
        // and would separate a refusal from the analysis it belongs to.
        ErrorCategory::Dependency
    }

    /// Whether the failure means the evidence is absent rather than insufficient.
    ///
    /// The question a caller asks when deciding whether to search further. Absent
    /// evidence can be collected; a basis that cannot establish a dependency cannot
    /// be improved by collecting more of it.
    #[must_use]
    pub const fn is_evidence_absent(&self) -> bool {
        matches!(self, Self::NoEvidenceCited { .. })
    }

    /// Whether the failure is a refusal to overstate a claim that may still be
    /// recorded as something weaker.
    ///
    /// True where the underlying observation remains valid. Reporting these as though
    /// the observation were wrong would discard a real finding, which is why the two
    /// questions are separate methods rather than one predicate.
    #[must_use]
    pub const fn leaves_observation_intact(&self) -> bool {
        matches!(
            self,
            Self::BasisCannotEstablishDependency { .. }
                | Self::RelationshipNotADependency { .. }
                | Self::BasisCannotEstablishArtifactDependency { .. }
        )
    }

    /// Whether the failure is a claim that could be completed by observing more.
    ///
    /// True where a field the class requires was not recorded, which is what an
    /// observation boundary, a network or a transaction outcome being absent means.
    #[must_use]
    pub const fn is_completable_by_observation(&self) -> bool {
        matches!(
            self,
            Self::NetworkUnrecorded { .. }
                | Self::BoundaryUnrecorded { .. }
                | Self::TransactionNotRecordedAsSuccessful { .. }
                | Self::ObservationRequired { .. }
        )
    }

    /// Converts the failure into the engine's structured error.
    #[must_use]
    pub fn into_error(self) -> EngineError {
        match self {
            Self::NoEvidenceCited { subject, object } => EngineError::Dependency(format!(
                "the claim that {subject} depends on {object} cites no evidence; a dependency \
                 without evidence is an assertion, and the specification requires a basis for one"
            )),
            Self::BasisCannotEstablishDependency {
                subject,
                object,
                basis,
                classes,
                detail,
            } => EngineError::Dependency(format!(
                "the {basis} basis cannot establish {classes} for {subject} on {object}: {detail}"
            )),
            Self::EvidenceKindNotPermitted {
                subject,
                object,
                class,
                cited,
                permitted,
            } => EngineError::Dependency(format!(
                "the {class} dependency of {subject} on {object} cites {cited}, but that class \
                 requires one of {permitted}; relabelling a claim does not strengthen its evidence"
            )),
            Self::RelationshipNotADependency {
                relationship,
                object_kind,
                detail,
            } => EngineError::Dependency(format!(
                "{relationship} with a {object_kind} object does not express a dependency: {detail}"
            )),
            Self::SelfDependency { entity } => EngineError::Dependency(format!(
                "{entity} cannot depend on itself; a self-loop is a defect in the graph rather \
                 than a relationship"
            )),
            Self::EndpointsNotPermitted {
                relationship,
                subject_kind,
                object_kind,
            } => EngineError::Dependency(format!(
                "{relationship} cannot connect a {subject_kind} to a {object_kind}; the \
                 relationship's semantics do not permit those endpoint kinds"
            )),
            Self::ObservationRequired {
                subject,
                object,
                class,
                basis,
            } => EngineError::Dependency(format!(
                "a {class} dependency of {subject} on {object} cannot rest on the {basis} basis; \
                 what a contract actually does while executing is only observable in a \
                 transaction or an event, and a declaration cannot substitute for one"
            )),
            Self::NetworkUnrecorded {
                subject,
                object,
                class,
            } => EngineError::Dependency(format!(
                "the {class} dependency of {subject} on {object} does not name the network it was \
                 observed on; a contract address identifies a contract only together with its \
                 network, so the dependency resolves to nothing a reader could check"
            )),
            Self::BoundaryUnrecorded {
                subject,
                object,
                class,
            } => EngineError::Dependency(format!(
                "the {class} dependency of {subject} on {object} does not record that its target \
                 lies outside the observable boundary; an entity Amasario cannot inspect is not \
                 an entity that failed inspection"
            )),
            Self::TransactionNotRecordedAsSuccessful {
                subject,
                object,
                outcome,
            } => EngineError::Dependency(format!(
                "a RUNTIME dependency of {subject} on {object} rests on a transaction recorded as \
                 {outcome}; runtime use must be established from a transaction recorded as \
                 successful, and an unknown outcome is not a successful one"
            )),
            Self::BasisCannotEstablishArtifactDependency {
                subject,
                object,
                basis,
            } => EngineError::Dependency(format!(
                "the {basis} basis cannot establish that {subject} depends on {object}: a \
                 dependency on an artifact or executable must rest on a digest match or on a \
                 build input declared by the subject, because metadata similarity is what \
                 coincides by chance and leaves nothing to re-compute"
            )),
            Self::UnverifiableClass {
                subject,
                object,
                class,
            } => EngineError::Dependency(format!(
                "the {class} dependency of {subject} on {object} cannot be reported as verified; \
                 it lies outside the observable boundary, so there is nothing to check it against"
            )),
            Self::ContradictionUnsupported { subject, object } => EngineError::Dependency(format!(
                "the dependency of {subject} on {object} was reported as contradicted without any \
                 contradicting evidence; a refutation needs something that refutes"
            )),
        }
    }
}

/// A one-line description of what went wrong, without the identifiers.
#[must_use]
pub const fn describe(failure: &DependencyFailure) -> &'static str {
    match failure {
        DependencyFailure::NoEvidenceCited { .. } => "the claim cites no evidence",
        DependencyFailure::BasisCannotEstablishDependency { .. } => {
            "the basis cannot establish the classes claimed"
        },
        DependencyFailure::EvidenceKindNotPermitted { .. } => {
            "the cited evidence is not a kind the class requires"
        },
        DependencyFailure::RelationshipNotADependency { .. } => {
            "the relationship does not express a dependency"
        },
        DependencyFailure::SelfDependency { .. } => "an entity cannot depend on itself",
        DependencyFailure::EndpointsNotPermitted { .. } => {
            "the relationship does not permit those endpoints"
        },
        DependencyFailure::ObservationRequired { .. } => "the class requires an observation",
        DependencyFailure::NetworkUnrecorded { .. } => "the network was not recorded",
        DependencyFailure::BoundaryUnrecorded { .. } => "the observation boundary was not recorded",
        DependencyFailure::TransactionNotRecordedAsSuccessful { .. } => {
            "the transaction was not recorded as successful"
        },
        DependencyFailure::BasisCannotEstablishArtifactDependency { .. } => {
            "the basis is not a digest match or a declared build input"
        },
        DependencyFailure::UnverifiableClass { .. } => "the class cannot be verified",
        DependencyFailure::ContradictionUnsupported { .. } => {
            "a contradiction was claimed without contradicting evidence"
        },
    }
}

/// Turns the first of several failures into a `Result`, or succeeds when the list is
/// empty.
///
/// # Errors
///
/// Returns the first failure's error when `failures` is non-empty.
pub fn first_failure(failures: Vec<DependencyFailure>) -> Result<()> {
    match failures.into_iter().next() {
        Some(failure) => Err(failure.into_error()),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn every_failure() -> Vec<DependencyFailure> {
        vec![
            DependencyFailure::NoEvidenceCited {
                subject: "C-a".to_owned(),
                object: "C-b".to_owned(),
            },
            DependencyFailure::BasisCannotEstablishDependency {
                subject: "C-a".to_owned(),
                object: "pkg".to_owned(),
                basis: "INFERRED_INTERFACE".to_owned(),
                classes: "PACKAGE".to_owned(),
                detail: "interface similarity is consistent with a requirement without being \
                         evidence of one"
                    .to_owned(),
            },
            DependencyFailure::EvidenceKindNotPermitted {
                subject: "C-a".to_owned(),
                object: "pkg".to_owned(),
                class: "PACKAGE".to_owned(),
                cited: "EVENT".to_owned(),
                permitted: "SOURCE, BUILD, ARTIFACT".to_owned(),
            },
            DependencyFailure::RelationshipNotADependency {
                relationship: "VERIFIED_BY".to_owned(),
                object_kind: "WASM".to_owned(),
                detail: "verification supports a claim; it does not create a requirement"
                    .to_owned(),
            },
            DependencyFailure::SelfDependency {
                entity: "C-a".to_owned(),
            },
            DependencyFailure::EndpointsNotPermitted {
                relationship: "INVOCATES".to_owned(),
                subject_kind: "WASM".to_owned(),
                object_kind: "CONTRACT".to_owned(),
            },
            DependencyFailure::ObservationRequired {
                subject: "C-a".to_owned(),
                object: "C-b".to_owned(),
                class: "RUNTIME".to_owned(),
                basis: "DECLARED_MANIFEST".to_owned(),
            },
            DependencyFailure::NetworkUnrecorded {
                subject: "C-a".to_owned(),
                object: "C-b".to_owned(),
                class: "CONTRACT".to_owned(),
            },
            DependencyFailure::BoundaryUnrecorded {
                subject: "C-a".to_owned(),
                object: "https://example.invalid/r".to_owned(),
                class: "EXTERNAL".to_owned(),
            },
            DependencyFailure::TransactionNotRecordedAsSuccessful {
                subject: "C-a".to_owned(),
                object: "C-b".to_owned(),
                outcome: "unknown".to_owned(),
            },
            DependencyFailure::BasisCannotEstablishArtifactDependency {
                subject: "C-a".to_owned(),
                object: "wasm-digest".to_owned(),
                basis: "CONFIGURED_ENDPOINT".to_owned(),
            },
            DependencyFailure::UnverifiableClass {
                subject: "C-a".to_owned(),
                object: "https://example.invalid/r".to_owned(),
                class: "EXTERNAL".to_owned(),
            },
            DependencyFailure::ContradictionUnsupported {
                subject: "C-a".to_owned(),
                object: "C-b".to_owned(),
            },
        ]
    }

    #[test]
    fn every_failure_has_a_category_a_description_and_a_code() {
        let failures = every_failure();
        for failure in &failures {
            assert_eq!(failure.category(), ErrorCategory::Dependency);
            assert_eq!(
                failure.category(),
                failure.clone().into_error().category(),
                "{failure:?} disagrees with its own error"
            );
            assert!(!describe(failure).is_empty());
            assert!(!failure.clone().into_error().code().is_empty());
        }
        assert_eq!(failures.len(), 13, "every variant is covered by this test");
    }

    #[test]
    fn the_classification_of_each_failure_is_consistent() {
        for failure in every_failure() {
            assert_eq!(
                failure.leaves_observation_intact(),
                matches!(
                    failure,
                    DependencyFailure::BasisCannotEstablishDependency { .. }
                        | DependencyFailure::RelationshipNotADependency { .. }
                        | DependencyFailure::BasisCannotEstablishArtifactDependency { .. }
                ),
                "{failure:?} misclassified as a refusal"
            );
            assert_eq!(
                failure.is_evidence_absent(),
                matches!(failure, DependencyFailure::NoEvidenceCited { .. }),
                "{failure:?} misclassified as absent evidence"
            );
            assert_eq!(
                failure.is_completable_by_observation(),
                matches!(
                    failure,
                    DependencyFailure::NetworkUnrecorded { .. }
                        | DependencyFailure::BoundaryUnrecorded { .. }
                        | DependencyFailure::TransactionNotRecordedAsSuccessful { .. }
                        | DependencyFailure::ObservationRequired { .. }
                ),
                "{failure:?} misclassified as completable"
            );
            assert!(
                !(failure.leaves_observation_intact() && failure.is_completable_by_observation()),
                "{failure:?} cannot be both a refusal and something more evidence would fix"
            );
        }
    }

    #[test]
    fn a_contract_dependency_without_a_network_explains_why_it_is_unresolvable() {
        let error = DependencyFailure::NetworkUnrecorded {
            subject: "C-a".to_owned(),
            object: "C-b".to_owned(),
            class: "CONTRACT".to_owned(),
        }
        .into_error();
        assert!(
            error.to_string().contains("only together with its network"),
            "got: {error}"
        );
    }

    #[test]
    fn an_unknown_transaction_outcome_is_not_treated_as_success() {
        let error = DependencyFailure::TransactionNotRecordedAsSuccessful {
            subject: "C-a".to_owned(),
            object: "C-b".to_owned(),
            outcome: "unknown".to_owned(),
        }
        .into_error();
        assert!(
            error
                .to_string()
                .contains("unknown outcome is not a successful one"),
            "got: {error}"
        );
    }

    #[test]
    fn an_artifact_dependency_refusal_names_what_would_have_sufficed() {
        let error = DependencyFailure::BasisCannotEstablishArtifactDependency {
            subject: "C-a".to_owned(),
            object: "wasm-digest".to_owned(),
            basis: "CONFIGURED_ENDPOINT".to_owned(),
        }
        .into_error();
        let message = error.to_string();
        assert!(message.contains("digest match"), "got: {message}");
        assert!(message.contains("nothing to re-compute"), "got: {message}");
    }

    #[test]
    fn an_empty_failure_list_succeeds_and_a_populated_one_reports_its_first() {
        first_failure(Vec::new()).expect("nothing to report");
        let error = first_failure(vec![DependencyFailure::SelfDependency {
            entity: "C-a".to_owned(),
        }])
        .expect_err("a failure must be reported");
        assert!(error.to_string().contains("cannot depend on itself"));
    }
}
