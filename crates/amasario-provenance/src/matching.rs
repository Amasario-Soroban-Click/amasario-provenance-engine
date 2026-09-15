//! The provenance chain, and how two facts are compared.
//!
//! # The chain
//!
//! The specification's chain is
//! `SOURCE → REVISION → BUILD → ARTIFACT → WASM → DEPLOYMENT → CONTRACT`, and
//! [`ProvenanceChain`] represents it as a sequence of [`ChainLinkKind`] stages. Each
//! stage is a claim, and each claim carries the basis it rests on and the confidence
//! it reached, so that [verification](crate::verification) can combine them without
//! re-reading the evidence.
//!
//! Stages are appended in order and `push` rejects a stage that would arrive out of
//! sequence. That check is not tidiness: a chain whose links are in the wrong order
//! describes a provenance that cannot exist, and a verification pass that accepted it
//! would report a conclusion about a relationship that was never claimed.
//!
//! # Comparison is three-valued
//!
//! [`MatchOutcome`] has an `Incomparable` variant, and its existence is the point.
//! Two digests computed under different algorithms are not equal and not unequal;
//! they are not comparable at all, and a boolean comparison would report them as
//! differing - which reads as a contradiction and is a much stronger statement than
//! the facts support. The same applies to two revisions of different kinds, where one
//! may identify a tree the other does not.

use std::fmt;
use std::str::FromStr;

use amasario_core::{
    Basis, Confidence, Digest, EngineError, EntityKind, EntityRef, Relationship, Result,
};
use serde::{Deserialize, Serialize};

use crate::errors::ProvenanceFailure;
use crate::source::Revision;

/// One stage of the provenance chain.
///
/// Ordered as the specification states the chain, and `Ord` follows that order so
/// that a chain can be checked for sequence by comparing stage positions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
#[non_exhaustive]
pub enum ChainLinkKind {
    /// The source record names a revision that resolves to one immutable tree.
    ///
    /// A property of the source record rather than an edge: there is no second entity
    /// on the other side of it, which is why [`ChainLinkKind::relationship`] returns
    /// `None` here.
    SourceResolved,
    /// The build was built from the source revision.
    SourceToBuild,
    /// The artifact was derived from the build.
    BuildToArtifact,
    /// The WASM module was derived from the artifact.
    ///
    /// A build that emits the module directly still records this stage: the artifact
    /// it produced is the module's bytes, so the derivation is one step and the link
    /// is truthful. An absent link here is therefore a gap like any other, which is
    /// the conservative reading - the engine has no evidence of the derivation, so it
    /// must not present the chain as complete.
    ArtifactToWasm,
    /// The module was the executable the deployment placed.
    ///
    /// Realised as `WASM DEPLOYED_AS DEPLOYMENT`, so this stage's object is the
    /// deployment record rather than the module; see
    /// [`ChainLinkKind::subject_kind`].
    WasmToDeployment,
    /// The contract is the entity that deployment produced.
    ///
    /// Realised as `CONTRACT DEPLOYED_AS DEPLOYMENT` - the specification's other
    /// stated instance of the relationship - so this stage's subject is the contract
    /// and its object is the deployment record.
    DeploymentToContract,
}

