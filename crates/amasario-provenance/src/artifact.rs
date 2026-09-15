//! Artifact identity: what an artifact is, what produced it, and what it came from.
//!
//! # Why an artifact is not its filename
//!
//! The specification's artifact model supports source archives, lockfiles, build
//! artifacts, WASM modules and deployment artifacts, and the only property that
//! identifies any of them is its digest. A filename is a build tool's choice, a size
//! is a consequence, and a path is a machine's layout; none of them survive a
//! rebuild on another machine, so none of them can identify content. Every type here
//! therefore keys on a [`Digest`] and treats the rest as description.
//!
//! # The parent relationship is one-directional
//!
//! An artifact records what it was derived from, never what was derived from it.
//! The reverse would be a second copy of the same fact, and two copies of an edge
//! are two places for it to be wrong. `DERIVED_FROM` is the relationship that carries
//! it, and its direction is object-to-subject: a change to the parent affects the
//! child, so the child is the subject.
//!
//! # Why the algorithm is part of the identity
//!
//! A bare 64-character hex string is equally consistent with a SHA-256 and a
//! truncated SHA-512, and the two are not comparable. `amasario-core`'s [`Digest`]
//! carries the algorithm, and this module refuses to accept a digest in any other
//! form, so two implementations cannot produce artifacts whose digests look alike and
//! mean different things.

use std::fmt;
use std::str::FromStr;

use amasario_core::{Digest, EngineError, EntityKind, EntityRef, Result};
use serde::{Deserialize, Serialize};

/// The kinds of artifact the specification's taxonomy defines.
///
/// Closed. An artifact whose kind is not recognisable cannot be placed in the
/// chain - the engine would not know whether it belongs between a source and a
/// build or between a build and a deployment - so it is rejected rather than
/// recorded as an unknown kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
#[non_exhaustive]
pub enum ArtifactType {
    /// A retrieved archive of a source tree.
    SourceArchive,
    /// A dependency lockfile, which pins resolved versions.
    Lockfile,
    /// The output of a build, before deployment.
    BuildArtifact,
    /// A WebAssembly module.
    Wasm,
    /// The artifact as it appears on a ledger.
    DeploymentArtifact,
}

impl ArtifactType {
    /// The stable wire name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SourceArchive => "SOURCE_ARCHIVE",
            Self::Lockfile => "LOCKFILE",
            Self::BuildArtifact => "BUILD_ARTIFACT",
            Self::Wasm => "WASM",
            Self::DeploymentArtifact => "DEPLOYMENT_ARTIFACT",
        }
    }

    /// Every kind, in the canonical order used for deterministic output.
    #[must_use]
    pub const fn all() -> &'static [Self] {
        &[
            Self::SourceArchive,
            Self::Lockfile,
            Self::BuildArtifact,
            Self::Wasm,
            Self::DeploymentArtifact,
        ]
    }

    /// Whether this kind is a compiled module.
    ///
    /// Used where the engine needs to know whether a digest can be compared against
    /// a contract's recorded executable hash, which is a question about WASM
    /// specifically rather than about artifacts generally.
    #[must_use]
    pub const fn is_executable(self) -> bool {
        matches!(self, Self::Wasm)
    }

    /// Whether this kind can be produced by a build.
    ///
    /// A source archive and a lockfile are retrieved rather than built, so
    /// attributing them to a build would invent a step that did not happen.
    #[must_use]
    pub const fn is_build_output(self) -> bool {
        matches!(self, Self::BuildArtifact | Self::Wasm)
    }
}

impl fmt::Display for ArtifactType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for ArtifactType {
    type Err = EngineError;

    fn from_str(value: &str) -> Result<Self> {
        Self::all()
            .iter()
            .copied()
            .find(|candidate| candidate.as_str() == value)
            .ok_or_else(|| EngineError::Validation {
                path: "/artifactType".to_owned(),
                detail: format!("unrecognised artifact type {value:?}"),
            })
    }
}

