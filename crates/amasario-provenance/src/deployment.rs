//! Deployment provenance: how an executable came to be at an address, and when.
//!
//! # Deployment and upgrade are different facts
//!
//! A contract's address survives an upgrade, so "the transaction that put this
//! executable at this address" and "the transaction that first created this
//! contract" are different questions with different answers. [`DeploymentKind`]
//! keeps them apart, and `Unknown` is a third value rather than a default: a record
//! that has not established which happened must not be read as either.
//!
//! # Why the network is part of the record
//!
//! A ledger sequence is only meaningful against a chain. The record therefore carries
//! the passphrase alongside the sequence, and `is_on_same_network_as` compares the
//! passphrase rather than an identifier - two operators can name one chain
//! differently, and two chains can be given the same name.

use std::fmt;
use std::str::FromStr;

use amasario_core::{
    ContractId, Digest, EngineError, EntityKind, EntityRef, LedgerSequence, Result, TransactionHash,
};
use serde::{Deserialize, Serialize};

use crate::errors::ProvenanceFailure;

/// How an executable came to be at a contract's address.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum DeploymentKind {
    /// The contract was created at this address by this transaction.
    Deploy,
    /// The contract already existed and this transaction replaced its executable.
    ///
    /// Named after the Soroban host function a contract invokes on itself,
    /// `update_current_contract_wasm`, rather than invented here.
    Upgrade,
    /// Which of the two happened was not established.
    ///
    /// A distinct value rather than a default, because a record that has not
    /// established it must not be read as either. The engine reaches it whenever the
    /// modification's transaction was not identified, which is the normal outcome of
    /// a bounded search that did not have to look.
    Unknown,
}

impl DeploymentKind {
    /// The stable wire name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Deploy => "DEPLOY",
            Self::Upgrade => "UPGRADE",
            Self::Unknown => "UNKNOWN",
        }
    }

    /// Whether this record establishes the contract's origin.
    ///
    /// Only a `Deploy` does. An `Upgrade` says the contract already existed, so it
    /// establishes what the contract is *now* and nothing about where it came from.
    #[must_use]
    pub const fn establishes_origin(self) -> bool {
        matches!(self, Self::Deploy)
    }

    /// Whether this record establishes what the contract currently executes.
    ///
    /// Both a `Deploy` and an `Upgrade` do: each put an executable in place. Only
    /// `Unknown` does not.
    #[must_use]
    pub const fn establishes_current_executable(self) -> bool {
        matches!(self, Self::Deploy | Self::Upgrade)
    }
}

impl fmt::Display for DeploymentKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for DeploymentKind {
    type Err = EngineError;

    fn from_str(value: &str) -> Result<Self> {
        match value {
            "DEPLOY" => Ok(Self::Deploy),
            "UPGRADE" => Ok(Self::Upgrade),
            "UNKNOWN" => Ok(Self::Unknown),
            other => Err(EngineError::Validation {
                path: "/deployment/kind".to_owned(),
                detail: format!("unrecognised deployment kind {other:?}"),
            }),
        }
    }
}

/// A contract's deployment provenance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DeploymentProvenance {
    /// The address the executable was placed at.
    pub contract_id: ContractId,
    /// The network's identifier, for display.
    pub network_id: String,
    /// The network's passphrase, which is what identifies the chain.
    pub passphrase: String,
    /// The ledger the deployment was included in.
    pub ledger: LedgerSequence,
    /// The transaction that performed it, when identified.
    ///
    /// Optional because identifying it requires reading the ledger's operations and
    /// following the candidate transaction, and a run that did not ask for that - or
    /// that was bounded before it finished - legitimately does not have it. A guessed
    /// transaction would be worse than none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transaction: Option<TransactionHash>,
    /// The position of the operation within its transaction, when recorded.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operation_index: Option<u32>,
    /// The executable hash put in place, when the executable is a WASM module.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wasm_hash: Option<Digest>,
    /// Whether this was a deployment or an upgrade.
    pub kind: DeploymentKind,
    /// Identifiers of the evidence records that support this claim.
    pub evidence: Vec<String>,
}