impl ChainLinkKind {
    /// The stable wire name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SourceResolved => "SOURCE_RESOLVED",
            Self::SourceToBuild => "SOURCE_TO_BUILD",
            Self::BuildToArtifact => "BUILD_TO_ARTIFACT",
            Self::ArtifactToWasm => "ARTIFACT_TO_WASM",
            Self::WasmToDeployment => "WASM_TO_DEPLOYMENT",
            Self::DeploymentToContract => "DEPLOYMENT_TO_CONTRACT",
        }
    }

    /// Every stage, in chain order.
    #[must_use]
    pub const fn all() -> &'static [Self] {
        &[
            Self::SourceResolved,
            Self::SourceToBuild,
            Self::BuildToArtifact,
            Self::ArtifactToWasm,
            Self::WasmToDeployment,
            Self::DeploymentToContract,
        ]
    }

    /// The position of this stage in the chain.
    #[must_use]
    pub const fn position(self) -> usize {
        match self {
            Self::SourceResolved => 0,
            Self::SourceToBuild => 1,
            Self::BuildToArtifact => 2,
            Self::ArtifactToWasm => 3,
            Self::WasmToDeployment => 4,
            Self::DeploymentToContract => 5,
        }
    }

    /// The relationship that realises this stage, where one does.
    ///
    /// `None` for [`ChainLinkKind::SourceResolved`], which is a property of the
    /// source record rather than an edge between two entities. Returning `None` rather
    /// than forcing a relationship means the chain cannot claim an edge that does not
    /// exist.
    #[must_use]
    pub const fn relationship(self) -> Option<Relationship> {
        match self {
            Self::SourceResolved => None,
            Self::SourceToBuild => Some(Relationship::BuiltFrom),
            Self::BuildToArtifact | Self::ArtifactToWasm => Some(Relationship::DerivedFrom),
            Self::WasmToDeployment | Self::DeploymentToContract => Some(Relationship::DeployedAs),
        }
    }

    /// The entity kind this stage's subject is.
    ///
    /// # The two `DEPLOYED_AS` stages run the other way
    ///
    /// The taxonomy declares `DEPLOYED_AS` with `subjectTypes`
    /// `[WASM, ARTIFACT, CONTRACT]` and `objectTypes` `[CONTRACT, DEPLOYMENT]`.
    /// `DEPLOYMENT` is therefore never a `DEPLOYED_AS` subject, which is why both
    /// final stages take the deployment record as their *object* even though the chain
    /// reads in the opposite direction.
    ///
    /// The direction is the specification's, not a choice made here: `DEPLOYED_AS`
    /// relates the deployed entity to the thing it was deployed as, so
    /// [`ChainLinkKind::WasmToDeployment`] is `WASM DEPLOYED_AS DEPLOYMENT` and
    /// [`ChainLinkKind::DeploymentToContract`] is `CONTRACT DEPLOYED_AS DEPLOYMENT`,
    /// the second of the taxonomy's two stated instances ("a contract address to the
    /// deployment record that created it"). Encoding the hops in chain order instead
    /// would assert `DEPLOYMENT DEPLOYED_AS CONTRACT`, which the taxonomy does not
    /// permit, and [`ChainLink::new`] refuses it.
    #[must_use]
    pub const fn subject_kind(self) -> EntityKind {
        match self {
            Self::SourceResolved => EntityKind::Source,
            Self::SourceToBuild => EntityKind::Build,
            Self::BuildToArtifact => EntityKind::Artifact,
            Self::ArtifactToWasm => EntityKind::Wasm,
            Self::WasmToDeployment => EntityKind::Wasm,
            Self::DeploymentToContract => EntityKind::Contract,
        }
    }

    /// The entity kind this stage's object is, where it has one.
    ///
    /// For both `DEPLOYED_AS` stages this is the deployment record; see
    /// [`ChainLinkKind::subject_kind`] for why the direction is the reverse of the
    /// chain's reading order.
    #[must_use]
    pub const fn object_kind(self) -> Option<EntityKind> {
        match self {
            Self::SourceResolved => None,
            Self::SourceToBuild => Some(EntityKind::Source),
            Self::BuildToArtifact => Some(EntityKind::Build),
            Self::ArtifactToWasm => Some(EntityKind::Artifact),
            Self::WasmToDeployment => Some(EntityKind::Deployment),
            Self::DeploymentToContract => Some(EntityKind::Deployment),
        }
    }

    /// The property of the chain this stage establishes, in a reader's words.
    #[must_use]
    pub const fn establishes(self) -> &'static str {
        match self {
            Self::SourceResolved => "the source revision identifies one immutable tree",
            Self::SourceToBuild => "the build read that source revision",
            Self::BuildToArtifact => "the build produced that artifact",
            Self::ArtifactToWasm => "that artifact is that module",
            Self::WasmToDeployment => "the deployment placed that executable",
            Self::DeploymentToContract => "that deployment produced this contract",
        }
    }
}

impl fmt::Display for ChainLinkKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for ChainLinkKind {
    type Err = EngineError;

    fn from_str(value: &str) -> Result<Self> {
        Self::all()
            .iter()
            .copied()
            .find(|candidate| candidate.as_str() == value)
            .ok_or_else(|| EngineError::Validation {
                path: "/chainLink".to_owned(),
                detail: format!("unrecognised chain link kind {value:?}"),
            })
    }
}

/// One stage of a chain, with how it was established.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChainLink {
    /// Which stage this is.
    pub kind: ChainLinkKind,
    /// The entity the stage is about.
    pub subject: EntityRef,
    /// The entity the stage was established from, where the stage is an edge.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub object: Option<EntityRef>,
    /// The basis the claim rests on.
    pub basis: Basis,
    /// The confidence reached, with the evidence that supports it.
    pub confidence: Confidence,
    /// What the evidence says about the claim.
    pub verification: amasario_core::VerificationStatus,
}

