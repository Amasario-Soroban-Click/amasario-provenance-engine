//! What an observed relationship is allowed to be called.
//!
//! # The rule this module enforces
//!
//! A dependency class is a claim about *why* the subject requires the object, and the
//! specification gives every class required evidence. Classification is therefore not
//! a labelling exercise: it is the point at which a claim either has the evidence its
//! name implies or is refused. Relabelling a claim is the cheapest way to upgrade one,
//! which is why the class's requirement is checked here rather than trusted.
//!
//! Four consequences are worth stating plainly.
//!
//! **A class is a set, not a value.** An observed cross-contract call is both a
//! [`DependencyClass::Contract`] dependency - the object is a contract - and a
//! [`DependencyClass::Runtime`] one, because it exists only while the subject
//! executes. Forcing a single label would discard one of two true statements.
//!
//! **`DEPENDS_ON` and `INVOCATES` are the only dependency relationships.** The other
//! six describe derivation, observation and verification. `BUILT_FROM` is how a build
//! came to exist, not a requirement, and recording it as a dependency would make every
//! artifact a consumer of its own inputs.
//!
//! **Interface inference is permitted exactly once.** `dependency/contract-dependency`
//! allows it for a `CONTRACT` dependency at no more than `LOW_CONFIDENCE`, and the
//! reason is that two contracts sharing an interface shape are common and frequently
//! unrelated. It cannot support a `RUNTIME` claim, because what a contract does while
//! executing is only observable in a transaction or an event.
//!
//! **Confidence is an upper bound, not a measurement.** The basis sets a ceiling
//! ([`amasario_core::Basis::confidence_ceiling`]) and this module reports that ceiling
//! rather than inventing a number. A caller holding stronger evidence has to say so by
//! citing it, which is the only route the specification allows.

use std::fmt;

use amasario_core::{
    Basis, Confidence, ConfidenceLevel, DependencyClass, EntityKind, EntityRef, EvidenceType,
    Network, ObservationBoundary, Relationship, Result, VerificationStatus,
};
use serde::{Deserialize, Serialize};

use crate::errors::DependencyFailure;

/// A citation into the evidence records that support a claim.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceRef {
    /// The evidence record's identifier.
    pub id: String,
    /// What kind of evidence the record is.
    pub kind: EvidenceType,
}

impl EvidenceRef {
    /// Cites an evidence record.
    ///
    /// # Errors
    ///
    /// Returns a dependency error when the identifier is empty, because a citation
    /// that names nothing cannot be followed back to what it supports.
    pub fn new(kind: EvidenceType, id: impl Into<String>) -> Result<Self> {
        let id = id.into();
        if id.is_empty() {
            return Err(DependencyFailure::NoEvidenceCited {
                subject: "an evidence citation".to_owned(),
                object: kind.as_str().to_owned(),
            }
            .into_error());
        }
        Ok(Self { id, kind })
    }
}

impl fmt::Display for EvidenceRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.kind.as_str(), self.id)
    }
}

/// The bases that can establish a dependency on an artifact or executable.
///
/// `dependency/artifact-dependency` requires a digest match or a build input declared
/// by the subject. `EMBEDDED_DIGEST` is the digest match; `RESOLVED_LOCKFILE` and
/// `DECLARED_MANIFEST` are the declared build input. Anything else - a configured
/// endpoint, an attestation, interface similarity - is metadata, and metadata is what
/// coincides by chance.
const ARTIFACT_BASES: &[Basis] = &[
    Basis::EmbeddedDigest,
    Basis::ResolvedLockfile,
    Basis::DeclaredManifest,
];

