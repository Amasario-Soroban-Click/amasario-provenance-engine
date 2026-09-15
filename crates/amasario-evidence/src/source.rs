//! Source and build evidence: where an artifact came from, and what produced it.
//!
//! # Why the two classes are built together
//!
//! `rules/provenance/source-to-build.yaml` treats the pair as one hop of one chain: a
//! revision was read, and a build consumed it. The evidence for the hop is two records,
//! and building them apart invites the mistake the taxonomy warns about twice - a source
//! record traceable "to the repository and the exact revision, not merely to the
//! repository", and a build record traceable "to the build record and the inputs it
//! declares". Both clauses are about the same failure: a record that names a repository or
//! a toolchain without pinning the revision or the configuration cannot be checked again,
//! and "cannot be checked again" is what makes `MEDIUM_CONFIDENCE` rather than
//! `VERIFIED` an honest rating.
//!
//! # What the toolchain identity is
//!
//! [`toolchain_identity`] joins a [`Toolchain`]'s name, version and source into one
//! string, because the evidence schema has one `toolchain` field and the three parts are
//! meaningless apart: `rustc` alone names no compiler, and `1.93.0` alone names nothing.
//! The three are joined rather than digested so that a reader of the evidence document can
//! see what was claimed, which is the whole purpose of an evidence record.

use amasario_core::{Digest, Result};
use amasario_provenance::{BuildProvenance, SourceProvenance, Toolchain};

use crate::collector::{EvidenceClass, EvidenceRecord};

/// The `traceableTo` a source record has to satisfy, quoted for the message a producer
/// reads when it does not.
const SOURCE_TRACE: &str = "the repository and the exact revision, not merely the repository";

/// A record for a source repository at a revision.
///
/// The revision's kind is copied from the [`amasario_provenance::Revision`] rather than
/// inferred from its text, so a caller that knows a tag is a tag does not have it recorded
/// as a branch, and a caller that does not know still gets the conservative inference.
///
/// # Errors
///
/// Returns a provenance error when the record does not satisfy the `SOURCE` class. The
/// case worth naming is a mutable revision: it is a valid record, and the same condition
/// also rates it `MEDIUM_CONFIDENCE`, because a branch denotes different trees at
/// different times.
pub fn from_source(
    provenance: &SourceProvenance,
    id: impl Into<String>,
    observed_at: impl Into<String>,
) -> Result<EvidenceRecord> {
    let mut record = EvidenceRecord::draft(
        id,
        EvidenceClass::parse("SOURCE"),
        format!(
            "the source was read from {} at revision {}",
            provenance.repository.url, provenance.revision.value
        ),
        observed_at,
    );
    record.repository = Some(provenance.repository.url.clone());
    record.revision = Some(provenance.revision.value.clone());
    record.revision_kind = Some(provenance.revision.kind);
    record.digest = provenance.archive_digest.clone();
    record.claim = format!(
        "the source at {} revision {} is the tree the artifact was built from ({SOURCE_TRACE})",
        provenance.repository.url, provenance.revision.value
    );
    record.validate()?;
    Ok(record)
}

/// The toolchain as one identity string.
///
/// Joined rather than hashed, because an evidence record exists so that a reader can see
/// what was claimed: a digest of a toolchain identity would be checkable and unreadable,
/// and the claim it supports is "this compiler at this version produced this artifact".
#[must_use]
pub fn toolchain_identity(toolchain: &Toolchain) -> String {
    match &toolchain.source {
        Some(source) => format!("{} {} ({source})", toolchain.name, toolchain.version),
        None => format!("{} {}", toolchain.name, toolchain.version),
    }
}

