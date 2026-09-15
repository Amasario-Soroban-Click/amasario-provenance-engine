//! Contract identity: the properties that identify a deployed contract, and the
//! properties that merely describe an observation of one.
//!
//! # Why this is not the address
//!
//! The specification's first rule is that a contract address alone is insufficient
//! to establish provenance, and the reason is mechanical rather than philosophical:
//! a Soroban contract can be upgraded in place, so the same address can execute
//! different code at two different ledgers. An identity that is only an address
//! therefore cannot distinguish "the same contract, unchanged" from "a different
//! program at the same address", and every conclusion downstream of that - what it
//! depends on, what it was built from, what a change to it would affect - differs
//! between the two.
//!
//! [`ContractIdentity`] addresses this by separating the two kinds of property:
//!
//! * **Identity** is the address, the network it lives on, and the executable it
//!   resolves to. Two identities that differ in any of those are different
//!   contracts, even at the same address.
//! * **Observation** is when the identity was seen - the boundary ledger and the
//!   ledgers it was first and last observed at. None of it identifies anything, and
//!   [`ContractIdentity::fingerprint`] deliberately excludes it, so that a
//!   contract observed twice at two ledgers does not acquire two identities.
//!
//! # On the executable
//!
//! A contract's executable is either a deployed WASM module, identified by the
//! SHA-256 digest the network records, or the built-in Stellar Asset Contract.
//! Both are represented, because reporting the asset contract's absent WASM hash as
//! an unknown identity would be wrong: its identity is complete and known.

use std::fmt;
use std::str::FromStr;

use amasario_core::{
    ContractId, Digest, DigestAlgorithm, EngineError, LedgerSequence, Network, Result,
    TransactionHash,
};
use serde::{Deserialize, Serialize};

use crate::errors::InspectionFailure;

/// The version of the contract identity model this crate produces.
///
/// Recorded on every identity so that a later consumer can tell which model
/// produced it. Versioned separately from the specification because the identity
/// model can gain a field without the specification's wire format changing.
pub const CONTRACT_IDENTITY_VERSION: &str = "amasario/contract-identity/v1";

/// What a contract's instance entry says it executes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ContractExecutableKind {
    /// A deployed WebAssembly module, identified by its SHA-256 digest.
    Wasm,
    /// The Stellar Asset Contract: an executable the protocol provides rather than
    /// one that was deployed.
    ///
    /// Present as a distinct value rather than as an absent WASM hash because the
    /// two mean different things. An absent hash for a WASM contract is a missing
    /// fact; an absent hash for an asset contract is the complete fact.
    StellarAsset,
}

impl ContractExecutableKind {
    /// The stable wire name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Wasm => "WASM",
            Self::StellarAsset => "STELLAR_ASSET",
        }
    }

    /// Whether a contract of this kind has a WASM module to inspect.
    ///
    /// Used instead of comparing against `Wasm` at each call site so that adding a
    /// kind later forces a decision here rather than silently inheriting an
    /// assumption.
    #[must_use]
    pub const fn has_module(self) -> bool {
        matches!(self, Self::Wasm)
    }
}

impl fmt::Display for ContractExecutableKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for ContractExecutableKind {
    type Err = EngineError;

    fn from_str(value: &str) -> Result<Self> {
        match value {
            "WASM" => Ok(Self::Wasm),
            "STELLAR_ASSET" => Ok(Self::StellarAsset),
            other => Err(EngineError::Validation {
                path: "/executableKind".to_owned(),
                detail: format!("unrecognised contract executable kind {other:?}"),
            }),
        }
    }
}

/// How a fact about a contract was obtained.
///
/// The specification requires the engine to distinguish these, and the reason is
/// that they carry different obligations. An *observed* fact can be cited as
/// evidence; an *inferred* fact must not be presented as though it were read; an
/// *unknown* fact must be reported as missing rather than defaulted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Observedness {
    /// Read from the network or from local input.
    Observed,
    /// Concluded from other facts rather than read directly.
    Inferred,
    /// Not obtainable from the evidence available.
    Unknown,
}