/// A relationship that was observed and has not yet been classified.
///
/// The name is deliberate. Nothing at this stage is a dependency: it is an
/// observation with a basis and citations, and [`classify`] decides what it is
/// allowed to become.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Candidate {
    /// The entity the relationship starts at.
    pub subject: EntityRef,
    /// The entity the relationship points at.
    pub object: EntityRef,
    /// The relationship between them.
    pub relationship: Relationship,
    /// What the relationship rests on.
    pub basis: Basis,
    /// The evidence that supports it. Never empty.
    pub evidence: Vec<EvidenceRef>,
    /// Evidence that contradicts it, where any was found.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub contradicting: Vec<EvidenceRef>,
    /// The boundary the observation was made at, where one was recorded.
    ///
    /// Carries the network, which a `CONTRACT` or `RUNTIME` dependency is required to
    /// name, and which is what makes such a dependency resolvable by a reader.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub boundary: Option<ObservationBoundary>,
    /// Whether the transaction the observation rests on was recorded as successful.
    ///
    /// Three-valued rather than a boolean for the same reason as the rest of the
    /// engine: an unrecorded outcome is not a successful one, and a `RUNTIME` claim
    /// may not rest on it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub successful: Option<bool>,
    /// What was observed, in a reader's words.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl Candidate {
    /// Records an observed relationship.
    ///
    /// # Errors
    ///
    /// Returns a dependency error when no evidence is cited, when the subject and the
    /// object are the same entity, or when the relationship's semantics do not permit
    /// the pair of endpoint kinds. All three are refused here rather than at
    /// classification time so that an impossible candidate cannot be constructed and
    /// then reported as merely unclassifiable.
    pub fn new(
        subject: EntityRef,
        object: EntityRef,
        relationship: Relationship,
        basis: Basis,
        evidence: Vec<EvidenceRef>,
    ) -> Result<Self> {
        if evidence.is_empty() {
            return Err(DependencyFailure::NoEvidenceCited {
                subject: subject.to_string(),
                object: object.to_string(),
            }
            .into_error());
        }
        if subject == object {
            return Err(DependencyFailure::SelfDependency {
                entity: subject.to_string(),
            }
            .into_error());
        }
        if !relationship.permits(subject.kind, object.kind) {
            return Err(DependencyFailure::EndpointsNotPermitted {
                relationship: relationship.as_str().to_owned(),
                subject_kind: subject.kind.as_str().to_owned(),
                object_kind: object.kind.as_str().to_owned(),
            }
            .into_error());
        }
        Ok(Self {
            subject,
            object,
            relationship,
            basis,
            evidence,
            contradicting: Vec::new(),
            boundary: None,
            successful: None,
            detail: None,
        })
    }

    /// Records evidence that contradicts the relationship.
    ///
    /// # Errors
    ///
    /// Returns a dependency error when the list is empty. A contradiction needs
    /// something that contradicts, and an empty one would otherwise be
    /// indistinguishable from no contradiction at all.
    pub fn contradicted_by(mut self, contradicting: Vec<EvidenceRef>) -> Result<Self> {
        if contradicting.is_empty() {
            return Err(DependencyFailure::ContradictionUnsupported {
                subject: self.subject.to_string(),
                object: self.object.to_string(),
            }
            .into_error());
        }
        self.contradicting = contradicting;
        Ok(self)
    }

    /// Records the boundary the observation was made at.
    #[must_use]
    pub fn observed_at(mut self, boundary: ObservationBoundary) -> Self {
        self.boundary = Some(boundary);
        self
    }

    /// Records whether the transaction was successful.
    #[must_use]
    pub const fn with_outcome(mut self, successful: Option<bool>) -> Self {
        self.successful = successful;
        self
    }

    /// Records what was observed, in a reader's words.
    #[must_use]
    pub fn explained_by(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    /// Whether any cited evidence contradicts the relationship.
    #[must_use]
    pub const fn is_contradicted(&self) -> bool {
        !self.contradicting.is_empty()
    }

    /// The network the observation was made on, where one was recorded.
    #[must_use]
    pub fn network(&self) -> Option<&Network> {
        self.boundary.as_ref().map(|boundary| &boundary.network)
    }

    /// The evidence kinds cited, deduplicated in canonical order.
    #[must_use]
    pub fn cited_kinds(&self) -> Vec<EvidenceType> {
        EvidenceType::all()
            .iter()
            .copied()
            .filter(|kind| self.evidence.iter().any(|cited| cited.kind == *kind))
            .collect()
    }
}

/// What a candidate was allowed to become.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Classification {
    /// The classes the evidence supports, in canonical order. Never empty.
    pub classes: Vec<DependencyClass>,
    /// The confidence the basis supports, with the citations behind it.
    pub confidence: Confidence,
    /// What the evidence says about the claim.
    pub verification: VerificationStatus,
    /// Why these classes, in terms of the relationship and the citations.
    pub rationale: String,
}

