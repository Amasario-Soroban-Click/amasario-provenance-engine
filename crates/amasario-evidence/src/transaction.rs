//! Transaction evidence: what a ledger recorded, including whether it succeeded.
//!
//! # The outcome is the field the class exists for
//!
//! `schema/evidence.schema.json` makes `successful` required for `TRANSACTION` evidence and
//! says why: "so that a failed transaction" cannot be read as a successful one. The
//! taxonomy is equally direct - "A transaction that failed MUST NOT be cited as evidence
//! that its intended effect occurred" - and [`TransactionOutcome`] therefore has no way to
//! leave the field unset. An outcome is either [`Outcome::Succeeded`] or
//! [`Outcome::Failed`], and a collector that does not know which cannot produce this
//! evidence at all, which is the honest position: an unknown outcome is not a successful
//! one.
//!
//! # What a failure is still good for
//!
//! A failed transaction is evidence of the attempt. It can support the claim that a
//! deployment was tried and did not take effect, and it is what
//! [`crate::confidence::basis_for`] rates directional rather than decisive for any claim
//! about an effect. Refusing it outright would lose the record that an attempt happened,
//! which is exactly the record an operator needs when a deployment did not land.

use amasario_core::{ContractId, LedgerSequence, ObservationBoundary, Result, TransactionHash};

use crate::collector::{EvidenceClass, EvidenceRecord};

/// How a transaction ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Outcome {
    /// The ledger included the transaction and it succeeded.
    Succeeded,
    /// The ledger included the transaction and it failed.
    Failed,
}

impl Outcome {
    /// The stable wire name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Succeeded => "SUCCEEDED",
            Self::Failed => "FAILED",
        }
    }

    /// Whether the transaction's intended effect can be asserted from it.
    #[must_use]
    pub const fn asserts_effect(self) -> bool {
        matches!(self, Self::Succeeded)
    }

    /// The value the evidence schema's `successful` field takes.
    #[must_use]
    pub const fn successful(self) -> bool {
        self.asserts_effect()
    }
}

/// What a ledger recorded about one transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransactionOutcome {
    /// The transaction's hash.
    pub hash: TransactionHash,
    /// How it ended.
    pub outcome: Outcome,
    /// The boundary the transaction was read at.
    pub boundary: ObservationBoundary,
    /// The contract the transaction concerned, when it concerns one.
    pub contract: Option<ContractId>,
}

impl TransactionOutcome {
    /// Records an outcome, requiring the boundary it was read at.
    ///
    /// The boundary is required rather than optional: "the transaction hash on a named
    /// network" is what a `TRANSACTION` record is traceable to, and a hash without a chain
    /// cannot be looked up again.
    #[must_use]
    pub const fn new(
        hash: TransactionHash,
        outcome: Outcome,
        boundary: ObservationBoundary,
    ) -> Self {
        Self {
            hash,
            outcome,
            boundary,
            contract: None,
        }
    }

    /// Records the contract the transaction concerned.
    #[must_use]
    pub fn concerning(mut self, contract: ContractId) -> Self {
        self.contract = Some(contract);
        self
    }

    /// The ledger the transaction was included in.
    #[must_use]
    pub const fn ledger(&self) -> LedgerSequence {
        self.boundary.ledger
    }
}

/// A record for a transaction, cited for a claim.
///
/// The claim is supplied rather than derived, because the same transaction supports
/// different claims and the record has to say which one it was collected for: a deployment
/// transaction supports "this executable became this contract", and the identical record
/// cited for "this contract's code has this hash" would be a different claim.
///
/// # Errors
///
/// Returns a provenance error when a failed transaction is cited for a claim asserting an
/// effect. The caller can distinguish the two by the [`Outcome`] it passed and should
/// phrase the claim accordingly - "the deployment was attempted" for a failure, "the
/// contract was deployed" for a success.
pub fn from_outcome(
    outcome: &TransactionOutcome,
    claim: impl Into<String>,
    id: impl Into<String>,
    observed_at: impl Into<String>,
) -> Result<EvidenceRecord> {
    let mut record =
        EvidenceRecord::draft(id, EvidenceClass::parse("TRANSACTION"), claim, observed_at);
    record.transaction = Some(outcome.hash.clone());
    record.ledger = Some(outcome.ledger());
    record.boundary = Some(outcome.boundary.clone());
    record.successful = Some(outcome.outcome.successful());
    record.contract_id = outcome.contract.as_ref().map(ToString::to_string);
    record.validate()?;
    Ok(record)
}