/// What an artifact was mechanically produced from.
///
/// Recorded as a reference plus the relationship's basis, because "this archive was
/// derived from that revision" is a claim the engine must be able to justify. A bare
/// parent with no basis would be an assertion the report could not explain, and a
/// report that cannot explain why an edge exists is one a reader has to trust.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DerivationSource {
    /// The entity the artifact was derived from.
    pub parent: EntityRef,
    /// The relationship that says so, which is always `DERIVED_FROM` or `BUILT_FROM`.
    pub relationship: String,
    /// Identifiers of the evidence records that support the derivation.
    pub evidence: Vec<String>,
}

impl DerivationSource {
    /// Records a derivation.
    ///
    /// # Errors
    ///
    /// Returns a validation error when no evidence is cited, and a provenance error
    /// when the relationship is neither `DERIVED_FROM` nor `BUILT_FROM`. Those two
    /// are the only relationships whose semantics mean "was produced from", and
    /// accepting others would let a dependency be recorded as a derivation.
    pub fn new(parent: EntityRef, relationship: &str, evidence: Vec<String>) -> Result<Self> {
        if !matches!(relationship, "DERIVED_FROM" | "BUILT_FROM") {
            return Err(EngineError::Validation {
                path: "/artifact/derivedFrom/relationship".to_owned(),
                detail: format!(
                    "{relationship} does not mean \"was produced from\"; only DERIVED_FROM and \
                     BUILT_FROM do"
                ),
            });
        }
        if evidence.is_empty() {
            return Err(EngineError::Validation {
                path: "/artifact/derivedFrom/evidence".to_owned(),
                detail: "a derivation must cite the evidence that supports it".to_owned(),
            });
        }
        Ok(Self {
            parent,
            relationship: relationship.to_owned(),
            evidence,
        })
    }
}

/// An artifact's identity, with the description that travels with it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ArtifactIdentity {
    /// The artifact's content digest. This is its identity.
    pub digest: Digest,
    /// The artifact's kind.
    pub artifact_type: ArtifactType,
    /// The artifact's size in bytes, when measured.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub byte_size: Option<u64>,
    /// The artifact's filename as produced, when recorded.
    ///
    /// Description only. Present because a reader looking for a downloaded file
    /// benefits from seeing the name it had, and explicitly documented as not an
    /// identity so that nothing compares two artifacts by name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_name: Option<String>,
    /// What the artifact was derived from, when that is known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub derived_from: Option<DerivationSource>,
    /// The entity that produced the artifact, when a build record exists.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub produced_by: Option<EntityRef>,
    /// Identifiers of the evidence records that support this identity.
    pub evidence: Vec<String>,
}

impl ArtifactIdentity {
    /// Records an artifact's identity.
    ///
    /// # Errors
    ///
    /// Returns a validation error when no evidence is cited, because an artifact
    /// identity with nothing behind it asserts that a digest was observed without
    /// naming what observed it.
    pub fn new(digest: Digest, artifact_type: ArtifactType, evidence: Vec<String>) -> Result<Self> {
        if evidence.is_empty() {
            return Err(EngineError::Validation {
                path: "/artifact/evidence".to_owned(),
                detail: "an artifact identity must cite the evidence that observed its digest"
                    .to_owned(),
            });
        }
        Ok(Self {
            digest,
            artifact_type,
            byte_size: None,
            file_name: None,
            derived_from: None,
            produced_by: None,
            evidence,
        })
    }

    /// Records the measured size.
    #[must_use]
    pub const fn with_size(mut self, byte_size: u64) -> Self {
        self.byte_size = Some(byte_size);
        self
    }

    /// Records the filename the artifact was produced under.
    #[must_use]
    pub fn with_file_name(mut self, name: impl Into<String>) -> Self {
        self.file_name = Some(name.into());
        self
    }

    /// Records what the artifact was derived from.
    #[must_use]
    pub fn derived_from(mut self, source: DerivationSource) -> Self {
        self.derived_from = Some(source);
        self
    }

    /// Records the build that produced the artifact.
    ///
    /// # Errors
    ///
    /// Returns a provenance error when the entity is not a build, since an artifact
    /// is produced by a build and attributing it to anything else would create an
    /// edge the relationship's semantics do not permit.
    pub fn produced_by(mut self, producer: EntityRef) -> Result<Self> {
        if producer.kind != EntityKind::Build {
            return Err(crate::errors::ProvenanceFailure::ImpossibleLink {
                relationship: "BUILT_BY".to_owned(),
                subject_kind: EntityKind::Artifact.as_str().to_owned(),
                object_kind: producer.kind.as_str().to_owned(),
            }
            .into_error());
        }
        self.produced_by = Some(producer);
        Ok(self)
    }