/// A record for a recorded build.
///
/// The configuration digest is optional on [`BuildProvenance`] and required by this
/// class, so a build that recorded none cannot produce citable build evidence. That is the
/// intended behaviour rather than an inconvenience: a build whose configuration was not
/// recorded cannot explain the artifact it claims to have produced, because two builds of
/// one revision with the same toolchain and different configuration produce different
/// artifacts.
///
/// # Errors
///
/// Returns a provenance error when the record does not satisfy the `BUILD` class.
pub fn from_build(
    provenance: &BuildProvenance,
    id: impl Into<String>,
    observed_at: impl Into<String>,
) -> Result<EvidenceRecord> {
    let mut record = EvidenceRecord::draft(
        id,
        EvidenceClass::parse("BUILD"),
        format!(
            "the build of revision {} for target {} produced the artifact under its recorded \
             digest",
            provenance.source_revision.value, provenance.target
        ),
        observed_at,
    );
    record.toolchain = Some(toolchain_identity(&provenance.toolchain));
    record.configuration_digest = provenance.configuration_digest.clone();
    record.revision = Some(provenance.source_revision.value.clone());
    record.revision_kind = Some(provenance.source_revision.kind);
    // The lockfile digest is recorded as the record's digest when it is present: it is the
    // one input that explains why two builds of one revision can differ, and putting it in
    // the digest field makes it comparable without a second field.
    record.digest = provenance
        .lockfile_digest
        .clone()
        .or_else(|| provenance.configuration_digest.clone());
    record.validate()?;
    Ok(record)
}

/// Whether a configuration digest explains a build.
///
/// A convenience for a caller deciding whether to collect build evidence at all: a build
/// whose configuration was not recorded can still be described, and it cannot produce a
/// citable `BUILD` record.
#[must_use]
pub fn configuration_is_recorded(provenance: &BuildProvenance) -> bool {
    provenance
        .configuration_digest
        .as_ref()
        .is_some_and(|digest| digest.algorithm() == amasario_core::DigestAlgorithm::Sha256)
}

