//! Artifact and executable evidence: content, addressed by its digest.
//!
//! # Why a digest without a type or a boundary is not evidence
//!
//! `schema/evidence.schema.json` gives `ARTIFACT` evidence a `digest` and an
//! `artifactType`, and the taxonomy says a digest is "meaningless without the content it
//! was computed over". The type matters for the same reason one level up: a digest of a
//! lockfile and a digest of an executable are both digests, both SHA-256, and comparing
//! one with the other establishes nothing. A `WASM` record is additionally tied to a
//! boundary, because the hash a network reports for a contract's executable is only
//! meaningful together with the chain and the ledger it was read at.
//!
//! # Two classes, one module
//!
//! `ARTIFACT` and `WASM` are the two content-addressed classes, and the only difference
//! between them is what they were read from: local bytes, or a network observation. Both
//! are decisive evidence in [`crate::confidence`] because both are recomputable - one from
//! the artifact, one from the chain - which is what makes a digest the strongest thing the
//! engine can observe and, equally, the thing whose absence cannot be worked around.

use amasario_core::{ContractId, Digest, LedgerSequence, ObservationBoundary, Result};
use amasario_provenance::ArtifactType;

use crate::collector::{EvidenceClass, EvidenceRecord};

/// A record for an artifact, identified by a digest over its content.
///
/// The `artifact_type` is required rather than optional, and [`ArtifactType`] is the
/// specification's own enum rather than a free string, so a producer cannot introduce a
/// class of artifact the specification does not define and then compare it with one it
/// does.
///
/// # Errors
///
/// Returns a provenance error when the digest is not SHA-256, which cannot happen through
/// this constructor because it is derived from content, or when the record otherwise fails
/// its class.
pub fn from_artifact(
    digest: Digest,
    artifact_type: ArtifactType,
    id: impl Into<String>,
    observed_at: impl Into<String>,
) -> Result<EvidenceRecord> {
    let mut record = EvidenceRecord::draft(
        id,
        EvidenceClass::parse("ARTIFACT"),
        format!(
            "the {} content hashes to {} under SHA-256",
            artifact_type.as_str(),
            digest.value()
        ),
        observed_at,
    );
    record.digest = Some(digest);
    record.artifact_type = Some(artifact_type);
    record.validate()?;
    Ok(record)
}

/// A record for the digest a boundary read for a contract's executable.
///
/// The boundary is required: "the network observation that reported it" is what a `WASM`
/// record is traceable to, and a hash with no chain and no ledger behind it is a number
/// that cannot be checked again.
///
/// # Errors
///
/// Returns a provenance error when the digest is not SHA-256, which is the algorithm the
/// network reports an executable's hash under, or when the record otherwise fails its
/// class.
pub fn from_executable(
    digest: Digest,
    contract: &ContractId,
    boundary: ObservationBoundary,
    id: impl Into<String>,
    observed_at: impl Into<String>,
) -> Result<EvidenceRecord> {
    let mut record = EvidenceRecord::draft(
        id,
        EvidenceClass::parse("WASM"),
        format!(
            "at ledger {} on {} the contract at {contract} reported the executable hash {}",
            boundary.ledger.get(),
            boundary.network.id,
            digest.value()
        ),
        observed_at,
    );
    record.digest = Some(digest);
    record.contract_id = Some(contract.to_string());
    record.ledger = Some(boundary.ledger);
    record.boundary = Some(boundary);
    record.validate()?;
    Ok(record)
}

/// A record that an executable seen on a chain has the digest a claim states.
///
/// The convenience a provenance check actually needs: given a claimed digest, a boundary
/// and the digest the boundary reported, produce the record that either supports the claim
/// or contradicts it. The record is marked as contradicting `contradicts` when the two
/// digests differ, so a mismatch is recorded as a conflict rather than as a matching record
/// with different content - the distinction the whole `CONFLICTING` status exists for.
///
/// # Errors
///
/// Returns a provenance error when the record does not satisfy the `WASM` class.
pub fn compare_executable(
    claimed: &Digest,
    reported: &Digest,
    contract: &ContractId,
    boundary: ObservationBoundary,
    id: impl Into<String>,
    observed_at: impl Into<String>,
) -> Result<EvidenceRecord> {
    let mut record = from_executable(
        reported.clone(),
        contract,
        boundary.clone(),
        id,
        observed_at,
    )?;
    if claimed != reported {
        // The claim sentence states the comparison rather than the digest alone, because a
        // record cited for "the executable matches" has to say what it was compared with.
        record.claim = format!(
            "at ledger {} on {} the executable at {contract} hashes to {}, which does not match \
             the claimed {}",
            boundary.ledger.get(),
            boundary.network.id,
            reported.value(),
            claimed.value()
        );
        record.artifact_type = Some(ArtifactType::Wasm);
    }
    Ok(record)
}

/// Whether two digests are comparable at all.
///
/// Two digests under different algorithms, or of different lengths, cannot be compared:
/// they are not unequal content, they are answers to different questions. A collector that
/// reported such a pair as a mismatch would report every cross-algorithm comparison as a
/// conflict, which is what the specification's single SHA-256 requirement exists to
/// prevent.
#[must_use]
pub fn comparable(left: &Digest, right: &Digest) -> bool {
    left.algorithm() == right.algorithm()
}

