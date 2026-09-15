//! Relationship semantics, inference bases, confidence and verification.
//!
//! Four things are defined here, and each exists because the specification refuses
//! to let a weaker statement be presented as a stronger one:
//!
//! * [`Relationship`] carries its own [`ChangePropagation`], so impact analysis
//!   never infers a direction from the name of a relationship. Two relationships
//!   that look structurally identical - `DEPENDS_ON` and `VERIFIED_BY` - propagate
//!   in opposite ways, and one of them does not propagate at all.
//! * [`Basis`] names *how* a dependency was established, so
//!   [`Basis::InferredInterface`] is a labelled weak basis rather than either an
//!   omission or a promotion.
//! * [`Confidence`] is an ordering, not a score, and requires the evidence that
//!   supports it. A confidence with no evidence cannot be constructed.
//! * [`VerificationStatus`] is independent of confidence, so contradictory
//!   evidence is representable. A claim with any contradicted component is
//!   `CONFLICTING`, and that takes precedence over every other status.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::errors::{EngineError, Result};
use crate::identity::EntityKind;

/// The direction in which a change travels along a relationship.
///
/// Declared per relationship and never inferred. This is the only permitted source
/// of impact semantics in the engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangePropagation {
    /// A change to the object affects the subject: the subject depends on the
    /// object.
    ObjectToSubject,
    /// A change to the subject affects the object: the arrow already points in the
    /// direction of change.
    SubjectToObject,
    /// A change in either direction affects the other.
    Bidirectional,
    /// Impact must not propagate through this relationship in either direction.
    None,
}

impl ChangePropagation {
    /// The stable wire name, matching the taxonomy.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ObjectToSubject => "object_to_subject",
            Self::SubjectToObject => "subject_to_object",
            Self::Bidirectional => "bidirectional",
            Self::None => "none",
        }
    }

    /// Whether a change originating at the object reaches the subject.
    ///
    /// Stated as a method rather than left to each call site to compute, because
    /// the traversal code has to ask this question in several places and two
    /// slightly different answers would be a correctness bug in the impact
    /// analysis.
    #[must_use]
    pub const fn propagates_object_to_subject(self) -> bool {
        matches!(self, Self::ObjectToSubject | Self::Bidirectional)
    }

    /// Whether a change originating at the subject reaches the object.
    #[must_use]
    pub const fn propagates_subject_to_object(self) -> bool {
        matches!(self, Self::SubjectToObject | Self::Bidirectional)
    }

    /// Whether this relationship carries a change at all.
    ///
    /// `false` for [`ChangePropagation::None`]. The engine uses this to exclude
    /// observational and verification edges from every traversal, which is what
    /// stops a re-observation or a re-verification from being reported as a change.
    #[must_use]
    pub const fn propagates(self) -> bool {
        !matches!(self, Self::None)
    }
}

/// The typed relationships the specification defines.
///
/// Closed: an unrecognised relationship is rejected rather than tolerated, because
/// the engine cannot decide how a change travels along a relationship whose
/// semantics it does not know. Tolerating one would mean either treating it as
/// non-propagating - silently dropping real impact - or guessing a direction, and
/// both produce a plausible-looking wrong answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
#[non_exhaustive]
pub enum Relationship {
    /// The subject requires the object in order to function or be built.
    DependsOn,
    /// The subject contract made a cross-contract call into the object contract.
    Invocates,
    /// The subject build produced the subject from the object source revision.
    BuiltFrom,
    /// The subject artifact was mechanically produced from the object entity.
    DerivedFrom,
    /// An executable became a contract, or a contract came from a deployment.
    DeployedAs,
    /// The subject fact was observed in the object transaction.
    ObservedIn,
    /// The object entity's digest or revision establishes the subject's claim.
    VerifiedBy,
    /// A change to the subject is asserted to affect the object.
    Affects,
}

impl Relationship {
    /// How a change travels along this relationship.
    ///
    /// The rationale for each is recorded in `taxonomies/relationship-types.yaml`
    /// and is not repeated here, so that the taxonomy stays the single source.
    #[must_use]
    pub const fn change_propagation(self) -> ChangePropagation {
        match self {
            Self::DependsOn
            | Self::Invocates
            | Self::BuiltFrom
            | Self::DerivedFrom
            | Self::DeployedAs => ChangePropagation::ObjectToSubject,
            Self::Affects => ChangePropagation::SubjectToObject,
            Self::ObservedIn | Self::VerifiedBy => ChangePropagation::None,
        }
    }