impl Classification {
    /// Whether the classification includes a class.
    #[must_use]
    pub fn has(&self, class: DependencyClass) -> bool {
        self.classes.contains(&class)
    }

    /// Whether the claim may be reported as an observed fact rather than an
    /// inference.
    ///
    /// The gate a report uses to decide which section a dependency belongs in, and
    /// stated here so that two reports cannot answer it differently. An inferred
    /// interface never qualifies, which is what keeps it out of the established-facts
    /// section the specification requires it to stay out of.
    #[must_use]
    pub const fn is_observed(&self) -> bool {
        matches!(self.verification, VerificationStatus::Verified) && self.confidence.has_support()
    }
}

/// The classes a relationship can express for a given object kind.
///
/// `None` means the relationship does not express a dependency at all, which is every
/// relationship except `DEPENDS_ON` and `INVOCATES`.
///
/// An artifact or executable object maps to [`DependencyClass::Wasm`] rather than to
/// no class: `dependency/artifact-dependency` speaks of "a dependency whose target is
/// an artifact or executable", and the taxonomy's only class for such a target is the
/// one named for the module. A *build* target has no class, because a build is an
/// input the subject was derived from rather than something it requires.
#[must_use]
pub const fn classes_for(
    relationship: Relationship,
    object_kind: EntityKind,
) -> Option<&'static [DependencyClass]> {
    match relationship {
        Relationship::Invocates => match object_kind {
            EntityKind::Contract => Some(&[DependencyClass::Contract, DependencyClass::Runtime]),
            _ => None,
        },
        Relationship::DependsOn => match object_kind {
            EntityKind::Contract => Some(&[DependencyClass::Contract]),
            EntityKind::Package => Some(&[DependencyClass::Package]),
            EntityKind::Wasm | EntityKind::Artifact => Some(&[DependencyClass::Wasm]),
            EntityKind::Source => Some(&[DependencyClass::External]),
            _ => None,
        },
        // DERIVED_FROM, BUILT_FROM, DEPLOYED_AS, OBSERVED_IN, VERIFIED_BY and AFFECTS
        // all reach this arm. None of them is a requirement.
        _ => None,
    }
}

/// Explains why a relationship does not express a dependency.
///
/// Takes the relationship alone because the object kind adds nothing to the answer for
/// any relationship that reaches here: the kinds that a *dependency* relationship
/// cannot point at are refused when the candidate is constructed, so a per-kind message
/// would be unreachable. The refusal itself carries the object kind, which is where a
/// reader needs it.
#[must_use]
pub const fn why_not_a_dependency(relationship: Relationship) -> &'static str {
    match relationship {
        Relationship::DerivedFrom | Relationship::BuiltFrom => {
            "derivation records how the subject came to exist; an input is not a requirement"
        },
        Relationship::DeployedAs => {
            "a deployment record describes where the subject went, not what it requires"
        },
        Relationship::ObservedIn => {
            "being observed in a transaction is not a requirement on the transaction"
        },
        Relationship::VerifiedBy => {
            "verification supports a claim; it does not create a dependency"
        },
        Relationship::Affects => {
            "an impact assertion states what a change reaches, not what the subject requires"
        },
        Relationship::Invocates | Relationship::DependsOn => {
            "the object kind is not one a dependency class describes"
        },
        // `Relationship` is non-exhaustive, so a relationship added by a later
        // specification version reaches this arm. The engine cannot say what it means,
        // and inventing a meaning is the one thing it must not do, so the refusal
        // reports the term rather than guessing at a class for it.
        _ => {
            "the relationship is not defined at this specification version, so the engine cannot \
             say what it expresses"
        },
    }
}