    /// A reference to this artifact as an entity.
    ///
    /// Artifacts are identified by their digest, matching the specification's
    /// decision that an artifact's name is not its identity.
    #[must_use]
    pub fn as_ref(&self) -> EntityRef {
        EntityRef {
            kind: EntityKind::Artifact,
            id: self.digest.value().to_owned(),
        }
    }

    /// Whether this artifact's digest matches another's.
    #[must_use]
    pub fn matches(&self, other: &Self) -> bool {
        self.digest.matches(&other.digest)
    }

    /// Whether the artifact's kind and its recorded derivation are consistent.
    ///
    /// # Errors
    ///
    /// Returns a validation error when no evidence is cited, and a provenance error
    /// when the kind is not a build output but a build is recorded as its producer -
    /// which would attribute a retrieved file to a build that did not make it.
    pub fn validate(&self) -> Result<()> {
        if self.evidence.is_empty() {
            return Err(EngineError::Validation {
                path: "/artifact/evidence".to_owned(),
                detail: "an artifact identity must cite the evidence that observed its digest"
                    .to_owned(),
            });
        }
        if self.produced_by.is_some() && !self.artifact_type.is_build_output() {
            return Err(EngineError::Validation {
                path: "/artifact/producedBy".to_owned(),
                detail: format!(
                    "a {} is retrieved rather than built, so it cannot have a build as its \
                     producer",
                    self.artifact_type
                ),
            });
        }
        Ok(())
    }
}