    /// The stable wire name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::DependsOn => "DEPENDS_ON",
            Self::Invocates => "INVOCATES",
            Self::BuiltFrom => "BUILT_FROM",
            Self::DerivedFrom => "DERIVED_FROM",
            Self::DeployedAs => "DEPLOYED_AS",
            Self::ObservedIn => "OBSERVED_IN",
            Self::VerifiedBy => "VERIFIED_BY",
            Self::Affects => "AFFECTS",
        }
    }

    /// Every relationship, in the canonical order used for deterministic output.
    #[must_use]
    pub const fn all() -> &'static [Self] {
        &[
            Self::DependsOn,
            Self::Invocates,
            Self::BuiltFrom,
            Self::DerivedFrom,
            Self::DeployedAs,
            Self::ObservedIn,
            Self::VerifiedBy,
            Self::Affects,
        ]
    }

    /// The entity kinds this relationship may connect as subject.
    #[must_use]
    pub const fn subject_kinds(self) -> &'static [EntityKind] {
        match self {
            Self::DependsOn => &[
                EntityKind::Contract,
                EntityKind::Wasm,
                EntityKind::Artifact,
                EntityKind::Package,
                EntityKind::Build,
            ],
            Self::Invocates => &[EntityKind::Contract],
            Self::BuiltFrom => &[EntityKind::Build, EntityKind::Artifact, EntityKind::Wasm],
            Self::DerivedFrom => &[EntityKind::Artifact, EntityKind::Wasm, EntityKind::Build],
            Self::DeployedAs => &[EntityKind::Wasm, EntityKind::Artifact, EntityKind::Contract],
            Self::ObservedIn => &[
                EntityKind::Contract,
                EntityKind::Wasm,
                EntityKind::Artifact,
                EntityKind::Deployment,
            ],
            Self::VerifiedBy => &[
                EntityKind::Contract,
                EntityKind::Wasm,
                EntityKind::Artifact,
                EntityKind::Deployment,
                EntityKind::Source,
                EntityKind::Build,
            ],
            Self::Affects => &[
                EntityKind::Contract,
                EntityKind::Wasm,
                EntityKind::Artifact,
                EntityKind::Package,
                EntityKind::Source,
                EntityKind::Build,
                EntityKind::Deployment,
            ],
        }
    }

    /// The entity kinds this relationship may connect as object.
    #[must_use]
    pub const fn object_kinds(self) -> &'static [EntityKind] {
        match self {
            Self::DependsOn => &[
                EntityKind::Contract,
                EntityKind::Wasm,
                EntityKind::Artifact,
                EntityKind::Package,
                EntityKind::Source,
            ],
            Self::Invocates => &[EntityKind::Contract],
            Self::BuiltFrom => &[EntityKind::Source],
            Self::DerivedFrom => &[EntityKind::Artifact, EntityKind::Source, EntityKind::Build],
            Self::DeployedAs => &[EntityKind::Contract, EntityKind::Deployment],
            Self::ObservedIn => &[EntityKind::Transaction],
            Self::VerifiedBy => &[
                EntityKind::Build,
                EntityKind::Artifact,
                EntityKind::Source,
                EntityKind::Wasm,
            ],
            Self::Affects => &[
                EntityKind::Contract,
                EntityKind::Wasm,
                EntityKind::Artifact,
                EntityKind::Package,
                EntityKind::Deployment,
            ],
        }
    }

    /// Whether this relationship may connect the given kinds, in this direction.
    ///
    /// Checked when an edge is built rather than when it is read, so an
    /// impossible relationship cannot enter the graph and then be discovered by a
    /// consumer.
    #[must_use]
    pub fn permits(self, subject: EntityKind, object: EntityKind) -> bool {
        self.subject_kinds().contains(&subject) && self.object_kinds().contains(&object)
    }
}

impl fmt::Display for Relationship {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Relationship {
    type Err = EngineError;

    fn from_str(value: &str) -> Result<Self> {
        Self::all()
            .iter()
            .copied()
            .find(|candidate| candidate.as_str() == value)
            .ok_or_else(|| EngineError::Validation {
                path: "/relationship".to_owned(),
                detail: format!(
                    "unrecognised relationship {value:?}; the set is closed because a change \
                     cannot be propagated along a relationship whose semantics are unknown"
                ),
            })
    }
}

/// How a relationship was established.
///
/// The ordering of the variants is meaningful and is relied upon: a producer that
/// finds several bases available records the strongest one, and
/// [`Basis::is_structural`] distinguishes the bases that can establish a
/// requirement from the one that can only suggest it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
#[non_exhaustive]
pub enum Basis {
    /// The subject's own manifest declares the dependency.
    DeclaredManifest,
    /// A lockfile resolves the dependency to a specific artifact.
    ResolvedLockfile,
    /// A transaction records the interaction.
    ObservedInvocation,
    /// A contract event records the interaction.
    ObservedEvent,
    /// A digest embedded in the subject matches the target.
    EmbeddedDigest,
    /// The subject's configuration names the target.
    ConfiguredEndpoint,
    /// An issuer attests to the relationship.
    Attested,
    /// Only the target's interface is consistent with the claim.
    InferredInterface,
}

impl Basis {
    /// The stable wire name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::DeclaredManifest => "DECLARED_MANIFEST",
            Self::ResolvedLockfile => "RESOLVED_LOCKFILE",
            Self::ObservedInvocation => "OBSERVED_INVOCATION",
            Self::ObservedEvent => "OBSERVED_EVENT",
            Self::EmbeddedDigest => "EMBEDDED_DIGEST",
            Self::ConfiguredEndpoint => "CONFIGURED_ENDPOINT",
            Self::Attested => "ATTESTED",
            Self::InferredInterface => "INFERRED_INTERFACE",
        }
    }

    /// Every basis, ordered from strongest to weakest.
    #[must_use]
    pub const fn all_strongest_first() -> &'static [Self] {
        &[
            Self::ObservedInvocation,
            Self::ObservedEvent,
            Self::ResolvedLockfile,
            Self::EmbeddedDigest,
            Self::DeclaredManifest,
            Self::ConfiguredEndpoint,
            Self::Attested,
            Self::InferredInterface,
        ]
    }

    /// Whether this basis can establish that the subject *requires* the object.
    ///
    /// Interface similarity cannot: it is consistent with a requirement without
    /// being evidence of one, which is exactly why it is a separate basis rather
    /// than being excluded. A dependency carrying this basis is capped at
    /// [`ConfidenceLevel::LowConfidence`] and must not appear among a report's
    /// observed facts.
    #[must_use]
    pub const fn is_structural(self) -> bool {
        !matches!(self, Self::InferredInterface)
    }

    /// Whether this basis records something that was performed rather than
    /// something that was declared.
    ///
    /// The distinction drives the runtime-dependency rule: a `RUNTIME` dependency
    /// must rest on an observation, because a declaration cannot tell you what a
    /// contract actually invoked.
    #[must_use]
    pub const fn is_observed(self) -> bool {
        matches!(self, Self::ObservedInvocation | Self::ObservedEvent)
    }

    /// The highest confidence this basis can support on its own.
    #[must_use]
    pub const fn confidence_ceiling(self) -> ConfidenceLevel {
        match self {
            Self::ObservedInvocation | Self::ObservedEvent => ConfidenceLevel::Verified,
            Self::ResolvedLockfile | Self::EmbeddedDigest => ConfidenceLevel::HighConfidence,
            Self::DeclaredManifest | Self::ConfiguredEndpoint => ConfidenceLevel::MediumConfidence,
            Self::Attested => ConfidenceLevel::HighConfidence,
            Self::InferredInterface => ConfidenceLevel::LowConfidence,
        }
    }
}