impl ChainLink {
    /// Records a stage.
    ///
    /// # Errors
    ///
    /// Returns a provenance error when the subject's kind is not the one the stage
    /// requires, or when the object is present and the stage's relationship does not
    /// permit the pair. The check happens here rather than at report time so that an
    /// impossible link cannot enter a chain and be discovered by a consumer.
    pub fn new(
        kind: ChainLinkKind,
        subject: EntityRef,
        object: Option<EntityRef>,
        basis: Basis,
        confidence: Confidence,
        verification: amasario_core::VerificationStatus,
    ) -> Result<Self> {
        if subject.kind != kind.subject_kind() {
            return Err(ProvenanceFailure::ImpossibleLink {
                relationship: kind.as_str().to_owned(),
                subject_kind: subject.kind.as_str().to_owned(),
                object_kind: kind.subject_kind().as_str().to_owned(),
            }
            .into_error());
        }
        match (kind.object_kind(), &object) {
            (None, Some(_)) => {
                return Err(ProvenanceFailure::ImpossibleLink {
                    relationship: kind.as_str().to_owned(),
                    subject_kind: subject.kind.as_str().to_owned(),
                    object_kind: "an entity".to_owned(),
                }
                .into_error());
            },
            (Some(expected), None) => {
                return Err(ProvenanceFailure::ImpossibleLink {
                    relationship: kind.as_str().to_owned(),
                    subject_kind: subject.kind.as_str().to_owned(),
                    object_kind: expected.as_str().to_owned(),
                }
                .into_error());
            },
            (Some(expected), Some(actual)) => {
                if actual.kind != expected {
                    return Err(ProvenanceFailure::ImpossibleLink {
                        relationship: kind.as_str().to_owned(),
                        subject_kind: subject.kind.as_str().to_owned(),
                        object_kind: actual.kind.as_str().to_owned(),
                    }
                    .into_error());
                }
                if let Some(relationship) = kind.relationship()
                    && !relationship.permits(subject.kind, actual.kind)
                {
                    return Err(ProvenanceFailure::ImpossibleLink {
                        relationship: relationship.as_str().to_owned(),
                        subject_kind: subject.kind.as_str().to_owned(),
                        object_kind: actual.kind.as_str().to_owned(),
                    }
                    .into_error());
                }
            },
            (None, None) => {},
        }
        Ok(Self {
            kind,
            subject,
            object,
            basis,
            confidence,
            verification,
        })
    }

    /// Whether this link's evidence contradicts its claim.
    #[must_use]
    pub const fn is_contradicted(&self) -> bool {
        self.verification.is_refutation() || self.confidence.is_contradicted()
    }

    /// Whether this link's claim was checked and not refuted.
    #[must_use]
    pub const fn is_affirmed(&self) -> bool {
        self.verification.is_affirmation()
    }
}

/// The provenance chain for one contract.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProvenanceChain {
    /// The contract the chain concludes at.
    pub contract: EntityRef,
    /// The links, in chain order.
    pub links: Vec<ChainLink>,
}

impl ProvenanceChain {
    /// A chain with no links yet.
    ///
    /// # Errors
    ///
    /// Returns a provenance error when the reference is not a contract. A chain
    /// concludes at a contract, and starting one at another kind would produce a
    /// sequence that cannot describe a contract's provenance.
    pub fn new(contract: EntityRef) -> Result<Self> {
        if contract.kind != EntityKind::Contract {
            return Err(ProvenanceFailure::ImpossibleLink {
                relationship: "the provenance chain".to_owned(),
                subject_kind: contract.kind.as_str().to_owned(),
                object_kind: EntityKind::Contract.as_str().to_owned(),
            }
            .into_error());
        }
        Ok(Self {
            contract,
            links: Vec::new(),
        })
    }

    /// Appends a link.
    ///
    /// # Errors
    ///
    /// Returns a provenance error when the link's stage precedes or repeats the last
    /// one appended. A chain whose links are out of order describes a provenance that
    /// cannot exist, and a completeness check would then be answering the wrong
    /// question.
    pub fn push(&mut self, link: ChainLink) -> Result<()> {
        if let Some(last) = self.links.last()
            && link.kind.position() <= last.kind.position()
        {
            return Err(EngineError::Validation {
                path: "/provenanceChain/links".to_owned(),
                detail: format!(
                    "the chain already reached {} and {link_kind} does not follow it; a chain's \
                     links describe one sequence",
                    last.kind,
                    link_kind = link.kind
                ),
            });
        }
        self.links.push(link);
        Ok(())
    }