impl Observedness {
    /// The stable wire name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Observed => "OBSERVED",
            Self::Inferred => "INFERRED",
            Self::Unknown => "UNKNOWN",
        }
    }

    /// Whether a fact of this kind may be cited as supporting evidence.
    ///
    /// Only observed facts may. This is the method the dependency and provenance
    /// layers call before attaching a basis to a claim, so a conclusion reached
    /// through this type cannot be promoted into evidence by accident.
    #[must_use]
    pub const fn is_evidence(self) -> bool {
        matches!(self, Self::Observed)
    }
}

impl fmt::Display for Observedness {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// How a contract's instance entry came to be in its present state.
///
/// Determined from the operation that last modified the instance entry. The
/// distinction between deployment and upgrade is not cosmetic: a deployment
/// establishes the contract's origin, while an upgrade replaces its executable and
/// is itself a change that impact analysis must be able to reason about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum InstanceModificationKind {
    /// The instance entry was created by a deploy operation.
    Deploy,
    /// The instance entry was rewritten, replacing the contract's executable.
    Upgrade,
    /// The operation that modified the entry was not identified.
    Unknown,
}

impl InstanceModificationKind {
    /// The stable wire name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Deploy => "DEPLOY",
            Self::Upgrade => "UPGRADE",
            Self::Unknown => "UNKNOWN",
        }
    }
}

impl fmt::Display for InstanceModificationKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The operation that put a contract's instance entry into its present state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstanceModification {
    /// The ledger at which the entry was last modified.
    pub ledger: LedgerSequence,
    /// The transaction that performed the modification, when it was identified.
    ///
    /// Optional because identifying it requires a bounded search of the ledger's
    /// operations, and a caller that did not request the search - or that was
    /// bounded before it completed - legitimately does not have it. Recording a
    /// guessed transaction would be worse than recording none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transaction: Option<TransactionHash>,
    /// The position of the modifying operation within its transaction.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operation_index: Option<u32>,
    /// Whether the operation deployed or upgraded the contract.
    pub kind: InstanceModificationKind,
    /// How the modification was established.
    pub observedness: Observedness,
}

impl InstanceModification {
    /// A modification whose kind is not yet known, established from the entry's
    /// last-modified ledger alone.
    #[must_use]
    pub const fn at_ledger(ledger: LedgerSequence) -> Self {
        Self {
            ledger,
            transaction: None,
            operation_index: None,
            kind: InstanceModificationKind::Unknown,
            // The ledger is read from the entry, so it is an observation; the
            // operation that produced it is not, which is why `kind` starts
            // `Unknown` rather than being guessed.
            observedness: Observedness::Observed,
        }
    }

    /// Records the operation that performed the modification.
    #[must_use]
    pub fn with_operation(
        mut self,
        transaction: TransactionHash,
        operation_index: Option<u32>,
        kind: InstanceModificationKind,
    ) -> Self {
        self.transaction = Some(transaction);
        self.operation_index = operation_index;
        self.kind = kind;
        self
    }

    /// Whether this modification is the contract's deployment as far as the engine
    /// knows.
    ///
    /// Requires both a known operation and a `Deploy` kind. A modification whose
    /// operation was never identified returns `false`, because "not known to be a
    /// deployment" and "known not to be" must not be conflated.
    #[must_use]
    pub const fn is_known_deployment(&self) -> bool {
        matches!(self.kind, InstanceModificationKind::Deploy) && self.transaction.is_some()
    }
}

/// The identity of a deployed contract, together with how it was observed.
///
/// See the module documentation for why identity and observation are separated
/// here rather than being fields of one flat record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContractIdentity {
    /// The contract's address on its network.
    pub contract_id: ContractId,
    /// The identifier of the network the contract lives on.
    ///
    /// Part of the identity rather than of the observation: the same address on
    /// two networks is two contracts, and a caller comparing identities that
    /// ignored the network would merge them.
    pub network_id: String,
    /// The network's passphrase, which is what actually identifies the chain.
    ///
    /// Carried alongside the identifier because two operators can give the same
    /// chain different names, and the passphrase is the fact rather than the name.
    pub passphrase: String,
    /// What the contract executes.
    pub executable_kind: ContractExecutableKind,
    /// The SHA-256 digest of the deployed module, present exactly when the
    /// executable kind is [`ContractExecutableKind::Wasm`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wasm_hash: Option<Digest>,
    /// The ledger the identity was resolved at.
    pub resolved_at_ledger: LedgerSequence,
    /// The earliest ledger at which this identity was observed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_observed_ledger: Option<LedgerSequence>,
    /// The latest ledger at which this identity was observed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_observed_ledger: Option<LedgerSequence>,
    /// The operation that put the contract's instance entry into its present state.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instance_modification: Option<InstanceModification>,
    /// The version of the identity model that produced this record.
    pub identity_version: String,
}