impl DeploymentProvenance {
    /// Records a deployment.
    ///
    /// # Errors
    ///
    /// Returns a validation error when no evidence is cited - a deployment record
    /// with nothing behind it asserts that a transaction happened without naming what
    /// observed it - or when the network's passphrase is empty, since a ledger
    /// sequence without a chain is meaningless.
    pub fn new(
        contract_id: ContractId,
        network_id: impl Into<String>,
        passphrase: impl Into<String>,
        ledger: LedgerSequence,
        kind: DeploymentKind,
        evidence: Vec<String>,
    ) -> Result<Self> {
        let passphrase = passphrase.into();
        if passphrase.is_empty() {
            return Err(EngineError::Validation {
                path: "/deployment/passphrase".to_owned(),
                detail: "a deployment record needs the network's passphrase; a ledger sequence \
                         without a chain does not identify a point in any history"
                    .to_owned(),
            });
        }
        if evidence.is_empty() {
            return Err(EngineError::Validation {
                path: "/deployment/evidence".to_owned(),
                detail: "a deployment claim must cite the evidence that supports it".to_owned(),
            });
        }
        Ok(Self {
            contract_id,
            network_id: network_id.into(),
            passphrase,
            ledger,
            transaction: None,
            operation_index: None,
            wasm_hash: None,
            kind,
            evidence,
        })
    }

    /// Records the transaction and operation that performed the deployment.
    #[must_use]
    pub fn by_transaction(
        mut self,
        transaction: TransactionHash,
        operation_index: Option<u32>,
    ) -> Self {
        self.transaction = Some(transaction);
        self.operation_index = operation_index;
        self
    }

    /// Records the executable hash the deployment put in place.
    ///
    /// # Errors
    ///
    /// Returns a provenance error when the digest is not a SHA-256, because SHA-256
    /// is the algorithm the Stellar network reports for a contract's executable hash
    /// and a digest under another algorithm would never match one.
    pub fn with_wasm_hash(mut self, digest: Digest) -> Result<Self> {
        if digest.algorithm() != amasario_core::DigestAlgorithm::Sha256 {
            return Err(EngineError::Validation {
                path: "/deployment/wasmHash".to_owned(),
                detail: format!(
                    "a WASM hash is a SHA-256; {} is not one, and a digest under another algorithm \
                     would never compare equal to the network's",
                    digest.prefixed()
                ),
            });
        }
        self.wasm_hash = Some(digest);
        Ok(self)
    }

    /// A reference to the deployment as an entity.
    ///
    /// A deployment is identified by the contract it acted on, qualified by the
    /// network, plus the ledger it happened at. Not by the transaction alone: a
    /// transaction that deployed several contracts is one transaction and several
    /// deployments.
    #[must_use]
    pub fn as_ref(&self) -> EntityRef {
        EntityRef {
            kind: EntityKind::Deployment,
            id: format!(
                "{}:{}:{}",
                self.passphrase,
                self.contract_id,
                self.ledger.get()
            ),
        }
    }

    /// The address qualified by the network it belongs to.
    #[must_use]
    pub fn network_qualified_id(&self) -> String {
        format!("{}:{}", self.network_id, self.contract_id)
    }

    /// Whether two records describe a deployment on the same chain.
    #[must_use]
    pub fn is_on_same_network_as(&self, other: &Self) -> bool {
        self.passphrase == other.passphrase
    }

    /// Whether this record establishes that the contract was created here.
    #[must_use]
    pub const fn establishes_origin(&self) -> bool {
        self.kind.establishes_origin()
    }

    /// Whether this record can be checked against a deployed module's bytes.
    ///
    /// Requires a WASM hash and a kind that puts an executable in place. An
    /// `Unknown` kind with a hash could have been an upgrade or a deployment, and the
    /// engine does not know which executable the hash belongs to.
    #[must_use]
    pub const fn is_checkable_against_module(&self) -> bool {
        self.wasm_hash.is_some() && self.kind.establishes_current_executable()
    }