    /// Appends a link, discarding a link that is already present.
    ///
    /// Used where a caller assembles a chain from independently collected facts and
    /// cannot guarantee the order. A repeated stage is dropped rather than replaced,
    /// so the first observation of a stage wins and a later, possibly weaker, one
    /// cannot overwrite it.
    pub fn push_if_absent(&mut self, link: ChainLink) {
        if self.link(link.kind).is_none() {
            self.links.push(link);
            self.links.sort_by_key(|existing| existing.kind.position());
        }
    }

    /// The link for a stage, when the chain has one.
    #[must_use]
    pub fn link(&self, kind: ChainLinkKind) -> Option<&ChainLink> {
        self.links.iter().find(|link| link.kind == kind)
    }

    /// The stages the chain has no link for.
    ///
    /// Every stage is reported, with no exemptions. A stage the engine could not
    /// establish is a gap whatever the reason, and a completeness check that excused
    /// one would be reporting a chain as stronger than its evidence.
    #[must_use]
    pub fn missing_links(&self) -> Vec<ChainLinkKind> {
        ChainLinkKind::all()
            .iter()
            .copied()
            .filter(|kind| self.link(*kind).is_none())
            .collect()
    }

    /// Whether the chain covers every stage.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.missing_links().is_empty()
    }

    /// Whether any link is contradicted by its evidence.
    #[must_use]
    pub fn has_contradiction(&self) -> bool {
        self.links.iter().any(ChainLink::is_contradicted)
    }

    /// The weakest confidence in the chain.
    ///
    /// A chain is no stronger than its weakest link, which is why aggregation is a
    /// minimum rather than an average: an average would let four strong links hide one
    /// unsupported one, and the chain would then read as better established than it
    /// is.
    #[must_use]
    pub fn weakest_confidence(&self) -> Option<amasario_core::ConfidenceLevel> {
        self.links
            .iter()
            .map(|link| link.confidence.level)
            .reduce(amasario_core::ConfidenceLevel::weakest)
    }
}

/// How two facts compare.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum MatchOutcome {
    /// The two agree.
    Match,
    /// The two disagree.
    Mismatch,
    /// The two cannot be compared at all.
    ///
    /// Reported separately from `Mismatch` because the difference is the difference
    /// between a contradiction, which makes a claim false, and an inability to check,
    /// which leaves it unknown. Two digests under different algorithms are the
    /// everyday case.
    Incomparable,
}

impl MatchOutcome {
    /// The stable wire name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Match => "MATCH",
            Self::Mismatch => "MISMATCH",
            Self::Incomparable => "INCOMPARABLE",
        }
    }

    /// Whether the comparison refutes the claim.
    #[must_use]
    pub const fn is_refutation(self) -> bool {
        matches!(self, Self::Mismatch)
    }

    /// Whether the comparison leaves the claim undetermined.
    #[must_use]
    pub const fn is_inconclusive(self) -> bool {
        matches!(self, Self::Incomparable)
    }
}

impl fmt::Display for MatchOutcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Compares two digests.
///
/// Digests under different algorithms are `Incomparable` rather than differing: a
/// SHA-256 and a SHA-512 of the same bytes are different values that are not
/// different *content*, and reporting them as a mismatch would turn an inability to
/// compare into a contradiction.
#[must_use]
pub fn match_digests(claimed: &Digest, computed: &Digest) -> MatchOutcome {
    if claimed.algorithm() != computed.algorithm() {
        return MatchOutcome::Incomparable;
    }
    if claimed.matches(computed) {
        MatchOutcome::Match
    } else {
        MatchOutcome::Mismatch
    }
}

/// Compares two revisions.
///
/// Two revisions agree when they identify the same commit. A revision that does not
/// identify a commit - an unresolved branch or tag - is `Incomparable`, because there
/// is nothing to compare: the record does not say which tree was built, so neither
/// agreement nor disagreement can be concluded.
#[must_use]
pub fn match_revisions(claimed: &Revision, rebuilt_from: &Revision) -> MatchOutcome {
    match (claimed.commit_id(), rebuilt_from.commit_id()) {
        (Some(left), Some(right)) => {
            if left == right {
                MatchOutcome::Match
            } else {
                MatchOutcome::Mismatch
            }
        },
        _ => MatchOutcome::Incomparable,
    }
}