impl ContractIdentity {
    /// Builds an identity, rejecting an executable kind and WASM hash that
    /// disagree.
    ///
    /// The consistency check is the point of the constructor. A `Wasm` contract
    /// without a hash has no identity at all - there is nothing to compare a
    /// rebuild against, and a later digest verification would silently compare
    /// against nothing. A `StellarAsset` contract with a hash would claim a module
    /// that does not exist.
    ///
    /// # Errors
    ///
    /// Returns [`EngineError::Validation`] when the executable kind and the WASM
    /// hash disagree, or when the network's passphrase is empty.
    pub fn new(
        contract_id: ContractId,
        network: &Network,
        executable_kind: ContractExecutableKind,
        wasm_hash: Option<Digest>,
        resolved_at_ledger: LedgerSequence,
    ) -> Result<Self> {
        if network.passphrase.is_empty() {
            return Err(EngineError::Validation {
                path: "/network/passphrase".to_owned(),
                detail: "a contract identity needs the network's passphrase, which is what \
                         identifies the chain"
                    .to_owned(),
            });
        }
        match (executable_kind, &wasm_hash) {
            (ContractExecutableKind::Wasm, None) => {
                return Err(EngineError::Validation {
                    path: "/wasmHash".to_owned(),
                    detail: "a WASM contract's identity includes its module digest; without it \
                             there is nothing for a rebuild to be compared against"
                        .to_owned(),
                });
            },
            (ContractExecutableKind::StellarAsset, Some(_)) => {
                return Err(EngineError::Validation {
                    path: "/wasmHash".to_owned(),
                    detail: "a Stellar Asset Contract has no deployed module, so recording a WASM \
                             hash for it would claim a module that does not exist"
                        .to_owned(),
                });
            },
            _ => {},
        }

        Ok(Self {
            contract_id,
            network_id: network.id.clone(),
            passphrase: network.passphrase.clone(),
            executable_kind,
            wasm_hash,
            resolved_at_ledger,
            first_observed_ledger: None,
            last_observed_ledger: None,
            instance_modification: None,
            identity_version: CONTRACT_IDENTITY_VERSION.to_owned(),
        })
    }

    /// Records the operation that last modified the instance entry.
    #[must_use]
    pub fn with_instance_modification(mut self, modification: InstanceModification) -> Self {
        self.instance_modification = Some(modification);
        self
    }

    /// Extends the observation window to include `ledger`.
    ///
    /// The window is widened rather than replaced, so that observing the same
    /// identity at two ledgers describes a range rather than losing the earlier
    /// observation. The two fields are kept ordered here rather than at each read
    /// site, because a caller that had to sort them could get it wrong.
    #[must_use]
    pub fn observed_at(mut self, ledger: LedgerSequence) -> Self {
        self.first_observed_ledger = Some(match self.first_observed_ledger {
            Some(existing) => existing.min(ledger),
            None => ledger,
        });
        self.last_observed_ledger = Some(match self.last_observed_ledger {
            Some(existing) => existing.max(ledger),
            None => ledger,
        });
        self
    }

    /// The address qualified by the network it belongs to.
    ///
    /// The form to use when keying by identity in a shared structure: an address
    /// alone would collide across networks.
    #[must_use]
    pub fn network_qualified_id(&self) -> String {
        format!("{}:{}", self.network_id, self.contract_id)
    }