/// A record for a successful transaction, which is the only kind that asserts an effect.
///
/// # Errors
///
/// Returns a provenance error when the outcome is a failure, because this constructor
/// exists for the case where the effect is what is being asserted. Use [`from_outcome`]
/// for a failure, which is a record about the attempt.
pub fn from_success(
    outcome: &TransactionOutcome,
    claim: impl Into<String>,
    id: impl Into<String>,
    observed_at: impl Into<String>,
) -> Result<EvidenceRecord> {
    if !outcome.outcome.asserts_effect() {
        return Err(
            crate::errors::EvidenceFailure::FailedTransactionCitedAsEffect {
                transaction: outcome.hash.to_string(),
                claim: claim.into(),
            }
            .into_error(),
        );
    }
    from_outcome(outcome, claim, id, observed_at)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::confidence::{EvidenceBasis, basis_for};
    use amasario_core::{Network, NetworkType};

    fn boundary() -> ObservationBoundary {
        ObservationBoundary::new(
            Network::new(
                "testnet",
                NetworkType::Testnet,
                "Test SDF Network ; September 2015",
            )
            .expect("a network"),
            LedgerSequence::new(4_242).expect("a real ledger"),
            "2026-01-01T00:00:00Z",
        )
    }

    fn hash() -> TransactionHash {
        TransactionHash::new("ab".repeat(32)).expect("a transaction hash")
    }

    fn outcome(outcome: Outcome) -> TransactionOutcome {
        TransactionOutcome::new(hash(), outcome, boundary())
    }

    #[test]
    fn a_successful_transaction_is_decisive_evidence() {
        let record = from_outcome(
            &outcome(Outcome::Succeeded),
            "the transaction carried the contract's deployment",
            "e-1",
            "2026-01-01T00:00:00Z",
        )
        .expect("a citable record");
        assert!(record.is_valid(), "{:?}", record.failures());
        assert_eq!(record.successful, Some(true));
        assert_eq!(record.ledger.expect("a ledger").get(), 4_242);
        assert_eq!(basis_for(&record), EvidenceBasis::Decisive);
    }

    #[test]
    fn a_failed_transaction_is_a_record_about_the_attempt() {
        let record = from_outcome(
            &outcome(Outcome::Failed),
            "the deployment was attempted and the ledger recorded a failure",
            "e-1",
            "2026-01-01T00:00:00Z",
        )
        .expect("a failed transaction is still citable for what it establishes");
        assert!(record.is_valid(), "{:?}", record.failures());
        assert_eq!(record.successful, Some(false));
        // It is evidence, and it is not decisive evidence of an effect.
        assert_eq!(basis_for(&record), EvidenceBasis::Directional);
        assert!(!basis_for(&record).is_decisive());
    }

    #[test]
    fn a_failed_transaction_cannot_be_cited_for_an_effect() {
        let error = from_success(
            &outcome(Outcome::Failed),
            "the executable became the contract at this address",
            "e-1",
            "2026-01-01T00:00:00Z",
        )
        .expect_err("a failed transaction is not evidence that its effect occurred");
        assert!(error.to_string().contains("failed"));
        assert!(
            from_success(
                &outcome(Outcome::Succeeded),
                "the executable became the contract at this address",
                "e-1",
                "2026-01-01T00:00:00Z",
            )
            .is_ok()
        );
    }

    #[test]
    fn a_transaction_record_without_an_outcome_is_not_constructible_through_this_module() {
        // The type makes the unset case unreachable rather than validated away, which is
        // why the schema's requirement for this class is satisfied by construction here.
        let mut record = EvidenceRecord::draft(
            "e-1",
            EvidenceClass::parse("TRANSACTION"),
            "the transaction carried the contract's deployment",
            "2026-01-01T00:00:00Z",
        );
        record.transaction = Some(hash());
        record.boundary = Some(boundary());
        assert!(
            record
                .failures()
                .iter()
                .any(|failure| failure.to_string().contains("succeeded")),
            "the class requirement still holds for a record assembled by hand"
        );
    }

    #[test]
    fn the_outcome_vocabulary_says_what_each_value_can_assert() {
        assert!(Outcome::Succeeded.asserts_effect());
        assert!(Outcome::Succeeded.successful());
        assert!(!Outcome::Failed.asserts_effect());
        assert!(!Outcome::Failed.successful());
        assert_eq!(Outcome::Succeeded.as_str(), "SUCCEEDED");
        assert_eq!(Outcome::Failed.as_str(), "FAILED");
    }

    #[test]
    fn a_transaction_concerning_a_contract_carries_the_address() {
        let contract = ContractId::new("CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAD2KM")
            .expect("a contract address");
        let outcome = outcome(Outcome::Succeeded).concerning(contract.clone());
        let record = from_outcome(
            &outcome,
            "the transaction invoked the contract at this address",
            "e-1",
            "2026-01-01T00:00:00Z",
        )
        .expect("a citable record");
        assert_eq!(
            record.contract_id.as_deref(),
            Some(contract.to_string().as_str())
        );
        assert_eq!(outcome.ledger().get(), 4_242);
    }
}