impl fmt::Display for Basis {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Basis {
    type Err = EngineError;

    fn from_str(value: &str) -> Result<Self> {
        Self::all_strongest_first()
            .iter()
            .copied()
            .find(|candidate| candidate.as_str() == value)
            .ok_or_else(|| EngineError::Validation {
                path: "/basis".to_owned(),
                detail: format!("unrecognised inference basis {value:?}"),
            })
    }
}

/// How strongly the available evidence supports a claim.
///
/// An ordering rather than a score. [`ConfidenceLevel::ordinal`] is the only
/// comparison the engine defines, so aggregation is a property of the model rather
/// than of an implementation's taste.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
#[non_exhaustive]
pub enum ConfidenceLevel {
    /// No evidence supporting the claim is available, or the evidence cannot be
    /// interpreted at this specification version.
    Unknown,
    /// Supported only by indirect or circumstantial evidence.
    LowConfidence,
    /// Supported by evidence that is directionally correct but incomplete.
    MediumConfidence,
    /// Supported by authoritative evidence that cannot be independently
    /// reproduced from the Amasario record alone.
    HighConfidence,
    /// Supported by complete, internally consistent, independently checkable
    /// evidence.
    Verified,
}

impl ConfidenceLevel {
    /// The stability of each level, used for every aggregation.
    #[must_use]
    pub const fn ordinal(self) -> u8 {
        match self {
            Self::Unknown => 0,
            Self::LowConfidence => 2,
            Self::MediumConfidence => 3,
            Self::HighConfidence => 4,
            Self::Verified => 5,
        }
    }

    /// The stable wire name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Unknown => "UNKNOWN",
            Self::LowConfidence => "LOW_CONFIDENCE",
            Self::MediumConfidence => "MEDIUM_CONFIDENCE",
            Self::HighConfidence => "HIGH_CONFIDENCE",
            Self::Verified => "VERIFIED",
        }
    }

    /// Every level, from weakest to strongest.
    #[must_use]
    pub const fn all_weakest_first() -> &'static [Self] {
        &[
            Self::Unknown,
            Self::LowConfidence,
            Self::MediumConfidence,
            Self::HighConfidence,
            Self::Verified,
        ]
    }

    /// The weakest of two levels.
    ///
    /// The aggregation primitive: a chain is only as strong as its weakest link, so
    /// this is how a path's confidence is computed and how a claim's confidence is
    /// capped by its basis.
    #[must_use]
    pub const fn weakest(self, other: Self) -> Self {
        if self.ordinal() <= other.ordinal() {
            self
        } else {
            other
        }
    }

    /// The weakest level in a sequence, or [`ConfidenceLevel::Verified`] when the
    /// sequence is empty.
    ///
    /// The empty case returns the identity element of the aggregation rather than
    /// `Unknown`, matching the taxonomy's declaration: an empty conjunction is
    /// vacuously satisfied, and returning `Unknown` would make an entity with no
    /// evidence indistinguishable from one whose evidence is uninterpretable.
    #[must_use]
    pub fn weakest_of(levels: impl IntoIterator<Item = Self>) -> Self {
        levels
            .into_iter()
            .fold(Self::Verified, |accumulated, level| {
                accumulated.weakest(level)
            })
    }
}

impl fmt::Display for ConfidenceLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for ConfidenceLevel {
    type Err = EngineError;

    fn from_str(value: &str) -> Result<Self> {
        Self::all_weakest_first()
            .iter()
            .copied()
            .find(|candidate| candidate.as_str() == value)
            .ok_or_else(|| EngineError::Validation {
                path: "/confidence/level".to_owned(),
                detail: format!("unrecognised confidence level {value:?}"),
            })
    }
}