/// Whether a source revision's rebuild produced the module the contract records.
///
/// The question the specification names explicitly, and the one case where an
/// incorrect `VERIFIED` is most damaging. It is answered here rather than at a call
/// site so that the three-valued outcome cannot be collapsed into a boolean on the
/// way.
#[must_use]
pub fn match_rebuilt_module(recorded: &Digest, rebuilt: &Digest) -> MatchOutcome {
    match_digests(recorded, rebuilt)
}

#[cfg(test)]
mod tests {
    use super::*;
    use amasario_core::{ConfidenceLevel, VerificationStatus};

    fn digest(seed: u8) -> Digest {
        Digest::sha256_of(&[seed])
    }

    fn confidence() -> Confidence {
        Confidence::new(ConfidenceLevel::Verified, vec!["e".to_owned()], Vec::new())
            .expect("a confidence with evidence")
    }

    fn source_ref() -> EntityRef {
        EntityRef::new(EntityKind::Source, "source-1").expect("a reference")
    }

    fn link(kind: ChainLinkKind, subject: EntityRef, object: Option<EntityRef>) -> ChainLink {
        ChainLink::new(
            kind,
            subject,
            object,
            Basis::DeclaredManifest,
            confidence(),
            VerificationStatus::Verified,
        )
        .expect("a permitted link")
    }

    fn full_chain() -> ProvenanceChain {
        let mut chain =
            ProvenanceChain::new(EntityRef::new(EntityKind::Contract, "C-x").expect("a reference"))
                .expect("a contract concludes a chain");
        chain
            .push(link(ChainLinkKind::SourceResolved, source_ref(), None))
            .expect("the first stage");
        chain
            .push(link(
                ChainLinkKind::SourceToBuild,
                EntityRef::new(EntityKind::Build, "build-1").expect("a reference"),
                Some(source_ref()),
            ))
            .expect("in order");
        chain
            .push(link(
                ChainLinkKind::BuildToArtifact,
                EntityRef::new(EntityKind::Artifact, "artifact-1").expect("a reference"),
                Some(EntityRef::new(EntityKind::Build, "build-1").expect("a reference")),
            ))
            .expect("in order");
        chain
            .push(link(
                ChainLinkKind::ArtifactToWasm,
                EntityRef::new(EntityKind::Wasm, "wasm-1").expect("a reference"),
                Some(EntityRef::new(EntityKind::Artifact, "artifact-1").expect("a reference")),
            ))
            .expect("in order");
        chain
            .push(link(
                ChainLinkKind::WasmToDeployment,
                wasm_ref(),
                Some(deployment_ref()),
            ))
            .expect("in order");
        chain
            .push(link(
                ChainLinkKind::DeploymentToContract,
                EntityRef::new(EntityKind::Contract, "C-x").expect("a reference"),
                Some(deployment_ref()),
            ))
            .expect("in order");
        chain
    }

    fn wasm_ref() -> EntityRef {
        EntityRef::new(EntityKind::Wasm, "wasm-1").expect("a reference")
    }

    fn deployment_ref() -> EntityRef {
        EntityRef::new(EntityKind::Deployment, "deployment-1").expect("a reference")
    }

    #[test]
    fn the_deployed_as_stages_run_from_the_deployed_entity_to_the_deployment_record() {
        // The direction is the taxonomy's, and it is the reverse of the chain's
        // reading order. Asserted here so that a later change cannot quietly flip it
        // into an edge the specification does not permit.
        assert_eq!(
            ChainLinkKind::WasmToDeployment.subject_kind(),
            EntityKind::Wasm
        );
        assert_eq!(
            ChainLinkKind::WasmToDeployment.object_kind(),
            Some(EntityKind::Deployment)
        );
        assert_eq!(
            ChainLinkKind::DeploymentToContract.subject_kind(),
            EntityKind::Contract
        );
        assert_eq!(
            ChainLinkKind::DeploymentToContract.object_kind(),
            Some(EntityKind::Deployment)
        );
        // The reason: DEPLOYMENT is not a DEPLOYED_AS subject, so the hop from the
        // deployment record to the contract is expressible only as the reverse edge.
        assert!(!Relationship::DeployedAs.permits(EntityKind::Deployment, EntityKind::Contract));
        assert!(Relationship::DeployedAs.permits(EntityKind::Contract, EntityKind::Deployment));
        assert!(Relationship::DeployedAs.permits(EntityKind::Wasm, EntityKind::Deployment));
    }