/// A digest over a build configuration, for a producer that has the configuration bytes.
///
/// Provided so that a collector does not reach for the hash function directly and pick a
/// different algorithm: the specification defines SHA-256, and a configuration digest in
/// anything else cannot be compared with one that is.
#[must_use]
pub fn configuration_digest(configuration: &[u8]) -> Digest {
    Digest::sha256_of(configuration)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collector::EvidenceRegistry;
    use amasario_core::{Digest, DigestAlgorithm};
    use amasario_provenance::{Repository, Revision, VcsKind};

    fn repository() -> Repository {
        Repository::new("https://github.com/example/contract", VcsKind::Git).expect("a repository")
    }

    fn source(revision: &str) -> SourceProvenance {
        SourceProvenance {
            repository: repository(),
            revision: Revision::commit(revision).expect("a commit"),
            subdirectory: None,
            archive_digest: None,
            retrieved_at: None,
            evidence: vec!["e-source".to_owned()],
        }
    }

    fn toolchain() -> Toolchain {
        Toolchain {
            name: "rustc".to_owned(),
            version: "1.93.0".to_owned(),
            source: Some("https://static.rust-lang.org".to_owned()),
        }
    }

    fn build() -> BuildProvenance {
        BuildProvenance {
            source_revision: Revision::commit("a".repeat(40)).expect("a commit"),
            toolchain: toolchain(),
            target: "wasm32-unknown-unknown".to_owned(),
            configuration_digest: Some(Digest::sha256_of(b"config")),
            lockfile_digest: Some(Digest::sha256_of(b"lock")),
            artifact: amasario_provenance::ArtifactIdentity::new(
                Digest::sha256_of(b"artifact"),
                amasario_provenance::ArtifactType::Wasm,
                vec!["e-artifact".to_owned()],
            )
            .expect("an artifact identity"),
            reproducibility: amasario_provenance::Reproducibility::not_attempted(),
            environment: Vec::new(),
            evidence: vec!["e-build".to_owned()],
        }
    }

    #[test]
    fn a_source_record_names_the_repository_and_the_exact_revision() {
        let provenance = source(&"a".repeat(40));
        let record = from_source(&provenance, "e-source", "2026-01-01T00:00:00Z")
            .expect("a citable source record");
        assert!(record.is_valid(), "{:?}", record.failures());
        assert_eq!(
            record.repository.as_deref(),
            Some(provenance.repository.url.as_str())
        );
        assert_eq!(record.revision.as_deref(), Some("a".repeat(40).as_str()));
        assert_eq!(record.class.as_str(), "SOURCE");
        assert!(
            record.claim.contains("exact revision") || record.claim.contains("revision"),
            "the claim states what the record supports: {}",
            record.claim
        );
    }

    #[test]
    fn a_branch_revision_is_a_valid_record_that_is_not_decisive() {
        let mut provenance = source(&"a".repeat(40));
        provenance.revision = Revision::branch("main").expect("a branch");
        let record = from_source(&provenance, "e-source", "2026-01-01T00:00:00Z")
            .expect("a branch record is still a record");
        // The record is valid: a branch pins a tree at the moment it was read. What it
        // cannot do is support a VERIFIED claim, and that ceiling is applied by the
        // confidence layer rather than by refusing the evidence - which is what
        // rules/provenance/source-to-build.yaml requires when it says a revision reached
        // through a mutable ref "MUST NOT be reported as VERIFIED".
        assert!(record.is_valid(), "{:?}", record.failures());
        assert_eq!(
            crate::confidence::basis_for(&record),
            crate::confidence::EvidenceBasis::Directional
        );
        let confidence = crate::confidence::assess(&[&record], &[]).expect("a confidence");
        assert_eq!(
            confidence.level,
            amasario_core::ConfidenceLevel::MediumConfidence
        );
        assert_ne!(confidence.level, amasario_core::ConfidenceLevel::Verified);
        assert_eq!(
            crate::confidence::status_for(&[&record], &[], 0),
            amasario_core::VerificationStatus::PartiallyVerified,
            "a record that is not deterministically comparable cannot verify a claim"
        );
    }

    #[test]
    fn a_build_record_carries_the_toolchain_and_the_configuration() {
        let record = from_build(&build(), "e-build", "2026-01-01T00:00:00Z")
            .expect("a citable build record");
        assert!(record.is_valid(), "{:?}", record.failures());
        assert_eq!(
            record.toolchain.as_deref(),
            Some("rustc 1.93.0 (https://static.rust-lang.org)")
        );
        assert_eq!(
            record
                .configuration_digest
                .as_ref()
                .expect("a digest")
                .algorithm(),
            DigestAlgorithm::Sha256
        );
        assert_eq!(record.class.as_str(), "BUILD");
    }

    #[test]
    fn a_build_without_a_recorded_configuration_cannot_produce_build_evidence() {
        let mut provenance = build();
        provenance.configuration_digest = None;
        provenance.lockfile_digest = None;
        let error = from_build(&provenance, "e-build", "2026-01-01T00:00:00Z").expect_err(
            "a build that did not record its configuration cannot explain its artifact",
        );
        assert!(error.to_string().contains("configuration"));
        assert!(!configuration_is_recorded(&provenance));
        assert!(configuration_is_recorded(&build()));
    }

    #[test]
    fn a_toolchain_identity_names_all_three_parts() {
        assert_eq!(
            toolchain_identity(&toolchain()),
            "rustc 1.93.0 (https://static.rust-lang.org)"
        );
        let mut without_source = toolchain();
        without_source.source = None;
        assert_eq!(toolchain_identity(&without_source), "rustc 1.93.0");
        assert_ne!(
            toolchain_identity(&toolchain()),
            toolchain_identity(&without_source),
            "two toolchains that differ must not share an identity"
        );
    }

    #[test]
    fn a_configuration_digest_is_always_sha256() {
        let digest = configuration_digest(b"a configuration");
        assert_eq!(digest.algorithm(), DigestAlgorithm::Sha256);
        assert_eq!(digest, Digest::sha256_of(b"a configuration"));
        assert_ne!(digest, configuration_digest(b"another configuration"));
    }

    #[test]
    fn both_records_can_be_collected_together() {
        let mut registry = EvidenceRegistry::new();
        registry
            .extend([
                from_source(&source(&"a".repeat(40)), "e-source", "2026-01-01T00:00:00Z")
                    .expect("a source record"),
                from_build(&build(), "e-build", "2026-01-01T00:00:00Z").expect("a build record"),
            ])
            .expect("both are citable");
        assert_eq!(registry.len(), 2);
        assert_eq!(
            registry.of_class(amasario_core::EvidenceType::Source).len(),
            1
        );
        assert_eq!(
            registry.of_class(amasario_core::EvidenceType::Build).len(),
            1
        );
    }
}