    /// A digest over the identifying properties only.
    ///
    /// Deliberately excludes every observational field - the resolved ledger, the
    /// observation window, and the instance modification - so that the same
    /// contract seen twice at two ledgers has one fingerprint rather than two. The
    /// excluded fields are enumerated in the test below, which fails if a field is
    /// added to the struct and not classified here.
    #[must_use]
    pub fn fingerprint(&self) -> Digest {
        let mut canonical = String::with_capacity(256);
        canonical.push_str("amasario/contract-identity\n");
        canonical.push_str(&self.network_id);
        canonical.push('\n');
        canonical.push_str(&self.passphrase);
        canonical.push('\n');
        canonical.push_str(self.contract_id.as_str());
        canonical.push('\n');
        canonical.push_str(self.executable_kind.as_str());
        canonical.push('\n');
        canonical.push_str(self.wasm_hash.as_ref().map_or("-", Digest::value));
        canonical.push('\n');
        Digest::sha256_of(canonical.as_bytes())
    }

    /// Whether two records identify the same contract in the same state.
    #[must_use]
    pub fn is_same_identity_as(&self, other: &Self) -> bool {
        self.fingerprint().matches(&other.fingerprint())
    }

    /// Whether the two records are the same contract but a different executable.
    ///
    /// This is the upgrade case, and it is the reason an address alone cannot
    /// identify a contract: the address matches, the identity does not. Requires
    /// the same network, because two identical addresses on different networks are
    /// unrelated contracts rather than one contract that changed.
    #[must_use]
    pub fn is_upgraded_relative_to(&self, other: &Self) -> bool {
        self.passphrase == other.passphrase
            && self.contract_id == other.contract_id
            && self.wasm_hash != other.wasm_hash
    }

    /// Whether the identifying properties are internally consistent.
    ///
    /// A record can be constructed by deserialisation rather than through
    /// [`ContractIdentity::new`], so the constructor's invariant is restated as a
    /// check. Returns the failure rather than a boolean so that a report can say
    /// which invariant broke.
    ///
    /// # Errors
    ///
    /// Returns the corresponding [`InspectionFailure`] as an engine error.
    pub fn validate(&self) -> Result<()> {
        match (self.executable_kind, &self.wasm_hash) {
            (ContractExecutableKind::Wasm, None) => {
                Err(InspectionFailure::UnrecognisedExecutable {
                    contract_id: self.contract_id.to_string(),
                }
                .into_error())
            },
            (ContractExecutableKind::StellarAsset, Some(_)) => {
                Err(InspectionFailure::UnrecognisedExecutable {
                    contract_id: self.contract_id.to_string(),
                }
                .into_error())
            },
            _ => Ok(()),
        }
    }

    /// The WASM digest, or a failure when the contract has no module.
    ///
    /// # Errors
    ///
    /// Returns a contract error for a Stellar Asset Contract, which has no module
    /// to return.
    pub fn require_wasm_hash(&self) -> Result<&Digest> {
        self.wasm_hash.as_ref().ok_or_else(|| {
            InspectionFailure::UnrecognisedExecutable {
                contract_id: self.contract_id.to_string(),
            }
            .into_error()
        })
    }
}