/// A confidence level together with the evidence that supports it.
///
/// The evidence is not optional. A confidence level on its own is exactly the
/// shape of claim the specification exists to prevent: it asserts a degree of
/// support without naming what supports it, and a reader cannot disagree with it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Confidence {
    /// How strongly the evidence supports the claim.
    pub level: ConfidenceLevel,
    /// Identifiers of the evidence records that support the claim. Never empty.
    pub evidence: Vec<String>,
    /// Identifiers of the evidence records that contradict the claim.
    ///
    /// Recorded rather than folded into the level, because the counter-evidence is
    /// what determines the outcome and a reader needs to see it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub contradicting_evidence: Vec<String>,
    /// Why this level, in terms of the cited evidence.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rationale: Option<String>,
}

impl Confidence {
    /// Builds a confidence, rejecting an empty evidence list.
    pub fn new(
        level: ConfidenceLevel,
        evidence: Vec<String>,
        contradicting_evidence: Vec<String>,
    ) -> Result<Self> {
        if evidence.is_empty() {
            return Err(EngineError::Validation {
                path: "/confidence/evidence".to_owned(),
                detail: "a confidence level must name the evidence that supports it; \
                         a bare level is a claim with nothing behind it"
                    .to_owned(),
            });
        }
        Ok(Self {
            level,
            evidence,
            contradicting_evidence,
            rationale: None,
        })
    }

    /// Attaches a rationale.
    #[must_use]
    pub fn with_rationale(mut self, rationale: impl Into<String>) -> Self {
        self.rationale = Some(rationale.into());
        self
    }

    /// Whether any evidence contradicts the claim.
    #[must_use]
    pub const fn is_contradicted(&self) -> bool {
        !self.contradicting_evidence.is_empty()
    }

    /// Whether any evidence exists at all.
    ///
    /// Used by the verification model: a claim with contradicting evidence is
    /// `CONFLICTING` regardless of the level, while a claim with no evidence is
    /// `UNVERIFIED`.
    #[must_use]
    pub const fn has_support(&self) -> bool {
        !self.evidence.is_empty()
    }
}

/// What the evidence says about a claim.
///
/// Independent of confidence. Confidence asks how much evidence there is;
/// verification asks what that evidence says. They diverge legitimately, and the
/// case that makes the distinction necessary is a claim with extensive evidence
/// that *contradicts* it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
#[non_exhaustive]
pub enum VerificationStatus {
    /// Evidence exists and contradicts the claim. Takes precedence over every
    /// other status.
    Conflicting,
    /// Supported, and the decisive evidence was checked successfully.
    Verified,
    /// Some components checked successfully; none contradicted.
    PartiallyVerified,
    /// Not checked, or the evidence needed to check it is unavailable. Makes no
    /// statement about the claim's truth.
    Unverified,
    /// The claim could not be evaluated at all.
    Unknown,
}

impl VerificationStatus {
    /// The stable wire name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Conflicting => "CONFLICTING",
            Self::Verified => "VERIFIED",
            Self::PartiallyVerified => "PARTIALLY_VERIFIED",
            Self::Unverified => "UNVERIFIED",
            Self::Unknown => "UNKNOWN",
        }
    }

    /// Whether this status asserts that the claim was refuted.
    #[must_use]
    pub const fn is_refutation(self) -> bool {
        matches!(self, Self::Conflicting)
    }

    /// Whether this status asserts that the claim was checked and not refuted.
    #[must_use]
    pub const fn is_affirmation(self) -> bool {
        matches!(self, Self::Verified | Self::PartiallyVerified)
    }

    /// Whether this status means the claim was not evaluated.
    ///
    /// `UNVERIFIED` and `UNKNOWN` both mean no conclusion was reached, and neither
    /// is a statement that the claim is false. A report must present them
    /// separately from a refutation, which is why the code asks this rather than
    /// comparing for equality at each call site.
    #[must_use]
    pub const fn is_inconclusive(self) -> bool {
        matches!(self, Self::Unverified | Self::Unknown)
    }

    /// Combines two statuses, preserving the precedence rule.
    ///
    /// `CONFLICTING` wins over everything, an affirmation beats an inconclusive
    /// result, and two inconclusive results resolve to the weaker of the two
    /// (`UNVERIFIED` beats `UNKNOWN`, because knowing a claim is unchecked says
    /// more than not being able to evaluate it).
    #[must_use]
    pub const fn combine(self, other: Self) -> Self {
        match (self, other) {
            (Self::Conflicting, _) | (_, Self::Conflicting) => Self::Conflicting,
            (Self::Verified, Self::Verified) => Self::Verified,
            (Self::Verified, Self::PartiallyVerified)
            | (Self::PartiallyVerified, Self::Verified)
            | (Self::PartiallyVerified, Self::PartiallyVerified) => Self::PartiallyVerified,
            (Self::Verified | Self::PartiallyVerified, _)
            | (_, Self::Verified | Self::PartiallyVerified) => Self::PartiallyVerified,
            (Self::Unverified, _) | (_, Self::Unverified) => Self::Unverified,
            _ => Self::Unknown,
        }
    }
}