    #[test]
    fn chain_stages_are_ordered_as_the_specification_states_the_chain() {
        for (index, kind) in ChainLinkKind::all().iter().enumerate() {
            assert_eq!(kind.position(), index);
            assert_eq!(
                ChainLinkKind::from_str(kind.as_str()).expect("round trip"),
                *kind
            );
            assert!(!kind.establishes().is_empty());
        }
        assert_eq!(ChainLinkKind::all().len(), 6);
        ChainLinkKind::from_str("SOURCE_TO_LEDGER").expect_err("an unknown stage is rejected");
    }

    #[test]
    fn every_stage_but_the_first_is_realised_by_a_permitted_relationship() {
        let source = source_ref();
        for kind in ChainLinkKind::all() {
            let Some(relationship) = kind.relationship() else {
                // A property of a record rather than an edge, and the chain must not
                // claim an edge that does not exist.
                assert_eq!(*kind, ChainLinkKind::SourceResolved);
                assert!(kind.object_kind().is_none());
                continue;
            };
            let subject = EntityRef::new(kind.subject_kind(), "s").expect("a reference");
            let object = EntityRef::new(kind.object_kind().expect("an object kind"), "o")
                .expect("a reference");
            assert!(
                relationship.permits(subject.kind, object.kind),
                "{kind} uses {relationship} which does not permit {} -> {}",
                subject.kind,
                object.kind
            );
        }
        assert!(source.kind == EntityKind::Source);
    }

    #[test]
    fn a_link_with_the_wrong_subject_kind_is_rejected() {
        // An impossible link must not be able to enter a chain and be discovered by a
        // consumer.
        let error = ChainLink::new(
            ChainLinkKind::SourceToBuild,
            EntityRef::new(EntityKind::Contract, "C-x").expect("a reference"),
            Some(source_ref()),
            Basis::DeclaredManifest,
            confidence(),
            VerificationStatus::Verified,
        )
        .expect_err("a contract is not a build");
        assert!(error.to_string().contains("cannot connect"));
    }

    #[test]
    fn a_link_whose_object_is_the_wrong_kind_is_rejected() {
        ChainLink::new(
            ChainLinkKind::SourceToBuild,
            EntityRef::new(EntityKind::Build, "build-1").expect("a reference"),
            Some(EntityRef::new(EntityKind::Artifact, "artifact-1").expect("a reference")),
            Basis::DeclaredManifest,
            confidence(),
            VerificationStatus::Verified,
        )
        .expect_err("a build is built from a source, not an artifact");
    }

    #[test]
    fn a_property_stage_may_not_carry_an_object() {
        // SourceResolved is a property of the source record, and an object on it
        // would be an edge the chain cannot justify.
        ChainLink::new(
            ChainLinkKind::SourceResolved,
            source_ref(),
            Some(EntityRef::new(EntityKind::Source, "other").expect("a reference")),
            Basis::DeclaredManifest,
            confidence(),
            VerificationStatus::Verified,
        )
        .expect_err("a property has no second entity");
    }

    #[test]
    fn a_chain_concludes_at_a_contract() {
        ProvenanceChain::new(EntityRef::new(EntityKind::Contract, "C-x").expect("a reference"))
            .expect("a contract");
        let error = ProvenanceChain::new(
            EntityRef::new(EntityKind::Artifact, "artifact-1").expect("a reference"),
        )
        .expect_err("an artifact is not a contract");
        assert!(error.to_string().contains("cannot connect"));
    }

    #[test]
    fn links_may_only_be_appended_in_chain_order() {
        // A chain whose links are out of order describes a provenance that cannot
        // exist.
        let mut chain =
            ProvenanceChain::new(EntityRef::new(EntityKind::Contract, "C-x").expect("a reference"))
                .expect("a contract");
        chain
            .push(link(
                ChainLinkKind::BuildToArtifact,
                EntityRef::new(EntityKind::Artifact, "artifact-1").expect("a reference"),
                Some(EntityRef::new(EntityKind::Build, "build-1").expect("a reference")),
            ))
            .expect("the first stage");

        let error = chain
            .push(link(ChainLinkKind::SourceResolved, source_ref(), None))
            .expect_err("the source stage cannot follow the artifact stage");
        assert!(
            error.to_string().contains("does not follow"),
            "got: {error}"
        );

        chain
            .push(link(
                ChainLinkKind::ArtifactToWasm,
                EntityRef::new(EntityKind::Wasm, "wasm-1").expect("a reference"),
                Some(EntityRef::new(EntityKind::Artifact, "artifact-1").expect("a reference")),
            ))
            .expect("the next stage in order");

        chain
            .push(link(
                ChainLinkKind::BuildToArtifact,
                EntityRef::new(EntityKind::Artifact, "artifact-1").expect("a reference"),
                Some(EntityRef::new(EntityKind::Build, "build-1").expect("a reference")),
            ))
            .expect_err("a repeated stage is refused");
    }