/// Classifies an observed relationship.
///
/// # Errors
///
/// Returns a dependency error when the basis cannot establish the classes claimed,
/// when the relationship does not express a dependency, when the cited evidence does
/// not include a kind the class requires, when a class that needs a network or a
/// boundary was recorded without one, or when a runtime claim rests on a transaction
/// whose outcome was not recorded as successful.
pub fn classify(candidate: &Candidate) -> Result<Classification> {
    let Some(classes) = classes_for(candidate.relationship, candidate.object.kind) else {
        return Err(DependencyFailure::RelationshipNotADependency {
            relationship: candidate.relationship.as_str().to_owned(),
            object_kind: candidate.object.kind.as_str().to_owned(),
            detail: why_not_a_dependency(candidate.relationship).to_owned(),
        }
        .into_error());
    };

    // Interface similarity is permitted for a CONTRACT dependency and nothing else.
    // The check is on the whole class set, not on membership: an INVOCATES candidate
    // whose classes include RUNTIME is refused even though CONTRACT is present.
    if !candidate.basis.is_structural()
        && !classes
            .iter()
            .all(|class| *class == DependencyClass::Contract)
    {
        return Err(DependencyFailure::BasisCannotEstablishDependency {
            subject: candidate.subject.to_string(),
            object: candidate.object.to_string(),
            basis: candidate.basis.as_str().to_owned(),
            classes: render_classes(classes),
            detail: "interface similarity is consistent with a requirement without being \
                     evidence of one, so the specification permits it only for a CONTRACT \
                     dependency at no more than low confidence"
                .to_owned(),
        }
        .into_error());
    }

    let cited = candidate.cited_kinds();
    for class in classes {
        if !cited
            .iter()
            .any(|kind| class.required_evidence().contains(kind))
        {
            return Err(DependencyFailure::EvidenceKindNotPermitted {
                subject: candidate.subject.to_string(),
                object: candidate.object.to_string(),
                class: class.as_str().to_owned(),
                cited: render_kinds(&cited),
                permitted: render_kinds(class.required_evidence()),
            }
            .into_error());
        }

        if class.requires_observation() && !candidate.basis.is_observed() {
            return Err(DependencyFailure::ObservationRequired {
                subject: candidate.subject.to_string(),
                object: candidate.object.to_string(),
                class: class.as_str().to_owned(),
                basis: candidate.basis.as_str().to_owned(),
            }
            .into_error());
        }

        if class.requires_network() && candidate.network().is_none() {
            return Err(DependencyFailure::NetworkUnrecorded {
                subject: candidate.subject.to_string(),
                object: candidate.object.to_string(),
                class: class.as_str().to_owned(),
            }
            .into_error());
        }

        if *class == DependencyClass::External && candidate.boundary.is_none() {
            return Err(DependencyFailure::BoundaryUnrecorded {
                subject: candidate.subject.to_string(),
                object: candidate.object.to_string(),
                class: class.as_str().to_owned(),
            }
            .into_error());
        }

        if *class == DependencyClass::Wasm && !ARTIFACT_BASES.contains(&candidate.basis) {
            return Err(DependencyFailure::BasisCannotEstablishArtifactDependency {
                subject: candidate.subject.to_string(),
                object: candidate.object.to_string(),
                basis: candidate.basis.as_str().to_owned(),
            }
            .into_error());
        }
    }

    // Runtime use must rest on a transaction recorded as successful. An unknown
    // outcome is not a successful one, and the three-valued field is what keeps that
    // distinction available here.
    if classes.contains(&DependencyClass::Runtime) && candidate.successful != Some(true) {
        return Err(DependencyFailure::TransactionNotRecordedAsSuccessful {
            subject: candidate.subject.to_string(),
            object: candidate.object.to_string(),
            outcome: match candidate.successful {
                Some(false) => "failed".to_owned(),
                Some(true) => "successful".to_owned(),
                None => "unknown".to_owned(),
            },
        }
        .into_error());
    }

    // The ceiling comes from the basis, and a contradiction lowers it further: a claim
    // with counter-evidence cannot also be the best-supported claim available.
    let level = if candidate.is_contradicted() {
        ConfidenceLevel::weakest_of([
            candidate.basis.confidence_ceiling(),
            ConfidenceLevel::LowConfidence,
        ])
    } else {
        candidate.basis.confidence_ceiling()
    };

    let evidence: Vec<String> = candidate
        .evidence
        .iter()
        .map(EvidenceRef::to_string)
        .collect();
    let contradicting: Vec<String> = candidate
        .contradicting
        .iter()
        .map(EvidenceRef::to_string)
        .collect();
    let confidence = Confidence::new(level, evidence, contradicting)?.with_rationale(format!(
        "the {} basis sets a ceiling of {}",
        candidate.basis,
        candidate.basis.confidence_ceiling()
    ));

    // An observation of an invocation *is* the check: the transaction was read and it
    // records the call, and the outcome was recorded as successful. Anything weaker
    // leaves the claim unchecked, and reporting that as verified would be the
    // overstatement this crate exists to prevent. An EXTERNAL target is never
    // verified, because there is nothing to check it against.
    let verification = if candidate.is_contradicted() {
        VerificationStatus::Conflicting
    } else if classes.contains(&DependencyClass::External) {
        VerificationStatus::Unverified
    } else if candidate.basis.is_observed() {
        VerificationStatus::Verified
    } else {
        VerificationStatus::Unverified
    };

    Ok(Classification {
        classes: classes.to_vec(),
        confidence,
        verification,
        rationale: rationale_for(candidate, classes),
    })
}