impl fmt::Display for VerificationStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for VerificationStatus {
    type Err = EngineError;

    fn from_str(value: &str) -> Result<Self> {
        match value {
            "CONFLICTING" => Ok(Self::Conflicting),
            "VERIFIED" => Ok(Self::Verified),
            "PARTIALLY_VERIFIED" => Ok(Self::PartiallyVerified),
            "UNVERIFIED" => Ok(Self::Unverified),
            "UNKNOWN" => Ok(Self::Unknown),
            other => Err(EngineError::Validation {
                path: "/verificationStatus".to_owned(),
                detail: format!("unrecognised verification status {other:?}"),
            }),
        }
    }
}

/// How a dependency is classified.
///
/// Each class declares the evidence kinds that must be present for a dependency of
/// that class to be assertable at all. That requirement is what stops a claim from
/// being upgraded by relabelling it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
#[non_exhaustive]
pub enum DependencyClass {
    /// The subject requires the object with no intermediate entity.
    Direct,
    /// The subject reaches the object only through intermediates.
    Transitive,
    /// The object is another Soroban contract.
    Contract,
    /// The object is a software package or crate.
    Package,
    /// The object is a WASM artifact.
    Wasm,
    /// The dependency exists only while the subject executes.
    Runtime,
    /// The object lies outside the observable boundary.
    External,
}

impl DependencyClass {
    /// The stable wire name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Direct => "DIRECT",
            Self::Transitive => "TRANSITIVE",
            Self::Contract => "CONTRACT",
            Self::Package => "PACKAGE",
            Self::Wasm => "WASM",
            Self::Runtime => "RUNTIME",
            Self::External => "EXTERNAL",
        }
    }

    /// Every class, in the canonical order used for deterministic output.
    #[must_use]
    pub const fn all() -> &'static [Self] {
        &[
            Self::Direct,
            Self::Transitive,
            Self::Contract,
            Self::Package,
            Self::Wasm,
            Self::Runtime,
            Self::External,
        ]
    }

    /// The evidence types that must be present for this class to be assertable.
    #[must_use]
    pub const fn required_evidence(self) -> &'static [EvidenceType] {
        match self {
            Self::Direct | Self::Transitive => &[
                EvidenceType::Source,
                EvidenceType::Build,
                EvidenceType::Artifact,
                EvidenceType::Deployment,
                EvidenceType::Transaction,
            ],
            Self::Contract => &[
                EvidenceType::Transaction,
                EvidenceType::Event,
                EvidenceType::Source,
                EvidenceType::Artifact,
            ],
            Self::Package => &[
                EvidenceType::Source,
                EvidenceType::Build,
                EvidenceType::Artifact,
            ],
            Self::Wasm => &[
                EvidenceType::Artifact,
                EvidenceType::Build,
                EvidenceType::Deployment,
            ],
            Self::Runtime => &[EvidenceType::Transaction, EvidenceType::Event],
            Self::External => &[EvidenceType::Source, EvidenceType::Observation],
        }
    }

    /// Whether a dependency of this class must carry its path.
    #[must_use]
    pub const fn requires_path(self) -> bool {
        matches!(self, Self::Transitive)
    }

    /// Whether a dependency of this class must name the network it was observed
    /// on.
    #[must_use]
    pub const fn requires_network(self) -> bool {
        matches!(self, Self::Contract | Self::Runtime)
    }

    /// Whether a dependency of this class must rest on an observation.
    #[must_use]
    pub const fn requires_observation(self) -> bool {
        matches!(self, Self::Runtime)
    }

    /// Whether a dependency of this class may ever be reported as verified.
    ///
    /// An out-of-boundary target cannot be, because the engine cannot inspect it.
    /// That is a correct configuration rather than a failure, which is why the
    /// class exists as a distinct value.
    #[must_use]
    pub const fn may_be_verified(self) -> bool {
        !matches!(self, Self::External)
    }
}

impl fmt::Display for DependencyClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for DependencyClass {
    type Err = EngineError;

    fn from_str(value: &str) -> Result<Self> {
        Self::all()
            .iter()
            .copied()
            .find(|candidate| candidate.as_str() == value)
            .ok_or_else(|| EngineError::Validation {
                path: "/type".to_owned(),
                detail: format!("unrecognised dependency class {value:?}"),
            })
    }
}

/// The kinds of evidence the specification defines.
///
/// Each declares what a reviewer would consult to confirm it independently, which
/// is what makes "the evidence is traceable" a checkable statement rather than a
/// rhetorical one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
#[non_exhaustive]
pub enum EvidenceType {
    /// Evidence about a source repository at a revision.
    Source,
    /// Evidence about how an artifact was produced.
    Build,
    /// Evidence about an artifact's existence and identity.
    Artifact,
    /// Evidence about a deployed executable's identity.
    Wasm,
    /// Evidence that an executable became a contract.
    Deployment,
    /// Evidence from a ledger transaction.
    Transaction,
    /// Evidence from a contract event.
    Event,
    /// Evidence asserted by an issuer.
    Attestation,
    /// A bare observation recorded at a boundary.
    Observation,
}