    #[test]
    fn a_full_chain_reports_every_stage_present_and_no_gap() {
        let chain = full_chain();
        assert!(chain.is_complete());
        assert!(chain.missing_links().is_empty());
        assert_eq!(chain.links.len(), 6);
        assert!(!chain.has_contradiction());
        assert_eq!(chain.weakest_confidence(), Some(ConfidenceLevel::Verified));
        assert!(chain.link(ChainLinkKind::WasmToDeployment).is_some());
        assert!(chain.link(ChainLinkKind::SourceResolved).is_some());
    }

    #[test]
    fn a_chain_missing_stages_names_each_gap() {
        let mut chain =
            ProvenanceChain::new(EntityRef::new(EntityKind::Contract, "C-x").expect("a reference"))
                .expect("a contract");
        chain
            .push(link(ChainLinkKind::SourceResolved, source_ref(), None))
            .expect("the first stage");

        let missing = chain.missing_links();
        assert!(missing.contains(&ChainLinkKind::SourceToBuild));
        assert!(missing.contains(&ChainLinkKind::DeploymentToContract));
        assert!(!chain.is_complete());
    }

    #[test]
    fn a_stage_the_engine_could_not_establish_is_a_gap_whatever_the_reason() {
        // The conservative reading, and the one the specification's "no overstatement"
        // requirement forces: an absent link is reported as a gap rather than being
        // excused, because a completeness check that excused one would present a chain
        // as stronger than its evidence.
        let mut chain =
            ProvenanceChain::new(EntityRef::new(EntityKind::Contract, "C-x").expect("a reference"))
                .expect("a contract");
        chain
            .push(link(
                ChainLinkKind::BuildToArtifact,
                EntityRef::new(EntityKind::Artifact, "wasm-1").expect("a reference"),
                Some(EntityRef::new(EntityKind::Build, "build-1").expect("a reference")),
            ))
            .expect("a build produced the module");
        assert!(
            chain
                .missing_links()
                .contains(&ChainLinkKind::ArtifactToWasm)
        );
        assert!(!chain.is_complete());
    }

    #[test]
    fn a_contradicted_link_is_detected_from_either_its_status_or_its_confidence() {
        let mut chain = full_chain();
        assert!(!chain.has_contradiction());

        chain.links[2].verification = VerificationStatus::Conflicting;
        assert!(chain.has_contradiction());

        let mut other = full_chain();
        other.links[2].confidence = Confidence::new(
            ConfidenceLevel::LowConfidence,
            vec!["e".to_owned()],
            vec!["counter".to_owned()],
        )
        .expect("valid");
        assert!(other.has_contradiction());
        assert!(other.links[2].is_contradicted());
        assert!(!other.links[0].is_contradicted());
    }

    #[test]
    fn the_chains_confidence_is_its_weakest_link() {
        // A chain is no stronger than its weakest link: an average would let four
        // strong links hide one unsupported one.
        let mut chain = full_chain();
        chain.links[1].confidence = Confidence::new(
            ConfidenceLevel::LowConfidence,
            vec!["e".to_owned()],
            Vec::new(),
        )
        .expect("valid");
        assert_eq!(
            chain.weakest_confidence(),
            Some(ConfidenceLevel::LowConfidence)
        );

        let mut empty =
            ProvenanceChain::new(EntityRef::new(EntityKind::Contract, "C-x").expect("a reference"))
                .expect("a contract");
        assert_eq!(empty.weakest_confidence(), None);
        empty.push_if_absent(link(ChainLinkKind::SourceResolved, source_ref(), None));
        assert_eq!(empty.weakest_confidence(), Some(ConfidenceLevel::Verified));
    }