fn rationale_for(candidate: &Candidate, classes: &[DependencyClass]) -> String {
    let classes = render_classes(classes);
    let network = match candidate.network() {
        Some(network) => format!(", observed on {}", network.id),
        None => String::new(),
    };
    match &candidate.detail {
        Some(detail) => format!(
            "{classes}: {detail}, established by the {} basis{network}",
            candidate.basis
        ),
        None => format!(
            "{classes}: {} {} {}, established by the {} basis{network}",
            candidate.subject, candidate.relationship, candidate.object, candidate.basis
        ),
    }
}

fn render_classes(classes: &[DependencyClass]) -> String {
    classes
        .iter()
        .map(|class| class.as_str())
        .collect::<Vec<_>>()
        .join(" and ")
}

fn render_kinds(kinds: &[EvidenceType]) -> String {
    kinds
        .iter()
        .map(|kind| kind.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use amasario_core::{EntityKind, LedgerSequence, NetworkType};

    fn entity(kind: EntityKind, id: &str) -> EntityRef {
        EntityRef::new(kind, id).expect("a reference")
    }

    fn citation(kind: EvidenceType, id: &str) -> EvidenceRef {
        EvidenceRef::new(kind, id).expect("a citation")
    }

    fn boundary() -> ObservationBoundary {
        ObservationBoundary {
            network: Network::new(
                "testnet",
                NetworkType::Testnet,
                "Test SDF Network ; September 2015",
            )
            .expect("a network"),
            ledger: LedgerSequence::new(1_000).expect("a ledger"),
            observed_at: "2026-09-15T00:00:00Z".to_owned(),
            spec_version: None,
        }
    }

    fn invocation() -> Candidate {
        Candidate::new(
            entity(EntityKind::Contract, "C-caller"),
            entity(EntityKind::Contract, "C-callee"),
            Relationship::Invocates,
            Basis::ObservedInvocation,
            vec![citation(EvidenceType::Transaction, "tx-1")],
        )
        .expect("an observed cross-contract call")
        .observed_at(boundary())
        .with_outcome(Some(true))
    }

    #[test]
    fn an_observed_cross_contract_call_is_both_a_contract_and_a_runtime_dependency() {
        // A single label would discard one of two true statements: the object is a
        // contract, and the requirement exists only while the subject executes.
        let classification = classify(&invocation()).expect("classifiable");
        assert!(classification.has(DependencyClass::Contract));
        assert!(classification.has(DependencyClass::Runtime));
        assert_eq!(classification.classes.len(), 2);
        assert_eq!(classification.verification, VerificationStatus::Verified);
        assert!(classification.is_observed());
        assert!(classification.rationale.contains("observed on testnet"));
    }

    #[test]
    fn classes_are_reported_in_canonical_order() {
        let classification = classify(&invocation()).expect("classifiable");
        let canonical: Vec<DependencyClass> = DependencyClass::all()
            .iter()
            .copied()
            .filter(|class| classification.has(*class))
            .collect();
        assert_eq!(classification.classes, canonical);
    }

    #[test]
    fn a_contract_dependency_must_name_its_network() {
        // A contract address identifies a contract only together with its network.
        let candidate = Candidate::new(
            entity(EntityKind::Contract, "C-a"),
            entity(EntityKind::Contract, "C-b"),
            Relationship::DependsOn,
            Basis::DeclaredManifest,
            vec![citation(EvidenceType::Source, "manifest-1")],
        )
        .expect("a constructible candidate");
        let error = classify(&candidate).expect_err("no network was recorded");
        assert!(
            error.to_string().contains("together with its network"),
            "got: {error}"
        );
    }

    #[test]
    fn interface_inference_supports_a_contract_dependency_and_nothing_more() {
        // The specification's one exception, bounded by the class it applies to.
        // The citation is the artifact whose interface was compared, because a CONTRACT
        // class requires one of transaction, event, source or artifact evidence - an
        // `Observation` citation is not a kind the class accepts.
        let contract_only = Candidate::new(
            entity(EntityKind::Contract, "C-a"),
            entity(EntityKind::Contract, "C-b"),
            Relationship::DependsOn,
            Basis::InferredInterface,
            vec![citation(EvidenceType::Artifact, "artifact-1")],
        )
        .expect("a constructible candidate")
        .observed_at(boundary());
        let classification = classify(&contract_only).expect("permitted for CONTRACT");
        assert_eq!(classification.classes, vec![DependencyClass::Contract]);
        assert_eq!(
            classification.confidence.level,
            ConfidenceLevel::LowConfidence
        );
        assert!(
            !classification.is_observed(),
            "an inferred relationship must stay out of the observed facts"
        );

        // The same basis cannot reach a RUNTIME claim: what a contract does while
        // executing is only observable in a transaction or an event.
        let runtime = Candidate::new(
            entity(EntityKind::Contract, "C-a"),
            entity(EntityKind::Contract, "C-b"),
            Relationship::Invocates,
            Basis::InferredInterface,
            vec![citation(EvidenceType::Artifact, "artifact-1")],
        )
        .expect("a constructible candidate")
        .observed_at(boundary())
        .with_outcome(Some(true));
        // The refusal comes from the basis check rather than from the observation rule,
        // because an INVOCATES candidate's class set includes RUNTIME and the basis is
        // refused for the whole set at once. Reporting the more fundamental refusal is
        // the honest answer: collecting an observation would change the basis, not the
        // class set's admissibility.
        let error = classify(&runtime).expect_err("similarity is not an observation");
        assert!(
            error.to_string().contains("CONTRACT and RUNTIME"),
            "got: {error}"
        );
    }

    #[test]
    fn similarity_cannot_reach_a_package_or_artifact_claim() {
        for (object, cited, label) in [
            (
                entity(EntityKind::Package, "soroban-sdk"),
                EvidenceType::Source,
                "pkg",
            ),
            (
                entity(EntityKind::Wasm, "wasm-1"),
                EvidenceType::Wasm,
                "wasm",
            ),
        ] {
            let candidate = Candidate::new(
                entity(EntityKind::Contract, "C-a"),
                object,
                Relationship::DependsOn,
                Basis::InferredInterface,
                vec![citation(cited, "e-1")],
            )
            .expect("a constructible candidate")
            .observed_at(boundary());
            let error = classify(&candidate).expect_err("similarity is not evidence");
            assert!(
                error.to_string().contains("only for a CONTRACT dependency"),
                "{label} was accepted: {error}"
            );
        }
    }

    #[test]
    fn runtime_use_must_rest_on_a_transaction_recorded_as_successful() {
        let failed = invocation().with_outcome(Some(false));
        let error = classify(&failed).expect_err("a reverted call is not runtime use");
        assert!(
            error.to_string().contains("recorded as failed"),
            "got: {error}"
        );

        let unknown = invocation().with_outcome(None);
        let error = classify(&unknown).expect_err("an unknown outcome is not success");
        assert!(
            error.to_string().contains("unknown outcome"),
            "got: {error}"
        );
    }

    #[test]
    fn an_artifact_dependency_must_rest_on_a_digest_or_a_declared_input() {
        let digest = Candidate::new(
            entity(EntityKind::Contract, "C-a"),
            entity(EntityKind::Wasm, "wasm-digest"),
            Relationship::DependsOn,
            Basis::EmbeddedDigest,
            vec![citation(EvidenceType::Artifact, "artifact-1")],
        )
        .expect("a digest match");
        let classification = classify(&digest).expect("a digest is comparable evidence");
        assert_eq!(classification.classes, vec![DependencyClass::Wasm]);
        assert_eq!(
            classification.confidence.level,
            ConfidenceLevel::HighConfidence
        );

        let metadata = Candidate::new(
            entity(EntityKind::Contract, "C-a"),
            entity(EntityKind::Artifact, "some-artifact"),
            Relationship::DependsOn,
            Basis::ConfiguredEndpoint,
            vec![citation(EvidenceType::Artifact, "artifact-1")],
        )
        .expect("a constructible candidate");
        let error = classify(&metadata).expect_err("a configured endpoint is metadata");
        assert!(
            error.to_string().contains("nothing to re-compute"),
            "got: {error}"
        );
    }

    #[test]
    fn an_external_dependency_must_record_its_boundary_and_is_never_verified() {
        let without = Candidate::new(
            entity(EntityKind::Contract, "C-a"),
            entity(EntityKind::Source, "https://example.invalid/r@abc"),
            Relationship::DependsOn,
            Basis::DeclaredManifest,
            vec![citation(EvidenceType::Source, "src-1")],
        )
        .expect("a constructible candidate");
        let error = classify(&without).expect_err("the boundary was not recorded");
        assert!(
            error
                .to_string()
                .contains("not an entity that failed inspection"),
            "got: {error}"
        );

        let with = without.observed_at(boundary());
        let classification = classify(&with).expect("classifiable");
        assert_eq!(classification.classes, vec![DependencyClass::External]);
        assert_eq!(classification.verification, VerificationStatus::Unverified);
        assert!(!classification.is_observed());
        assert!(!DependencyClass::External.may_be_verified());
    }

    #[test]
    fn a_package_dependency_needs_source_build_or_artifact_evidence() {
        let candidate = Candidate::new(
            entity(EntityKind::Contract, "C-a"),
            entity(EntityKind::Package, "soroban-sdk 22.0.0"),
            Relationship::DependsOn,
            Basis::DeclaredManifest,
            vec![citation(EvidenceType::Event, "event-1")],
        )
        .expect("a constructible candidate")
        .observed_at(boundary());
        let error = classify(&candidate).expect_err("an event says nothing about a package");
        let message = error.to_string();
        assert!(message.contains("requires one of"), "got: {message}");
        assert!(message.contains("SOURCE"), "got: {message}");
    }

    #[test]
    fn a_package_dependency_on_a_lockfile_is_high_confidence_and_unverified() {
        let candidate = Candidate::new(
            entity(EntityKind::Contract, "C-a"),
            entity(EntityKind::Package, "soroban-sdk"),
            Relationship::DependsOn,
            Basis::ResolvedLockfile,
            vec![citation(EvidenceType::Build, "lock-1")],
        )
        .expect("a constructible candidate")
        .observed_at(boundary());
        let classification = classify(&candidate).expect("classifiable");
        assert_eq!(classification.classes, vec![DependencyClass::Package]);
        assert_eq!(
            classification.confidence.level,
            ConfidenceLevel::HighConfidence
        );
        assert_eq!(classification.verification, VerificationStatus::Unverified);
        assert!(!classification.is_observed());
        assert!(
            classification
                .confidence
                .rationale
                .as_deref()
                .is_some_and(|rationale| rationale.contains("ceiling"))
        );
    }

    #[test]
    fn derivation_and_verification_are_not_dependencies() {
        for relationship in [
            Relationship::DerivedFrom,
            Relationship::BuiltFrom,
            Relationship::DeployedAs,
            Relationship::ObservedIn,
            Relationship::VerifiedBy,
            Relationship::Affects,
        ] {
            let (object, kind) = match relationship {
                Relationship::DerivedFrom | Relationship::BuiltFrom => {
                    (entity(EntityKind::Source, "src"), EvidenceType::Source)
                },
                Relationship::ObservedIn => (
                    entity(EntityKind::Transaction, "tx"),
                    EvidenceType::Transaction,
                ),
                Relationship::DeployedAs => (
                    entity(EntityKind::Contract, "C-x"),
                    EvidenceType::Deployment,
                ),
                _ => (entity(EntityKind::Wasm, "wasm"), EvidenceType::Wasm),
            };
            let candidate = Candidate::new(
                entity(EntityKind::Artifact, "artifact-1"),
                object,
                relationship,
                Basis::EmbeddedDigest,
                vec![citation(kind, "e-1")],
            );
            let error = classify(&candidate.expect("a constructible candidate"))
                .expect_err("a derivation is not a dependency, whatever its object");
            assert!(
                error.to_string().contains("does not express a dependency"),
                "{relationship} was accepted: {error}"
            );
        }
    }

    #[test]
    fn the_relationship_alone_explains_the_refusal() {
        assert!(why_not_a_dependency(Relationship::BuiltFrom).contains("input"));
        assert!(why_not_a_dependency(Relationship::VerifiedBy).contains("does not create"));
    }

    #[test]
    fn a_build_target_is_refused_before_classification() {
        // `DEPENDS_ON` object kinds are [CONTRACT, WASM, ARTIFACT, PACKAGE, SOURCE], so a
        // build target cannot be expressed as a dependency at all. The refusal happens
        // where the candidate is constructed, which is why classification never sees
        // one - a per-kind message here would be unreachable code claiming to work.
        let error = Candidate::new(
            entity(EntityKind::Artifact, "artifact-1"),
            entity(EntityKind::Build, "build-1"),
            Relationship::DependsOn,
            Basis::EmbeddedDigest,
            vec![citation(EvidenceType::Build, "b-1")],
        )
        .expect_err("a build is not a permitted DEPENDS_ON object");
        assert!(error.to_string().contains("endpoint kinds"), "got: {error}");
    }

    #[test]
    fn contradicting_evidence_wins_over_a_verified_basis() {
        let candidate = invocation()
            .contradicted_by(vec![citation(EvidenceType::Transaction, "tx-2-reverted")])
            .expect("a contradiction with something behind it");
        let classification = classify(&candidate).expect("classifiable");
        assert_eq!(classification.verification, VerificationStatus::Conflicting);
        assert!(!classification.is_observed());
        assert_eq!(
            classification.confidence.level,
            ConfidenceLevel::LowConfidence
        );
        assert!(classification.confidence.is_contradicted());
    }

    #[test]
    fn a_candidate_must_cite_evidence_and_cannot_be_a_self_loop() {
        Candidate::new(
            entity(EntityKind::Contract, "C-a"),
            entity(EntityKind::Contract, "C-b"),
            Relationship::Invocates,
            Basis::ObservedInvocation,
            Vec::new(),
        )
        .expect_err("no evidence");

        Candidate::new(
            entity(EntityKind::Contract, "C-a"),
            entity(EntityKind::Contract, "C-a"),
            Relationship::Invocates,
            Basis::ObservedInvocation,
            vec![citation(EvidenceType::Transaction, "tx-1")],
        )
        .expect_err("a self-loop is a graph defect rather than a relationship");
    }

    #[test]
    fn a_contradiction_needs_something_that_contradicts() {
        invocation()
            .contradicted_by(Vec::new())
            .expect_err("an empty contradiction is not a contradiction");
    }

    #[test]
    fn endpoints_the_relationship_does_not_permit_are_refused_at_construction() {
        let error = Candidate::new(
            entity(EntityKind::Wasm, "wasm-1"),
            entity(EntityKind::Contract, "C-b"),
            Relationship::Invocates,
            Basis::ObservedInvocation,
            vec![citation(EvidenceType::Transaction, "tx-1")],
        )
        .expect_err("the relationship does not permit these endpoints");
        assert!(error.to_string().contains("endpoint kinds"));
    }

    #[test]
    fn an_empty_citation_is_refused() {
        EvidenceRef::new(EvidenceType::Transaction, "").expect_err("a citation names a record");
    }

    #[test]
    fn classification_is_deterministic() {
        let first = classify(&invocation()).expect("classifiable");
        let second = classify(&invocation()).expect("classifiable");
        assert_eq!(first, second);
        assert_eq!(
            serde_json::to_string(&first).expect("serialises"),
            serde_json::to_string(&second).expect("serialises")
        );
    }
}