/// Whether two digests identify the same content, as a verification-ready answer.
///
/// Returns the pair rather than a boolean because the caller almost always needs to
/// report both values, and recomputing them at the report site is how the two come to
/// disagree with the comparison that was actually made.
#[must_use]
pub fn compare_digests(claimed: &Digest, computed: &Digest) -> (bool, String, String) {
    (
        claimed.matches(computed),
        claimed.value().to_owned(),
        computed.value().to_owned(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn digest(seed: u8) -> Digest {
        Digest::sha256_of(&[seed])
    }

    fn wasm(seed: u8) -> ArtifactIdentity {
        ArtifactIdentity::new(digest(seed), ArtifactType::Wasm, vec!["e".to_owned()])
            .expect("a valid identity")
    }

    #[test]
    fn artifact_types_round_trip_through_their_wire_names() {
        for kind in ArtifactType::all() {
            assert_eq!(
                ArtifactType::from_str(kind.as_str()).expect("round trip"),
                *kind
            );
        }
        assert_eq!(ArtifactType::all().len(), 5);
        ArtifactType::from_str("TARBALL").expect_err("an unknown kind is rejected");
    }

    #[test]
    fn only_a_wasm_artifact_is_an_executable() {
        assert!(ArtifactType::Wasm.is_executable());
        assert!(!ArtifactType::SourceArchive.is_executable());
        assert!(!ArtifactType::Lockfile.is_executable());
        assert!(!ArtifactType::DeploymentArtifact.is_executable());
    }

    #[test]
    fn a_retrieved_artifact_is_not_a_build_output() {
        assert!(ArtifactType::BuildArtifact.is_build_output());
        assert!(ArtifactType::Wasm.is_build_output());
        assert!(!ArtifactType::SourceArchive.is_build_output());
        assert!(!ArtifactType::Lockfile.is_build_output());
        assert!(!ArtifactType::DeploymentArtifact.is_build_output());
    }

    #[test]
    fn an_artifact_identity_must_cite_evidence() {
        let error = ArtifactIdentity::new(digest(1), ArtifactType::Wasm, Vec::new())
            .expect_err("no evidence observes the digest");
        assert!(error.to_string().contains("must cite the evidence"));
    }

    #[test]
    fn artifacts_are_identified_by_their_digest_not_their_name() {
        // A filename is a build tool's choice and does not survive a rebuild.
        let first = wasm(1).with_file_name("token.wasm");
        let second = wasm(1).with_file_name("token-optimized.wasm");
        assert!(first.matches(&second));
        assert_eq!(first.as_ref(), second.as_ref());
        assert_eq!(first.as_ref().kind, EntityKind::Artifact);

        let third = wasm(2);
        assert!(!first.matches(&third));
        assert_ne!(first.as_ref(), third.as_ref());
    }

    #[test]
    fn a_derivation_must_use_a_relationship_that_means_produced_from() {
        let parent = EntityRef::new(EntityKind::Source, "source-1").expect("a reference");
        DerivationSource::new(parent.clone(), "DERIVED_FROM", vec!["e".to_owned()])
            .expect("a derivation");
        DerivationSource::new(parent.clone(), "BUILT_FROM", vec!["e".to_owned()]).expect("a build");

        let error = DerivationSource::new(parent, "DEPENDS_ON", vec!["e".to_owned()])
            .expect_err("a dependency is not a derivation");
        assert!(
            error.to_string().contains("does not mean"),
            "the error must say why: {error}"
        );
    }

    #[test]
    fn a_derivation_must_cite_evidence() {
        let parent = EntityRef::new(EntityKind::Source, "source-1").expect("a reference");
        DerivationSource::new(parent, "DERIVED_FROM", Vec::new()).expect_err("no evidence");
    }

    #[test]
    fn an_artifact_can_only_be_produced_by_a_build() {
        // Attributing a digest to anything but a build would create an edge the
        // relationship's semantics do not permit.
        let build = EntityRef::new(EntityKind::Build, "build-1").expect("a reference");
        let resolved = wasm(1)
            .produced_by(build)
            .expect("a build may produce an artifact");
        assert_eq!(
            resolved.produced_by.as_ref().map(|entity| entity.kind),
            Some(EntityKind::Build)
        );

        let source = EntityRef::new(EntityKind::Source, "source-1").expect("a reference");
        let error = wasm(1)
            .produced_by(source)
            .expect_err("a source does not produce an artifact");
        assert!(error.to_string().contains("cannot connect"));
    }

    #[test]
    fn a_retrieved_artifact_cannot_be_attributed_to_a_build() {
        // A source archive is retrieved rather than built, so a build attributed to
        // it would be a step that did not happen.
        let build = EntityRef::new(EntityKind::Build, "build-1").expect("a reference");
        let mut archive =
            ArtifactIdentity::new(digest(3), ArtifactType::SourceArchive, vec!["e".to_owned()])
                .expect("a valid identity");
        archive.produced_by = Some(build);

        let error = archive
            .validate()
            .expect_err("a retrieved file has no builder");
        assert!(error.to_string().contains("retrieved rather than built"));
    }

    #[test]
    fn a_deserialised_artifact_without_evidence_is_caught_by_validation() {
        let mut artifact = wasm(1);
        artifact.validate().expect("self-consistent");
        artifact.evidence.clear();
        artifact
            .validate()
            .expect_err("not a record without evidence");
    }

    #[test]
    fn comparing_digests_reports_both_values_alongside_the_answer() {
        // The caller almost always needs both, and recomputing them at the report
        // site is how the two come to disagree with the comparison that was made.
        let (matches, claimed, computed) = compare_digests(&digest(1), &digest(1));
        assert!(matches);
        assert_eq!(claimed, digest(1).value());
        assert_eq!(computed, digest(1).value());

        let (matches, claimed, computed) = compare_digests(&digest(1), &digest(2));
        assert!(!matches);
        assert_ne!(claimed, computed);
    }

    #[test]
    fn an_artifact_round_trips_through_json() {
        let original = wasm(1)
            .with_size(4096)
            .with_file_name("token.wasm")
            .derived_from(
                DerivationSource::new(
                    EntityRef::new(EntityKind::Build, "build-1").expect("a reference"),
                    "BUILT_FROM",
                    vec!["e".to_owned()],
                )
                .expect("a build"),
            );
        let json = serde_json::to_string(&original).expect("serialises");
        let restored: ArtifactIdentity = serde_json::from_str(&json).expect("deserialises");
        assert_eq!(restored, original);
        restored.validate().expect("self-consistent");
    }
}