    #[test]
    fn push_if_absent_orders_the_links_and_keeps_the_first_observation() {
        // A later, possibly weaker, observation must not overwrite an earlier one.
        let mut chain =
            ProvenanceChain::new(EntityRef::new(EntityKind::Contract, "C-x").expect("a reference"))
                .expect("a contract");
        let mut second = link(
            ChainLinkKind::WasmToDeployment,
            wasm_ref(),
            Some(deployment_ref()),
        );
        second.verification = VerificationStatus::Unverified;
        chain.push_if_absent(second);

        let mut first = link(
            ChainLinkKind::SourceResolved,
            EntityRef::new(EntityKind::Source, "source-1").expect("a reference"),
            None,
        );
        first.basis = Basis::ObservedInvocation;
        chain.push_if_absent(first);

        assert_eq!(chain.links[0].kind, ChainLinkKind::SourceResolved);
        assert_eq!(chain.links[1].kind, ChainLinkKind::WasmToDeployment);

        // A repeat of a stage already present is dropped.
        chain.push_if_absent(link(
            ChainLinkKind::WasmToDeployment,
            wasm_ref(),
            Some(deployment_ref()),
        ));
        assert_eq!(chain.links.len(), 2);
        assert_eq!(
            chain
                .link(ChainLinkKind::WasmToDeployment)
                .map(|found| found.verification),
            Some(VerificationStatus::Unverified),
            "the first observation wins"
        );
    }

    #[test]
    fn digests_under_different_algorithms_are_incomparable_rather_than_different() {
        // A SHA-256 and a SHA-512 of the same bytes are different values that are not
        // different content, and reporting them as a mismatch would turn an inability
        // to compare into a contradiction.
        let sha256 = Digest::sha256_of(b"same bytes");
        let sha512 = Digest::new(amasario_core::DigestAlgorithm::Sha512, &"a".repeat(128))
            .expect("a valid digest");

        assert_eq!(match_digests(&sha256, &sha512), MatchOutcome::Incomparable);
        assert!(match_digests(&sha256, &sha512).is_inconclusive());
        assert!(!match_digests(&sha256, &sha512).is_refutation());
        assert_eq!(MatchOutcome::Incomparable.as_str(), "INCOMPARABLE");
    }

    #[test]
    fn digests_of_the_same_content_match_and_different_content_does_not() {
        assert_eq!(match_digests(&digest(1), &digest(1)), MatchOutcome::Match);
        assert_eq!(
            match_digests(&digest(1), &digest(2)),
            MatchOutcome::Mismatch
        );
        assert!(match_digests(&digest(1), &digest(2)).is_refutation());
    }

    #[test]
    fn an_unresolved_revision_is_incomparable_rather_than_different() {
        // The record does not say which tree was built, so neither agreement nor
        // disagreement can be concluded.
        let branch = Revision::branch("main").expect("valid");
        let commit = Revision::commit("9f2c1e0a3b4c5d6e7f8091a2b3c4d5e6f7081920").expect("valid");

        assert_eq!(
            match_revisions(&branch, &commit),
            MatchOutcome::Incomparable
        );
        assert_eq!(
            match_revisions(&commit, &branch),
            MatchOutcome::Incomparable
        );

        let resolved = Revision::branch("main")
            .expect("valid")
            .resolved_to("9f2c1e0a3b4c5d6e7f8091a2b3c4d5e6f7081920")
            .expect("valid");
        assert_eq!(match_revisions(&resolved, &commit), MatchOutcome::Match);

        let other = Revision::commit("a".repeat(40)).expect("valid");
        assert_eq!(match_revisions(&other, &commit), MatchOutcome::Mismatch);
    }

    #[test]
    fn a_rebuild_that_produced_a_different_module_is_a_mismatch() {
        // The case the specification names explicitly, and the one where an incorrect
        // VERIFIED is most damaging.
        assert_eq!(
            match_rebuilt_module(&digest(1), &digest(1)),
            MatchOutcome::Match
        );
        assert_eq!(
            match_rebuilt_module(&digest(1), &digest(9)),
            MatchOutcome::Mismatch
        );
    }

    #[test]
    fn a_chain_round_trips_through_json() {
        let original = full_chain();
        let json = serde_json::to_string(&original).expect("serialises");
        let restored: ProvenanceChain = serde_json::from_str(&json).expect("deserialises");
        assert_eq!(restored, original);
        assert!(restored.is_complete());
    }

    #[test]
    fn a_chain_serialises_deterministically() {
        let first = full_chain();
        let second = full_chain();
        assert_eq!(
            serde_json::to_string(&first).expect("serialises"),
            serde_json::to_string(&second).expect("serialises")
        );
    }
}