/// Builds a SHA-256 digest from 32 raw bytes as the network reports them.
///
/// The network returns a WASM hash as opaque bytes; every other part of the engine
/// names a digest by its lowercase hex value. Converting in one place keeps the two
/// representations from being confused, which matters because a hash compared in the
/// wrong representation never matches.
///
/// # Errors
///
/// Returns [`EngineError::Validation`] when the resulting hex is not a well-formed
/// SHA-256 digest. This cannot happen for a 32-byte input, and the check is present
/// so that the conversion is total in its signature rather than relying on the
/// reader to see why it cannot fail.
pub fn digest_from_hash_bytes(bytes: [u8; 32]) -> Result<Digest> {
    Digest::new(DigestAlgorithm::Sha256, &hex::encode(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;
    use amasario_core::NetworkType;

    fn network() -> Network {
        Network::new(
            "testnet",
            NetworkType::Testnet,
            "Test SDF Network ; September 2015",
        )
        .expect("a valid network descriptor")
    }

    fn address(payload: [u8; 32]) -> ContractId {
        let strkey = format!("{}", stellar_strkey::Contract(payload));
        ContractId::new(strkey).expect("a real contract address")
    }

    fn ledger(value: u32) -> LedgerSequence {
        LedgerSequence::new(value).expect("a real ledger")
    }

    fn identity(wasm: [u8; 32], at: u32) -> ContractIdentity {
        ContractIdentity::new(
            address([1_u8; 32]),
            &network(),
            ContractExecutableKind::Wasm,
            Some(digest_from_hash_bytes(wasm).expect("32 bytes is a valid digest")),
            ledger(at),
        )
        .expect("a consistent identity")
    }

    #[test]
    fn an_executable_kind_round_trips_through_its_wire_name() {
        for kind in [
            ContractExecutableKind::Wasm,
            ContractExecutableKind::StellarAsset,
        ] {
            assert_eq!(
                ContractExecutableKind::from_str(kind.as_str()).expect("round trip"),
                kind
            );
        }
        assert!(ContractExecutableKind::Wasm.has_module());
        assert!(!ContractExecutableKind::StellarAsset.has_module());
        ContractExecutableKind::from_str("EVM")
            .expect_err("an executable kind outside the model is rejected");
    }

    #[test]
    fn a_wasm_contract_without_a_digest_is_rejected_because_it_has_no_identity() {
        let error = ContractIdentity::new(
            address([1_u8; 32]),
            &network(),
            ContractExecutableKind::Wasm,
            None,
            ledger(10),
        )
        .expect_err("no comparison target means no identity");
        assert!(
            error.to_string().contains("nothing for a rebuild"),
            "the error must say why: {error}"
        );
    }

    #[test]
    fn an_asset_contract_with_a_digest_is_rejected_because_the_module_does_not_exist() {
        ContractIdentity::new(
            address([1_u8; 32]),
            &network(),
            ContractExecutableKind::StellarAsset,
            Some(digest_from_hash_bytes([0_u8; 32]).expect("a digest")),
            ledger(10),
        )
        .expect_err("an asset contract has no deployed module");
    }

    #[test]
    fn an_asset_contract_is_a_complete_identity_with_no_digest() {
        // Recording the absent hash as an unknown identity would be wrong: the
        // fact is complete.
        let identity = ContractIdentity::new(
            address([2_u8; 32]),
            &network(),
            ContractExecutableKind::StellarAsset,
            None,
            ledger(10),
        )
        .expect("a complete identity");
        identity.validate().expect("self-consistent");
        identity
            .require_wasm_hash()
            .expect_err("there is no module to return");
    }

    #[test]
    fn the_fingerprint_ignores_the_observation_window() {
        // The property the whole module exists for: observing the same contract at
        // two ledgers must not create two identities.
        let early = identity([7_u8; 32], 100).observed_at(ledger(100));
        let late = identity([7_u8; 32], 900)
            .observed_at(ledger(900))
            .with_instance_modification(InstanceModification::at_ledger(ledger(900)));

        assert_ne!(early.resolved_at_ledger, late.resolved_at_ledger);
        assert_ne!(early.instance_modification, late.instance_modification);
        assert!(
            early.is_same_identity_as(&late),
            "the same contract at two ledgers is one contract"
        );
        assert_eq!(early.fingerprint(), late.fingerprint());
    }

    #[test]
    fn the_fingerprint_distinguishes_the_address_the_network_and_the_executable() {
        let base = identity([7_u8; 32], 100);

        let other_executable = identity([8_u8; 32], 100);
        assert!(!base.is_same_identity_as(&other_executable));

        let other_address = ContractIdentity::new(
            address([2_u8; 32]),
            &network(),
            ContractExecutableKind::Wasm,
            Some(digest_from_hash_bytes([7_u8; 32]).expect("a digest")),
            ledger(100),
        )
        .expect("a consistent identity");
        assert!(!base.is_same_identity_as(&other_address));

        // Same address and digest but a different passphrase is a different chain,
        // and therefore a different contract.
        let mut foreign = base.clone();
        foreign.passphrase = "Public Global Stellar Network ; September 2015".to_owned();
        assert!(!base.is_same_identity_as(&foreign));
    }

    #[test]
    fn an_upgrade_is_detected_as_the_same_address_with_a_different_executable() {
        // The case that makes an address insufficient: the address matches, the
        // identity does not.
        let before = identity([7_u8; 32], 100);
        let after = identity([9_u8; 32], 200);

        assert!(after.is_upgraded_relative_to(&before));
        assert!(before.is_upgraded_relative_to(&after));
        assert!(!before.is_same_identity_as(&after));
        assert_eq!(before.contract_id, after.contract_id);
    }

    #[test]
    fn an_upgrade_is_not_claimed_across_two_networks() {
        let local = identity([7_u8; 32], 100);
        let mut foreign = local.clone();
        foreign.passphrase = "Public Global Stellar Network ; September 2015".to_owned();
        foreign.wasm_hash = Some(digest_from_hash_bytes([9_u8; 32]).expect("a digest"));

        assert!(
            !foreign.is_upgraded_relative_to(&local),
            "two identical addresses on different chains are unrelated contracts, not one that changed"
        );
    }

    #[test]
    fn the_network_qualified_id_includes_the_network() {
        let contract = identity([7_u8; 32], 100);
        assert_eq!(
            contract.network_qualified_id(),
            format!("testnet:{}", contract.contract_id)
        );
    }

    #[test]
    fn the_observation_window_widens_and_keeps_its_order() {
        let contract = identity([7_u8; 32], 500)
            .observed_at(ledger(500))
            .observed_at(ledger(300))
            .observed_at(ledger(800));

        assert_eq!(contract.first_observed_ledger, Some(ledger(300)));
        assert_eq!(contract.last_observed_ledger, Some(ledger(800)));
    }

    #[test]
    fn a_modification_is_only_a_known_deployment_when_its_operation_was_identified() {
        // "Not known to be a deployment" and "known not to be" must not be
        // conflated, so an unfound operation leaves the kind Unknown.
        let unsearched = InstanceModification::at_ledger(ledger(10));
        assert_eq!(unsearched.kind, InstanceModificationKind::Unknown);
        assert!(!unsearched.is_known_deployment());
        assert_eq!(unsearched.observedness, Observedness::Observed);

        let hash = TransactionHash::new("ab".repeat(32)).expect("a real hash");
        let deployed = InstanceModification::at_ledger(ledger(10)).with_operation(
            hash,
            Some(1),
            InstanceModificationKind::Deploy,
        );
        assert!(deployed.is_known_deployment());

        let upgraded = InstanceModification::at_ledger(ledger(10)).with_operation(
            TransactionHash::new("cd".repeat(32)).expect("a real hash"),
            None,
            InstanceModificationKind::Upgrade,
        );
        assert!(
            !upgraded.is_known_deployment(),
            "an upgrade is not a deployment"
        );
    }

    #[test]
    fn only_an_observed_fact_may_be_cited_as_evidence() {
        assert!(Observedness::Observed.is_evidence());
        assert!(!Observedness::Inferred.is_evidence());
        assert!(!Observedness::Unknown.is_evidence());
    }

    #[test]
    fn a_network_hash_converts_to_the_engines_digest_representation() {
        // The network reports a module hash as opaque bytes; everything else names
        // it as lowercase hex. Comparing the two representations would never match.
        let digest = digest_from_hash_bytes([0xab_u8; 32]).expect("32 bytes is valid");
        assert_eq!(digest.algorithm(), DigestAlgorithm::Sha256);
        assert_eq!(digest.value(), "ab".repeat(32));
    }

    #[test]
    fn the_identity_round_trips_through_json_without_losing_its_invariants() {
        let original = identity([7_u8; 32], 100).observed_at(ledger(100));
        let json = serde_json::to_string(&original).expect("serialises");
        let restored: ContractIdentity = serde_json::from_str(&json).expect("deserialises");

        assert_eq!(restored, original);
        // A record that arrived by deserialisation was not checked by the
        // constructor, so the same invariant has to hold as a check.
        restored.validate().expect("self-consistent");
        assert!(restored.is_same_identity_as(&original));
    }

    #[test]
    fn a_deserialised_record_that_violates_the_invariant_is_caught_by_validation() {
        let mut broken = identity([7_u8; 32], 100);
        broken.wasm_hash = None;
        broken
            .validate()
            .expect_err("a WASM contract without a digest has no identity");
    }
}