    /// Checks the record's own invariants.
    ///
    /// # Errors
    ///
    /// Returns a validation error when no evidence is cited, when the passphrase is
    /// empty, or when a WASM hash is recorded under an algorithm other than SHA-256.
    pub fn validate(&self) -> Result<()> {
        if self.evidence.is_empty() {
            return Err(EngineError::Validation {
                path: "/deployment/evidence".to_owned(),
                detail: "a deployment claim must cite the evidence that supports it".to_owned(),
            });
        }
        if self.passphrase.is_empty() {
            return Err(ProvenanceFailure::DeploymentUnrecorded {
                contract_id: self.contract_id.to_string(),
            }
            .into_error());
        }
        if self
            .wasm_hash
            .as_ref()
            .is_some_and(|digest| digest.algorithm() != amasario_core::DigestAlgorithm::Sha256)
        {
            return Err(EngineError::Validation {
                path: "/deployment/wasmHash".to_owned(),
                detail: "a WASM hash is a SHA-256".to_owned(),
            });
        }
        Ok(())
    }

    /// Every way this record differs from another, for a report's "what changed"
    /// section.
    ///
    /// Returns field names rather than a boolean so that a report can say which fact
    /// moved. An upgrade is exactly the case a consumer asks this question about, and
    /// "the deployment changed" without an answer to *how* is not actionable.
    #[must_use]
    pub fn differences(&self, other: &Self) -> Vec<&'static str> {
        let mut differences = Vec::new();
        if self.contract_id != other.contract_id {
            differences.push("contractId");
        }
        if self.passphrase != other.passphrase {
            differences.push("passphrase");
        }
        if self.ledger != other.ledger {
            differences.push("ledger");
        }
        if self.transaction != other.transaction {
            differences.push("transaction");
        }
        if self.wasm_hash != other.wasm_hash {
            differences.push("wasmHash");
        }
        if self.kind != other.kind {
            differences.push("kind");
        }
        differences
    }
}

/// How far a deployment record has been established.
///
/// # Why eligibility is a method here rather than a filter in the impact layer
///
/// The taxonomy's five terms are about a deployment record, so the rule that reads
/// them belongs with the record. Impact analysis needs exactly one question answered
/// (`eligible_for_impact`) and asking it of the record keeps the answer in one place:
/// an impact finding that named a deployment which never took effect would report the
/// consequence of an event that did not happen.
///
/// The taxonomy's terms are `OBSERVED`, `CONFIRMED`, `UNCONFIRMED`, `FAILED` and
/// `UNKNOWN`. `OBSERVED` and `CONFIRMED` are both eligible: a deployment seen in a
/// ledger has taken effect by definition, and requiring `CONFIRMED` would exclude
/// records the engine can establish from a successful transaction but has not
/// cross-checked against a second source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum DeploymentStatus {
    /// The deployment was seen in a ledger that was observed.
    Observed,
    /// The deployment was cross-checked and its transaction confirmed successful.
    Confirmed,
    /// The deployment was recorded but not established as having taken effect.
    Unconfirmed,
    /// The deployment's transaction failed, so the deployment never took effect.
    Failed,
    /// Whether the deployment took effect was not established.
    Unknown,
}

impl DeploymentStatus {
    /// The stable wire name, matching the taxonomy.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Observed => "OBSERVED",
            Self::Confirmed => "CONFIRMED",
            Self::Unconfirmed => "UNCONFIRMED",
            Self::Failed => "FAILED",
            Self::Unknown => "UNKNOWN",
        }
    }

    /// Whether a change could affect a deployment with this status.
    ///
    /// The rule `impact/deployment-impact` states the exclusion directly: a finding
    /// must not name a deployment whose status is `UNCONFIRMED`, `FAILED` or
    /// `UNKNOWN`. `FAILED` is the clearest case - the deployment never happened - but
    /// the other two are excluded for the same reason: an impact claim asserts that a
    /// change may reach a thing that exists, and each of these three says that has not
    /// been established.
    ///
    /// # Examples
    ///
    /// ```
    /// use amasario_provenance::DeploymentStatus;
    ///
    /// assert!(DeploymentStatus::Confirmed.eligible_for_impact());
    /// assert!(DeploymentStatus::Observed.eligible_for_impact());
    /// assert!(!DeploymentStatus::Failed.eligible_for_impact());
    /// assert!(!DeploymentStatus::Unknown.eligible_for_impact());
    /// ```
    #[must_use]
    pub const fn eligible_for_impact(self) -> bool {
        matches!(self, Self::Observed | Self::Confirmed)
    }
}

