//! Deployment evidence: an executable becoming the contract at an address.
//!
//! # Why a deployment is the class that ties the chain together
//!
//! `TRANSACTION` evidence records that a transaction was included; `DEPLOYMENT` evidence
//! records that a specific executable became the contract at a specific address, in a
//! specific ledger, by a specific transaction. It is the hop where the artifact chain meets
//! the chain of state, and the taxonomy's `traceableTo` for it - "the deployment
//! transaction and ledger" - is exactly the minimum needed to re-check it.
//!
//! [`from_deployment`] takes the provenance record the engine already builds and a
//! [`DeploymentStatus`], and refuses to produce evidence for a deployment that has not been
//! established as having taken effect. That is the same rule the impact layer applies, at
//! the layer where the evidence is created: a failed deployment is evidence of an attempt,
//! and it cannot be evidence that an executable became a contract.
//!
//! Runtime observations belong to [`crate::event`] rather than here, because a deployment
//! is a state transition and an event is an execution: the two are collected from
//! different queries and are cited for different claims.

use amasario_core::{ObservationBoundary, Result};
use amasario_provenance::{DeploymentProvenance, DeploymentStatus};

use crate::collector::{EvidenceClass, EvidenceRecord};

/// A record that an executable became the contract at an address.
///
/// # Errors
///
/// Returns a provenance error when the deployment is not eligible - that is, when its
/// status is `UNCONFIRMED`, `FAILED` or `UNKNOWN` - or when the record otherwise fails its
/// class. The refusal message names the status, because "the deployment is not citable" is
/// not actionable and "the deployment is `FAILED`" is.
pub fn from_deployment(
    provenance: &DeploymentProvenance,
    status: DeploymentStatus,
    boundary: ObservationBoundary,
    id: impl Into<String>,
    observed_at: impl Into<String>,
) -> Result<EvidenceRecord> {
    if !status.eligible_for_impact() {
        return Err(
            crate::errors::EvidenceFailure::FailedTransactionCitedAsEffect {
                transaction: provenance.transaction.as_ref().map_or_else(
                    || "an unrecorded transaction".to_owned(),
                    |hash| hash.to_string(),
                ),
                claim: format!(
                    "the executable {} became the contract at {} (status {})",
                    provenance
                        .wasm_hash
                        .as_ref()
                        .map_or("recorded in the deployment", |digest| digest.value()),
                    provenance.contract_id,
                    status.as_str()
                ),
            }
            .into_error(),
        );
    }

    let mut record = EvidenceRecord::draft(
        id,
        EvidenceClass::parse("DEPLOYMENT"),
        format!(
            "at ledger {} on {} the transaction {} recorded a {} at {} for the executable {} \
             (status {})",
            provenance.ledger.get(),
            boundary.network.id,
            provenance
                .transaction
                .as_ref()
                .map_or("with an unrecorded hash", |hash| hash.as_str()),
            provenance.kind.as_str(),
            provenance.contract_id,
            provenance
                .wasm_hash
                .as_ref()
                .map_or("recorded by the deployment", |digest| digest.value()),
            status.as_str()
        ),
        observed_at,
    );
    record.contract_id = Some(provenance.contract_id.to_string());
    record.transaction = provenance.transaction.clone();
    record.ledger = Some(provenance.ledger);
    record.boundary = Some(boundary);
    // `successful` is the schema's field for "this took effect", and a deployment that has
    // been established as observed or confirmed is one that did.
    record.successful = Some(status.eligible_for_impact());
    record.digest = provenance.wasm_hash.clone();
    record.artifact_type = provenance
        .wasm_hash
        .as_ref()
        .map(|_| amasario_provenance::ArtifactType::Wasm);
    record.validate()?;
    Ok(record)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::confidence::{EvidenceBasis, basis_for};
    use amasario_core::{
        ContractId, Digest, LedgerSequence, Network, NetworkType, TransactionHash,
    };
    use amasario_provenance::DeploymentKind;

    const PASSPHRASE: &str = "Test SDF Network ; September 2015";

    fn boundary() -> ObservationBoundary {
        ObservationBoundary::new(
            Network::new("testnet", NetworkType::Testnet, PASSPHRASE).expect("a network"),
            LedgerSequence::new(1_000).expect("a real ledger"),
            "2026-01-01T00:00:00Z",
        )
    }

    fn contract() -> ContractId {
        ContractId::new("CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAD2KM")
            .expect("a contract address")
    }

    fn provenance() -> DeploymentProvenance {
        DeploymentProvenance::new(
            contract(),
            "testnet",
            PASSPHRASE,
            LedgerSequence::new(1_000).expect("a real ledger"),
            DeploymentKind::Deploy,
            vec!["tx-hash".to_owned()],
        )
        .expect("a deployment record")
        .by_transaction(
            TransactionHash::new("ab".repeat(32)).expect("a transaction hash"),
            Some(0),
        )
        .with_wasm_hash(Digest::sha256_of(b"module"))
        .expect("a wasm hash")
    }

    #[test]
    fn a_confirmed_deployment_produces_decisive_evidence() {
        let record = from_deployment(
            &provenance(),
            DeploymentStatus::Confirmed,
            boundary(),
            "e-deploy",
            "2026-01-01T00:00:00Z",
        )
        .expect("a citable deployment record");
        assert!(record.is_valid(), "{:?}", record.failures());
        assert!(record.transaction.is_some());
        assert_eq!(record.ledger.expect("a ledger").get(), 1_000);
        assert_eq!(record.successful, Some(true));
        assert_eq!(record.digest, provenance().wasm_hash);
        assert_eq!(basis_for(&record), EvidenceBasis::Decisive);
    }

    #[test]
    fn a_deployment_that_has_not_taken_effect_cannot_be_cited_as_evidence_of_one() {
        for status in [
            DeploymentStatus::Unconfirmed,
            DeploymentStatus::Failed,
            DeploymentStatus::Unknown,
        ] {
            let error = from_deployment(
                &provenance(),
                status,
                boundary(),
                "e-deploy",
                "2026-01-01T00:00:00Z",
            )
            .expect_err("an unestablished deployment is not evidence that it took effect");
            let message = error.to_string();
            assert!(message.contains(status.as_str()), "got: {message}");
            assert!(
                message.contains("not of the effect"),
                "the refusal must say what the record is evidence of instead: {message}"
            );
        }
        // An observed deployment counts: it was seen in a ledger, which is what taking
        // effect means.
        assert!(
            from_deployment(
                &provenance(),
                DeploymentStatus::Observed,
                boundary(),
                "e-deploy",
                "2026-01-01T00:00:00Z",
            )
            .is_ok()
        );
    }

    #[test]
    fn a_deployment_without_a_recorded_executable_hash_separates_the_facts() {
        // A deployment record can exist without the hash it installed, and the evidence
        // says so rather than substituting a value: the record then supports the claim
        // about the address and the ledger, and nothing about which executable arrived.
        let without_hash = DeploymentProvenance::new(
            contract(),
            "testnet",
            PASSPHRASE,
            LedgerSequence::new(1_000).expect("a real ledger"),
            DeploymentKind::Upgrade,
            vec!["tx-hash".to_owned()],
        )
        .expect("a deployment record")
        .by_transaction(
            TransactionHash::new("ef".repeat(32)).expect("a transaction hash"),
            None,
        );
        let record = from_deployment(
            &without_hash,
            DeploymentStatus::Confirmed,
            boundary(),
            "e-deploy",
            "2026-01-01T00:00:00Z",
        )
        .expect("a citable record");
        assert!(record.is_valid(), "{:?}", record.failures());
        assert!(record.digest.is_none());
        assert!(record.claim.contains("recorded by the deployment"));
        assert!(record.claim.contains("UPGRADE"));
    }
}