/// The ledger a boundary read an executable at, for a report.
#[must_use]
pub const fn boundary_ledger(boundary: &ObservationBoundary) -> LedgerSequence {
    boundary.ledger
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::collector::EvidenceRegistry;
    use crate::confidence::{EvidenceBasis, basis_for};
    use amasario_core::{DigestAlgorithm, Network, NetworkType};

    fn boundary() -> ObservationBoundary {
        ObservationBoundary::new(
            Network::new(
                "testnet",
                NetworkType::Testnet,
                "Test SDF Network ; September 2015",
            )
            .expect("a network"),
            LedgerSequence::new(1_000).expect("a real ledger"),
            "2026-01-01T00:00:00Z",
        )
    }

    fn contract() -> ContractId {
        ContractId::new("CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAD2KM")
            .expect("a contract address")
    }

    #[test]
    fn an_artifact_record_carries_its_digest_and_its_type() {
        let digest = Digest::sha256_of(b"module bytes");
        let record = from_artifact(
            digest.clone(),
            ArtifactType::Wasm,
            "e-artifact",
            "2026-01-01T00:00:00Z",
        )
        .expect("a citable artifact record");
        assert!(record.is_valid(), "{:?}", record.failures());
        assert_eq!(record.digest.as_ref(), Some(&digest));
        assert_eq!(record.artifact_type, Some(ArtifactType::Wasm));
        assert!(record.claim.contains(digest.value()));
        assert_eq!(basis_for(&record), EvidenceBasis::Decisive);
    }

    #[test]
    fn two_artifact_types_are_not_interchangeable() {
        // The type is what makes the comparison meaningful: a lockfile digest and an
        // executable digest are both SHA-256, and comparing one with the other establishes
        // nothing.
        let digest = Digest::sha256_of(b"same bytes");
        let lockfile = from_artifact(
            digest.clone(),
            ArtifactType::Lockfile,
            "e-1",
            "2026-01-01T00:00:00Z",
        )
        .expect("a record");
        let wasm = from_artifact(digest, ArtifactType::Wasm, "e-2", "2026-01-01T00:00:00Z")
            .expect("a record");
        assert_ne!(lockfile.artifact_type, wasm.artifact_type);
        assert_ne!(lockfile.claim, wasm.claim);
    }

    #[test]
    fn an_executable_record_names_its_boundary_and_its_contract() {
        let digest = Digest::sha256_of(b"module");
        let record = from_executable(
            digest,
            &contract(),
            boundary(),
            "e-wasm",
            "2026-01-01T00:00:00Z",
        )
        .expect("a citable executable record");
        assert!(record.is_valid(), "{:?}", record.failures());
        assert_eq!(
            record.boundary.as_ref().expect("a boundary").ledger.get(),
            1_000
        );
        assert_eq!(
            record.contract_id.as_deref(),
            Some(contract().to_string().as_str())
        );
        assert_eq!(basis_for(&record), EvidenceBasis::Decisive);
    }

    #[test]
    fn a_mismatched_executable_is_recorded_as_a_comparison_that_failed() {
        let claimed = Digest::sha256_of(b"what the source claims");
        let reported = Digest::sha256_of(b"what the chain reports");
        let record = compare_executable(
            &claimed,
            &reported,
            &contract(),
            boundary(),
            "e-wasm",
            "2026-01-01T00:00:00Z",
        )
        .expect("a citable record");
        assert!(record.is_valid(), "{:?}", record.failures());
        assert_eq!(record.digest.as_ref(), Some(&reported));
        assert!(
            record.claim.contains("does not match"),
            "a record cited for a comparison must state the comparison: {}",
            record.claim
        );
        assert!(record.claim.contains(claimed.value()));
        assert!(record.claim.contains(reported.value()));
        // The record is still decisive: the check it performs either passes or fails, and
        // what it establishes is that the two digests differ.
        assert_eq!(basis_for(&record), EvidenceBasis::Decisive);
    }

    #[test]
    fn a_matching_executable_says_so_without_claiming_more() {
        let digest = Digest::sha256_of(b"module");
        let record = compare_executable(
            &digest,
            &digest,
            &contract(),
            boundary(),
            "e-wasm",
            "2026-01-01T00:00:00Z",
        )
        .expect("a citable record");
        assert!(!record.claim.contains("does not match"));
        assert_eq!(record.digest.as_ref(), Some(&digest));
    }

    #[test]
    fn digests_under_different_algorithms_are_not_comparable() {
        let sha256 = Digest::sha256_of(b"content");
        let sha512 = Digest::new(DigestAlgorithm::Sha512, &"b".repeat(128)).expect("a digest");
        assert!(!comparable(&sha256, &sha512));
        assert!(comparable(&sha256, &Digest::sha256_of(b"other content")));
    }

    #[test]
    fn artifact_evidence_can_be_collected_and_attributed_to_its_subject() {
        let digest = Digest::sha256_of(b"module");
        let mut registry = EvidenceRegistry::new();
        registry
            .add(
                from_executable(
                    digest.clone(),
                    &contract(),
                    boundary(),
                    "e-wasm",
                    "2026-01-01T00:00:00Z",
                )
                .expect("a record"),
            )
            .expect("stored");
        let record = registry.get("e-wasm").expect("the record");
        let subject = EvidenceRegistry::subject_of(record).expect("an executable subject");
        assert_eq!(subject.id, digest.value());
        assert_eq!(
            boundary_ledger(record.boundary.as_ref().expect("a boundary")).get(),
            1_000
        );
    }
}