impl fmt::Display for DeploymentStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for DeploymentStatus {
    type Err = EngineError;

    fn from_str(value: &str) -> Result<Self> {
        match value {
            "OBSERVED" => Ok(Self::Observed),
            "CONFIRMED" => Ok(Self::Confirmed),
            "UNCONFIRMED" => Ok(Self::Unconfirmed),
            "FAILED" => Ok(Self::Failed),
            "UNKNOWN" => Ok(Self::Unknown),
            other => Err(EngineError::Validation {
                path: "/deployment/status".to_owned(),
                detail: format!("unrecognised deployment status {other:?}"),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PASSPHRASE: &str = "Test SDF Network ; September 2015";

    fn address(payload: [u8; 32]) -> ContractId {
        let strkey = format!("{}", stellar_strkey::Contract(payload));
        ContractId::new(strkey).expect("a real contract address")
    }

    fn digest(seed: u8) -> Digest {
        Digest::sha256_of(&[seed])
    }

    fn deployment(kind: DeploymentKind) -> DeploymentProvenance {
        DeploymentProvenance::new(
            address([1_u8; 32]),
            "testnet",
            PASSPHRASE,
            LedgerSequence::new(1_000).expect("a real ledger"),
            kind,
            vec!["e".to_owned()],
        )
        .expect("a valid deployment")
    }

    #[test]
    fn a_deployment_record_must_cite_evidence_and_name_a_chain() {
        // A ledger sequence without a chain does not identify a point in any history.
        DeploymentProvenance::new(
            address([1_u8; 32]),
            "testnet",
            "",
            LedgerSequence::new(1).expect("valid"),
            DeploymentKind::Deploy,
            vec!["e".to_owned()],
        )
        .expect_err("no chain means no ledger");

        DeploymentProvenance::new(
            address([1_u8; 32]),
            "testnet",
            PASSPHRASE,
            LedgerSequence::new(1).expect("valid"),
            DeploymentKind::Deploy,
            Vec::new(),
        )
        .expect_err("no evidence observes the deployment");
    }

    #[test]
    fn only_a_deploy_establishes_the_contracts_origin() {
        // An upgrade says the contract already existed, so it establishes what the
        // contract is now and nothing about where it came from.
        assert!(DeploymentKind::Deploy.establishes_origin());
        assert!(!DeploymentKind::Upgrade.establishes_origin());
        assert!(!DeploymentKind::Unknown.establishes_origin());

        assert!(DeploymentKind::Deploy.establishes_current_executable());
        assert!(DeploymentKind::Upgrade.establishes_current_executable());
        assert!(!DeploymentKind::Unknown.establishes_current_executable());
    }

    #[test]
    fn an_unknown_kind_is_not_read_as_either_outcome() {
        let unknown = deployment(DeploymentKind::Unknown);
        assert!(!unknown.establishes_origin());
        assert!(!unknown.kind.establishes_current_executable());
        assert!(!unknown.is_checkable_against_module());
    }

    #[test]
    fn a_wasm_hash_must_be_a_sha256() {
        // SHA-256 is what the network reports for a contract's executable hash, so a
        // digest under another algorithm would never compare equal to it.
        deployment(DeploymentKind::Deploy)
            .with_wasm_hash(digest(1))
            .expect("a SHA-256 is a WASM hash");

        let error = deployment(DeploymentKind::Deploy)
            .with_wasm_hash(
                Digest::new(amasario_core::DigestAlgorithm::Sha512, &"a".repeat(128))
                    .expect("valid"),
            )
            .expect_err("a SHA-512 is not a WASM hash");
        assert!(error.to_string().contains("would never compare equal"));
    }

    #[test]
    fn a_wasm_hash_and_a_known_kind_are_both_needed_to_check_against_a_module() {
        let deployed = deployment(DeploymentKind::Deploy)
            .with_wasm_hash(digest(1))
            .expect("valid");
        assert!(deployed.is_checkable_against_module());

        let no_hash = deployment(DeploymentKind::Deploy);
        assert!(!no_hash.is_checkable_against_module());

        let unknown_with_hash = deployment(DeploymentKind::Unknown)
            .with_wasm_hash(digest(1))
            .expect("valid");
        assert!(
            !unknown_with_hash.is_checkable_against_module(),
            "an unknown kind does not say which executable the hash belongs to"
        );
    }

    #[test]
    fn a_deployment_is_identified_by_its_contract_chain_and_ledger() {
        // A transaction that deployed several contracts is one transaction and
        // several deployments, so the transaction alone is not the identity.
        let first = deployment(DeploymentKind::Deploy);
        assert_eq!(first.as_ref().kind, EntityKind::Deployment);
        assert!(first.as_ref().id.contains(&first.contract_id.to_string()));
        assert!(first.as_ref().id.contains(PASSPHRASE));
        assert!(first.as_ref().id.ends_with("1000"));
        assert_eq!(
            first.network_qualified_id(),
            format!("testnet:{}", first.contract_id)
        );
    }

    #[test]
    fn two_records_of_one_chain_compare_by_passphrase_not_by_identifier() {
        // Two operators can name one chain differently, and the passphrase is the
        // fact rather than the name.
        let mut renamed = deployment(DeploymentKind::Deploy);
        renamed.network_id = "my-testnet".to_owned();
        let original = deployment(DeploymentKind::Deploy);
        assert!(original.is_on_same_network_as(&renamed));

        let mut other_chain = deployment(DeploymentKind::Deploy);
        other_chain.passphrase = "Public Global Stellar Network ; September 2015".to_owned();
        assert!(!original.is_on_same_network_as(&other_chain));
    }

    #[test]
    fn the_differences_between_two_deployments_name_each_field_that_moved() {
        let before = deployment(DeploymentKind::Deploy)
            .with_wasm_hash(digest(1))
            .expect("valid");
        let after = deployment(DeploymentKind::Upgrade)
            .with_wasm_hash(digest(2))
            .expect("valid");

        let differences = before.differences(&after);
        assert!(differences.contains(&"kind"));
        assert!(differences.contains(&"wasmHash"));
        assert!(
            !differences.contains(&"contractId"),
            "the address did not move, which is exactly why the address alone is not an identity"
        );
        assert!(before.differences(&before).is_empty());
    }

    #[test]
    fn a_deployment_records_the_transaction_that_performed_it() {
        let hash = TransactionHash::new("ab".repeat(32)).expect("a real hash");
        let recorded = deployment(DeploymentKind::Deploy).by_transaction(hash.clone(), Some(2));
        assert_eq!(recorded.transaction, Some(hash));
        assert_eq!(recorded.operation_index, Some(2));
    }

    #[test]
    fn deployment_kinds_round_trip_through_their_wire_names() {
        for kind in [
            DeploymentKind::Deploy,
            DeploymentKind::Upgrade,
            DeploymentKind::Unknown,
        ] {
            assert_eq!(
                DeploymentKind::from_str(kind.as_str()).expect("round trip"),
                kind
            );
        }
        DeploymentKind::from_str("MIGRATE").expect_err("an unknown kind is rejected");
    }

    #[test]
    fn a_deserialised_deployment_is_caught_by_validation() {
        let mut record = deployment(DeploymentKind::Deploy);
        record.validate().expect("self-consistent");
        record.evidence.clear();
        record
            .validate()
            .expect_err("not a record without evidence");
    }

    #[test]
    fn a_deployment_round_trips_through_json() {
        let original = deployment(DeploymentKind::Deploy)
            .by_transaction(
                TransactionHash::new("cd".repeat(32)).expect("a real hash"),
                Some(1),
            )
            .with_wasm_hash(digest(3))
            .expect("valid");
        let json = serde_json::to_string(&original).expect("serialises");
        let restored: DeploymentProvenance = serde_json::from_str(&json).expect("deserialises");
        assert_eq!(restored, original);
        restored.validate().expect("self-consistent");
    }
}