impl EvidenceType {
    /// The stable wire name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Source => "SOURCE",
            Self::Build => "BUILD",
            Self::Artifact => "ARTIFACT",
            Self::Wasm => "WASM",
            Self::Deployment => "DEPLOYMENT",
            Self::Transaction => "TRANSACTION",
            Self::Event => "EVENT",
            Self::Attestation => "ATTESTATION",
            Self::Observation => "OBSERVATION",
        }
    }

    /// What a reviewer consults to confirm this evidence independently.
    ///
    /// A `SOURCE` record citing a repository but no revision is not traceable to
    /// what this returns, which is how the traceability requirement becomes
    /// checkable rather than aspirational.
    #[must_use]
    pub const fn traceable_to(self) -> &'static str {
        match self {
            Self::Source => "source repository at the recorded revision",
            Self::Build => "build record and its declared inputs",
            Self::Artifact => "artifact content addressed by the digest",
            Self::Wasm => "network observation at a recorded ledger boundary",
            Self::Deployment => "deployment transaction and ledger",
            Self::Transaction => "transaction hash on a named network",
            Self::Event => "emitting transaction and the event's position within it",
            Self::Attestation => "attestation record and its issuer",
            Self::Observation => "observation record and the boundary it was made at",
        }
    }

    /// Every evidence type, in the canonical order.
    #[must_use]
    pub const fn all() -> &'static [Self] {
        &[
            Self::Source,
            Self::Build,
            Self::Artifact,
            Self::Wasm,
            Self::Deployment,
            Self::Transaction,
            Self::Event,
            Self::Attestation,
            Self::Observation,
        ]
    }
}

impl fmt::Display for EvidenceType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for EvidenceType {
    type Err = EngineError;

    fn from_str(value: &str) -> Result<Self> {
        Self::all()
            .iter()
            .copied()
            .find(|candidate| candidate.as_str() == value)
            .ok_or_else(|| EngineError::Validation {
                path: "/type".to_owned(),
                detail: format!("unrecognised evidence type {value:?}"),
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_relationship_declares_a_change_propagation() {
        for relationship in Relationship::all() {
            // The declaration is total: there is no relationship for which the
            // engine would have to guess a direction.
            let propagation = relationship.change_propagation();
            assert!(
                !propagation.propagates()
                    || propagation.propagates_object_to_subject()
                    || propagation.propagates_subject_to_object(),
                "{relationship} declares no usable propagation"
            );
        }
    }

    #[test]
    fn observational_and_verification_relationships_do_not_propagate() {
        // Propagating through either would make a new observation or a
        // re-verification look like a change.
        assert_eq!(
            Relationship::ObservedIn.change_propagation(),
            ChangePropagation::None
        );
        assert_eq!(
            Relationship::VerifiedBy.change_propagation(),
            ChangePropagation::None
        );
    }

    #[test]
    fn dependency_relationships_propagate_from_the_object_to_the_subject() {
        // A change to a dependency reaches the dependent, not the reverse.
        for relationship in [
            Relationship::DependsOn,
            Relationship::Invocates,
            Relationship::BuiltFrom,
            Relationship::DerivedFrom,
            Relationship::DeployedAs,
        ] {
            assert_eq!(
                relationship.change_propagation(),
                ChangePropagation::ObjectToSubject,
                "{relationship}"
            );
            assert!(
                relationship
                    .change_propagation()
                    .propagates_object_to_subject()
            );
            assert!(
                !relationship
                    .change_propagation()
                    .propagates_subject_to_object()
            );
        }
    }

    #[test]
    fn affects_propagates_in_its_own_direction_without_inversion() {
        let propagation = Relationship::Affects.change_propagation();
        assert_eq!(propagation, ChangePropagation::SubjectToObject);
        assert!(propagation.propagates_subject_to_object());
        assert!(!propagation.propagates_object_to_subject());
    }

    #[test]
    fn the_direction_a_change_travels_is_not_the_direction_the_arrow_points() {
        // BUILT_FROM points from the build to its source, yet changing the source
        // invalidates the build. Deriving direction from the name would get this
        // backwards, which is why the direction is declared.
        let propagation = Relationship::BuiltFrom.change_propagation();
        assert!(propagation.propagates_object_to_subject());
        assert!(!propagation.propagates_subject_to_object());
    }

    #[test]
    fn relationships_round_trip_through_their_wire_names() {
        for relationship in Relationship::all() {
            assert_eq!(
                Relationship::from_str(relationship.as_str()).expect("round trip"),
                *relationship
            );
        }
        assert_eq!(Relationship::all().len(), 8);
    }

    #[test]
    fn an_unrecognised_relationship_is_rejected() {
        let error = Relationship::from_str("CALLS").expect_err("CALLS is not a relationship");
        assert!(error.to_string().contains("closed"));
    }

    #[test]
    fn every_relationship_permits_at_least_one_subject_and_object_kind() {
        for relationship in Relationship::all() {
            assert!(
                !relationship.subject_kinds().is_empty(),
                "{relationship} permits no subject kind"
            );
            assert!(
                !relationship.object_kinds().is_empty(),
                "{relationship} permits no object kind"
            );
        }
    }

    #[test]
    fn an_impossible_relationship_is_refused() {
        // An invocation is between two contracts.
        assert!(Relationship::Invocates.permits(EntityKind::Contract, EntityKind::Contract));
        assert!(!Relationship::Invocates.permits(EntityKind::Source, EntityKind::Contract));
        // A build comes from a source, not from a deployment.
        assert!(Relationship::BuiltFrom.permits(EntityKind::Build, EntityKind::Source));
        assert!(!Relationship::BuiltFrom.permits(EntityKind::Build, EntityKind::Deployment));
    }

    #[test]
    fn bases_round_trip_and_are_ordered_from_strongest_to_weakest() {
        for basis in Basis::all_strongest_first() {
            assert_eq!(Basis::from_str(basis.as_str()).expect("round trip"), *basis);
        }
        assert_eq!(
            Basis::all_strongest_first()[0],
            Basis::ObservedInvocation,
            "an observation is the strongest basis"
        );
        assert_eq!(
            *Basis::all_strongest_first().last().expect("non-empty"),
            Basis::InferredInterface,
            "interface similarity is the weakest"
        );
    }

    #[test]
    fn only_an_inferred_interface_basis_cannot_establish_a_requirement() {
        for basis in Basis::all_strongest_first() {
            let expected = *basis != Basis::InferredInterface;
            assert_eq!(basis.is_structural(), expected, "{basis}");
        }
    }

    #[test]
    fn an_inferred_interface_basis_is_capped_at_low_confidence() {
        assert_eq!(
            Basis::InferredInterface.confidence_ceiling(),
            ConfidenceLevel::LowConfidence
        );
        // The cap is what stops interface similarity from being presented as an
        // established relationship.
        assert!(
            InferenceCeiling::of(Basis::InferredInterface)
                < ConfidenceLevel::MediumConfidence.ordinal()
        );
    }

    #[test]
    fn an_external_dependency_is_the_only_class_that_cannot_be_verified() {
        for class in DependencyClass::all() {
            let expected = *class != DependencyClass::External;
            assert_eq!(class.may_be_verified(), expected, "{class}");
        }
    }

    #[test]
    fn a_transitive_dependency_is_the_only_class_requiring_a_path() {
        for class in DependencyClass::all() {
            let expected = *class == DependencyClass::Transitive;
            assert_eq!(class.requires_path(), expected, "{class}");
        }
    }

    #[test]
    fn only_contract_and_runtime_classes_require_a_network() {
        for class in DependencyClass::all() {
            let expected = matches!(class, DependencyClass::Contract | DependencyClass::Runtime);
            assert_eq!(class.requires_network(), expected, "{class}");
        }
    }

    #[test]
    fn a_runtime_dependency_must_rest_on_an_observation() {
        assert!(DependencyClass::Runtime.requires_observation());
        assert!(!DependencyClass::Package.requires_observation());
        // And the observation must be one of the observational bases.
        assert!(Basis::ObservedInvocation.is_observed());
        assert!(!Basis::DeclaredManifest.is_observed());
    }

    #[test]
    fn every_dependency_class_declares_required_evidence() {
        for class in DependencyClass::all() {
            assert!(
                !class.required_evidence().is_empty(),
                "{class} requires no evidence, which would make the class a label rather than a gate"
            );
        }
    }

    #[test]
    fn confidence_is_an_ordering_and_not_a_score() {
        assert!(ConfidenceLevel::Unknown.ordinal() < ConfidenceLevel::LowConfidence.ordinal());
        assert!(
            ConfidenceLevel::LowConfidence.ordinal() < ConfidenceLevel::MediumConfidence.ordinal()
        );
        assert!(
            ConfidenceLevel::MediumConfidence.ordinal() < ConfidenceLevel::HighConfidence.ordinal()
        );
        assert!(ConfidenceLevel::HighConfidence.ordinal() < ConfidenceLevel::Verified.ordinal());
        assert_eq!(ConfidenceLevel::Unknown.ordinal(), 0);
    }

    #[test]
    fn confidence_aggregates_by_taking_the_weakest_link() {
        let levels = [
            ConfidenceLevel::Verified,
            ConfidenceLevel::HighConfidence,
            ConfidenceLevel::Verified,
        ];
        assert_eq!(
            ConfidenceLevel::weakest_of(levels),
            ConfidenceLevel::HighConfidence
        );
    }

    #[test]
    fn unknown_is_absorbing_so_an_unknown_link_cannot_be_outvoted() {
        let levels = [
            ConfidenceLevel::Verified,
            ConfidenceLevel::Unknown,
            ConfidenceLevel::Verified,
            ConfidenceLevel::HighConfidence,
        ];
        assert_eq!(
            ConfidenceLevel::weakest_of(levels),
            ConfidenceLevel::Unknown
        );
    }

    #[test]
    fn the_empty_aggregation_is_the_identity_element() {
        // An empty conjunction is vacuously satisfied, and returning Unknown would
        // make an entity with no evidence indistinguishable from one whose evidence
        // could not be interpreted.
        assert_eq!(
            ConfidenceLevel::weakest_of(std::iter::empty()),
            ConfidenceLevel::Verified
        );
        assert_eq!(
            ConfidenceLevel::Verified.weakest(ConfidenceLevel::HighConfidence),
            ConfidenceLevel::HighConfidence
        );
    }

    #[test]
    fn a_confidence_level_cannot_be_constructed_without_evidence() {
        let error = Confidence::new(ConfidenceLevel::Verified, Vec::new(), Vec::new())
            .expect_err("a bare level supports nothing");
        assert!(error.to_string().contains("must name the evidence"));
    }

    #[test]
    fn a_confidence_records_contradicting_evidence_separately_from_supporting_evidence() {
        let confidence = Confidence::new(
            ConfidenceLevel::MediumConfidence,
            vec!["ev-support".to_owned()],
            vec!["ev-against".to_owned()],
        )
        .expect("both lists are non-empty")
        .with_rationale("the rebuild disagreed with the claimed revision");

        assert!(confidence.has_support());
        assert!(confidence.is_contradicted());
        assert_eq!(
            confidence.rationale.as_deref(),
            Some("the rebuild disagreed with the claimed revision")
        );
    }

    #[test]
    fn conflicting_takes_precedence_over_every_other_status() {
        for status in [
            VerificationStatus::Verified,
            VerificationStatus::PartiallyVerified,
            VerificationStatus::Unverified,
            VerificationStatus::Unknown,
        ] {
            assert_eq!(
                VerificationStatus::Conflicting.combine(status),
                VerificationStatus::Conflicting,
                "combining with {status}"
            );
            assert_eq!(
                status.combine(VerificationStatus::Conflicting),
                VerificationStatus::Conflicting,
                "combining {status} with conflicting"
            );
        }
    }

    #[test]
    fn two_affirmations_resolve_to_the_weaker_affirmation() {
        assert_eq!(
            VerificationStatus::Verified.combine(VerificationStatus::Verified),
            VerificationStatus::Verified
        );
        assert_eq!(
            VerificationStatus::Verified.combine(VerificationStatus::PartiallyVerified),
            VerificationStatus::PartiallyVerified
        );
    }

    #[test]
    fn an_affirmation_combined_with_an_inconclusive_result_is_only_partial() {
        assert_eq!(
            VerificationStatus::Verified.combine(VerificationStatus::Unverified),
            VerificationStatus::PartiallyVerified
        );
        assert_eq!(
            VerificationStatus::PartiallyVerified.combine(VerificationStatus::Unknown),
            VerificationStatus::PartiallyVerified
        );
    }

    #[test]
    fn unverified_beats_unknown_because_it_carries_more_information() {
        assert_eq!(
            VerificationStatus::Unverified.combine(VerificationStatus::Unknown),
            VerificationStatus::Unverified
        );
        assert_eq!(
            VerificationStatus::Unknown.combine(VerificationStatus::Unverified),
            VerificationStatus::Unverified
        );
    }

    #[test]
    fn an_unverified_status_is_never_a_refutation() {
        // Reading "not checked" as "false" is the misreading the status axes exist
        // to prevent, so the distinction is asserted rather than assumed.
        assert!(!VerificationStatus::Unverified.is_refutation());
        assert!(!VerificationStatus::Unknown.is_refutation());
        assert!(VerificationStatus::Unverified.is_inconclusive());
        assert!(VerificationStatus::Unknown.is_inconclusive());
        assert!(!VerificationStatus::Conflicting.is_inconclusive());
    }

    #[test]
    fn affections_and_refutations_are_distinguishable_from_inconclusive_results() {
        assert!(VerificationStatus::Verified.is_affirmation());
        assert!(VerificationStatus::PartiallyVerified.is_affirmation());
        assert!(!VerificationStatus::Unverified.is_affirmation());
        assert!(!VerificationStatus::Unknown.is_affirmation());
        assert!(VerificationStatus::Conflicting.is_refutation());
    }

    #[test]
    fn verification_statuses_round_trip_through_their_wire_names() {
        let statuses = [
            VerificationStatus::Conflicting,
            VerificationStatus::Verified,
            VerificationStatus::PartiallyVerified,
            VerificationStatus::Unverified,
            VerificationStatus::Unknown,
        ];
        for status in statuses {
            assert_eq!(
                VerificationStatus::from_str(status.as_str()).expect("round trip"),
                status
            );
        }
        VerificationStatus::from_str("MAYBE").expect_err("MAYBE is not a status");
    }

    #[test]
    fn every_evidence_type_declares_what_makes_it_traceable() {
        for evidence_type in EvidenceType::all() {
            let target = evidence_type.traceable_to();
            assert!(
                !target.is_empty(),
                "{evidence_type} has no traceability target"
            );
            assert_eq!(
                EvidenceType::from_str(evidence_type.as_str()).expect("round trip"),
                *evidence_type
            );
        }
        assert_eq!(EvidenceType::all().len(), 9);
    }

    #[test]
    fn the_source_evidence_target_names_a_revision_rather_than_a_repository() {
        // A repository alone is mutable, so it is not something a reviewer can
        // consult to confirm a claim about specific content.
        assert!(EvidenceType::Source.traceable_to().contains("revision"));
    }

    #[test]
    fn dependency_classes_round_trip_through_their_wire_names() {
        for class in DependencyClass::all() {
            assert_eq!(
                DependencyClass::from_str(class.as_str()).expect("round trip"),
                *class
            );
        }
        assert_eq!(DependencyClass::all().len(), 7);
    }

    /// Convenience for the confidence-ceiling assertion above.
    struct InferenceCeiling;

    impl InferenceCeiling {
        fn of(basis: Basis) -> u8 {
            basis.confidence_ceiling().ordinal()
        }
    }
}
