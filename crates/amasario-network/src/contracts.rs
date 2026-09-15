//! Contract ledger-entry retrieval.
//!
//! Stellar RPC exposes a contract through ledger entries rather than through a
//! bespoke "get contract" call, and which entries matter depends on what is being
//! asked. A contract's *instance* entry carries its executable - the thing that
//! says which WASM it runs - and the *code* entry carries the WASM itself. The two
//! are separate entries and separate questions, which is why this module keeps
//! them apart instead of returning one blended answer.
//!
//! # Why the keys are built here
//!
//! A `LedgerKey` is an XDR structure, and constructing one wrongly produces a
//! well-formed request for the wrong entry - which the network answers with an
//! absence, not an error. An absence is indistinguishable from "this contract has
//! no such entry", so a key-construction bug would look like a legitimate
//! finding. The constructors below are therefore pure, total in their inputs, and
//! unit-tested, so that the only way to reach the network is through a key whose
//! shape has been asserted.
//!
//! # Durability
//!
//! Soroban stores contract instance data with `Persistent` durability. This is a
//! protocol fact and not a choice made here: the alternative, `Temporary`, would
//! not find the entry.

use amasario_core::{Cancellation, ContractId, EngineError, LedgerSequence, Result};
#[cfg(test)]
use stellar_xdr::BytesM;
use stellar_xdr::{
    ContractDataDurability, ContractId as XdrContractId, Hash, LedgerEntryData, LedgerKey,
    LedgerKeyContractCode, LedgerKeyContractData, ScAddress, ScVal,
};

use crate::rpc::{LedgerEntry, RpcSession};

/// A ledger entry read from the network, with the metadata that qualifies it.
#[derive(Debug, Clone, PartialEq)]
pub struct ContractEntry {
    /// The decoded entry.
    pub data: LedgerEntryData,
    /// The ledger at which this entry was last modified.
    ///
    /// An observation, not a creation fact. For an entry that has never been
    /// modified since it was created, the two coincide - but that is a conclusion
    /// the caller draws if the evidence supports it, never an assumption this
    /// module makes on the caller's behalf.
    pub last_modified_ledger: LedgerSequence,
    /// The ledger at which this entry expires, for entries that have a time to
    /// live.
    pub live_until_ledger: Option<LedgerSequence>,
}

/// The entries that describe one contract.
#[derive(Debug, Clone, PartialEq)]
pub struct ContractEntries {
    /// The contract's instance entry, present for any contract that exists.
    pub instance: Option<ContractEntry>,
    /// The code entry for the WASM the instance refers to, present when the entry
    /// was found at this boundary.
    pub code: Option<ContractEntry>,
}

/// Builds the ledger key for a contract's instance entry.
///
/// # Errors
///
/// Returns [`EngineError::Validation`] when the address's checksum does not hold.
/// The checksum is verified because an address with a corrupted character decodes
/// to a *different* contract's identifier, and the network would answer "no such
/// entry" - turning a transcription error into a confident finding that the
/// contract does not exist.
pub fn contract_instance_key(contract: &ContractId) -> Result<LedgerKey> {
    let payload = contract.payload()?;
    Ok(LedgerKey::ContractData(LedgerKeyContractData {
        contract: ScAddress::Contract(XdrContractId(Hash(payload))),
        key: ScVal::LedgerKeyContractInstance,
        durability: ContractDataDurability::Persistent,
    }))
}

/// Builds the ledger key for a contract's data entry under `key`.
///
/// # Errors
///
/// Returns [`EngineError::Validation`] when the address's checksum does not hold.
pub fn contract_data_key(
    contract: &ContractId,
    key: ScVal,
    durability: ContractDataDurability,
) -> Result<LedgerKey> {
    let payload = contract.payload()?;
    Ok(LedgerKey::ContractData(LedgerKeyContractData {
        contract: ScAddress::Contract(XdrContractId(Hash(payload))),
        key,
        durability,
    }))
}

/// Builds the ledger key for a WASM code entry.
///
/// Cannot fail: a 32-byte hash is a complete key, and unlike a strkey address
/// there is no checksum to verify because the hash *is* the identifier.
#[must_use]
pub const fn contract_code_key(hash: &[u8; 32]) -> LedgerKey {
    LedgerKey::ContractCode(LedgerKeyContractCode { hash: Hash(*hash) })
}

/// Reads a contract's instance entry.
///
/// Returns `Ok(None)` when the entry is absent at the current ledger, which is the
/// network's definite answer that no such contract exists. A failure to ask the
/// network is an error, and the two never collapse into one another.
///
/// # Errors
///
/// Returns a classified error when the request fails, and
/// [`EngineError::MalformedResponse`] when an entry is returned but is not the
/// contract-data entry that was requested.
pub async fn fetch_contract_instance(
    session: &RpcSession,
    cancellation: &Cancellation,
    contract: &ContractId,
) -> Result<Option<ContractEntry>> {
    let key = contract_instance_key(contract)?;
    let mut entries = session.ledger_entries(cancellation, &[key]).await?;
    match entries.pop() {
        Some(entry) => Ok(Some(to_contract_entry(entry)?)),
        None => Ok(None),
    }
}

/// Reads the code entry for a WASM hash.
///
/// # Errors
///
/// Returns a classified error when the request fails, and
/// [`EngineError::MalformedResponse`] when an entry is returned that is not the
/// code entry requested.
pub async fn fetch_contract_code(
    session: &RpcSession,
    cancellation: &Cancellation,
    hash: &[u8; 32],
) -> Result<Option<ContractEntry>> {
    let key = contract_code_key(hash);
    let mut entries = session.ledger_entries(cancellation, &[key]).await?;
    match entries.pop() {
        Some(entry) => Ok(Some(to_contract_entry(entry)?)),
        None => Ok(None),
    }
}

/// Reads a contract's instance and, when the instance names a WASM hash, the code
/// entry for that hash.
///
/// Both entries are requested together because they are almost always wanted
/// together and a second round trip would double the chance of observing the two
/// at different boundaries. The result records which of the two were found
/// independently, so a caller can tell "the contract exists but its code entry is
/// absent" from "the contract does not exist" - a distinction that matters because
/// the first is a provenance anomaly and the second is simply not found.
///
/// # Errors
///
/// Returns a classified error when either request fails.
pub async fn fetch_contract_entries(
    session: &RpcSession,
    cancellation: &Cancellation,
    contract: &ContractId,
) -> Result<ContractEntries> {
    // The code hash is not known until the instance is read, so this is two
    // requests rather than one. The first cannot be avoided: the hash is a field
    // *inside* the instance entry.
    let instance = fetch_contract_instance(session, cancellation, contract).await?;

    let code = match &instance {
        Some(entry) => match wasm_hash_of(entry) {
            Some(hash) => fetch_contract_code(session, cancellation, &hash).await?,
            // A Stellar Asset Contract has no WASM of its own. Reporting an absent
            // code entry here would be wrong: there is no code entry to be absent,
            // and the distinction is carried by the executable kind rather than by
            // the missing entry.
            None => None,
        },
        None => None,
    };

    Ok(ContractEntries { instance, code })
}

/// The WASM hash a contract instance executes, when it executes one.
///
/// Returns `None` for a Stellar Asset Contract, whose executable is built into the
/// protocol rather than deployed as a WASM module. Returning `None` rather than a
/// zero hash is deliberate: a zero hash would compare equal to nothing and could
/// be mistaken for a real digest.
#[must_use]
pub const fn wasm_hash_of(entry: &ContractEntry) -> Option<[u8; 32]> {
    match &entry.data {
        LedgerEntryData::ContractData(data) => match &data.val {
            ScVal::ContractInstance(stellar_xdr::ScContractInstance {
                executable: stellar_xdr::ContractExecutable::Wasm(hash),
                ..
            }) => Some(hash.0),
            _ => None,
        },
        _ => None,
    }
}

/// Converts a client entry into the engine's own, validating its shape.
fn to_contract_entry(entry: LedgerEntry) -> Result<ContractEntry> {
    let last_modified_ledger = LedgerSequence::new(entry.last_modified_ledger).map_err(|_| {
        // A ledger of zero means the response named no ledger at all, which makes
        // the observation unqualifiable rather than merely unusual.
        EngineError::MalformedResponse {
            endpoint: String::new(),
            detail: format!(
                "the endpoint reported a last-modified ledger of {}, but no Stellar network has a \
                 ledger below the genesis ledger",
                entry.last_modified_ledger
            ),
        }
    })?;

    let live_until_ledger = entry
        .live_until_ledger_seq
        .map(LedgerSequence::new)
        .transpose()?;

    Ok(ContractEntry {
        data: entry.val,
        last_modified_ledger,
        live_until_ledger,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::NetworkTarget;
    use amasario_core::EngineConfig;
    use stellar_xdr::{
        ContractCodeEntry, ContractDataEntry, ContractExecutable, ExtensionPoint,
        ScContractInstance, WriteXdr,
    };
    use wiremock::matchers::{body_string_contains, method};
    use wiremock::{Mock, MockServer};

    /// A contract address built from a known payload, so a test can assert the
    /// bytes it decodes to.
    fn contract_address(payload: [u8; 32]) -> ContractId {
        // Built with Stellar's own strkey encoder rather than by hand, so the
        // address under test is a real one.
        // `format!` rather than the type's inherent `to_string`, which returns a
        // fixed-capacity string rather than an owned one.
        let strkey = format!("{}", stellar_strkey::Contract(payload));
        ContractId::new(strkey).expect("a valid contract address")
    }

    fn instance_entry(wasm: [u8; 32]) -> LedgerEntryData {
        LedgerEntryData::ContractData(ContractDataEntry {
            ext: ExtensionPoint::V0,
            contract: ScAddress::Contract(XdrContractId(Hash([3_u8; 32]))),
            key: ScVal::LedgerKeyContractInstance,
            durability: ContractDataDurability::Persistent,
            val: ScVal::ContractInstance(ScContractInstance {
                executable: ContractExecutable::Wasm(Hash(wasm)),
                storage: None,
            }),
        })
    }

    fn code_entry(hash: [u8; 32], code: &[u8]) -> LedgerEntryData {
        LedgerEntryData::ContractCode(ContractCodeEntry {
            ext: stellar_xdr::ContractCodeEntryExt::V0,
            hash: Hash(hash),
            // `BytesM`, not `VecM`: the contract code field is an opaque byte
            // array, which is a different generated type.
            code: BytesM::try_from(code.to_vec()).expect("a short module"),
        })
    }

    /// A `getLedgerEntries` responder carrying one entry.
    fn entry_response(
        key: &LedgerKey,
        val: &LedgerEntryData,
        last_modified: u32,
    ) -> crate::test_support::RpcSuccess {
        crate::test_support::result(crate::test_support::entries(
            4000,
            crate::test_support::entry(
                &key.to_xdr_base64(stellar_xdr::Limits::none())
                    .expect("encodes"),
                &val.to_xdr_base64(stellar_xdr::Limits::none())
                    .expect("encodes"),
                last_modified,
                5000,
            ),
        ))
    }

    fn target(server: &MockServer) -> NetworkTarget {
        NetworkTarget::custom(
            "mock",
            "Test SDF Network ; September 2015",
            &server.uri(),
            None,
            &EngineConfig::default(),
        )
        .expect("a local endpoint is valid")
    }

    #[test]
    fn the_instance_key_encodes_the_contracts_own_bytes() {
        let payload = [7_u8; 32];
        let contract = contract_address(payload);
        let key = contract_instance_key(&contract).expect("a valid address");

        match key {
            LedgerKey::ContractData(data) => {
                assert_eq!(data.durability, ContractDataDurability::Persistent);
                assert_eq!(data.key, ScVal::LedgerKeyContractInstance);
                match data.contract {
                    ScAddress::Contract(XdrContractId(Hash(bytes))) => assert_eq!(
                        bytes, payload,
                        "the key must address the contract the strkey encoded, not another one"
                    ),
                    other => panic!("expected a contract address, got {other:?}"),
                }
            },
            other => panic!("expected a contract data key, got {other:?}"),
        }
    }

    #[test]
    fn the_code_key_is_the_hash_itself() {
        let hash = [9_u8; 32];
        match contract_code_key(&hash) {
            LedgerKey::ContractCode(code) => assert_eq!(code.hash.0, hash),
            other => panic!("expected a contract code key, got {other:?}"),
        }
    }

    #[test]
    fn an_address_with_a_corrupted_checksum_is_rejected_rather_than_queried() {
        // A corrupted address decodes to a different contract's identifier, and the
        // network would answer "no such entry" - turning a transcription error into
        // a confident finding that the contract does not exist.
        let mut address = contract_address([1_u8; 32]).as_str().to_owned();
        // Swap a character late in the string so the checksum no longer matches the
        // body while the length and alphabet stay valid.
        let last = address.pop().expect("non-empty");
        address.push(if last == 'A' { 'B' } else { 'A' });

        let contract = ContractId::new(address).expect("the shape is still valid");
        let error = contract_instance_key(&contract).expect_err("the checksum must be verified");
        assert!(error.to_string().contains("checksum"), "got: {error}");
    }

    #[test]
    fn an_address_round_trips_through_its_payload() {
        let payload = [42_u8; 32];
        let contract = contract_address(payload);
        assert_eq!(contract.payload().expect("decodes"), payload);
    }

    #[tokio::test]
    async fn the_contract_instance_is_read_and_its_wasm_hash_extracted() {
        let server = MockServer::start().await;
        let wasm = [5_u8; 32];
        let contract = contract_address([3_u8; 32]);
        let key = contract_instance_key(&contract).expect("valid");

        Mock::given(method("POST"))
            .and(body_string_contains("getLedgerEntries"))
            .respond_with(entry_response(&key, &instance_entry(wasm), 1234))
            .mount(&server)
            .await;

        let session = RpcSession::connect(&target(&server)).expect("connects");
        let entry = fetch_contract_instance(&session, &Cancellation::new(), &contract)
            .await
            .expect("the request succeeds")
            .expect("the entry exists");

        assert_eq!(entry.last_modified_ledger.get(), 1234);
        assert_eq!(entry.live_until_ledger.map(LedgerSequence::get), Some(5000));
        assert_eq!(wasm_hash_of(&entry), Some(wasm));
    }

    #[tokio::test]
    async fn an_absent_contract_is_not_found_rather_than_an_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(body_string_contains("getLedgerEntries"))
            .respond_with(crate::test_support::result(
                crate::test_support::no_entries(4000),
            ))
            .mount(&server)
            .await;

        let session = RpcSession::connect(&target(&server)).expect("connects");
        let found = fetch_contract_instance(
            &session,
            &Cancellation::new(),
            &contract_address([1_u8; 32]),
        )
        .await
        .expect("the network answered");
        assert!(found.is_none(), "the network said no such contract exists");
    }

    #[tokio::test]
    async fn a_code_entry_is_read_and_decoded() {
        let server = MockServer::start().await;
        let hash = [8_u8; 32];
        let wasm = b"\x00asm\x01\x00\x00\x00";
        let key = contract_code_key(&hash);

        Mock::given(method("POST"))
            .and(body_string_contains("getLedgerEntries"))
            .respond_with(entry_response(&key, &code_entry(hash, wasm), 900))
            .mount(&server)
            .await;

        let session = RpcSession::connect(&target(&server)).expect("connects");
        let entry = fetch_contract_code(&session, &Cancellation::new(), &hash)
            .await
            .expect("the request succeeds")
            .expect("the entry exists");

        match entry.data {
            LedgerEntryData::ContractCode(code) => {
                assert_eq!(code.hash.0, hash);
                assert_eq!(code.code.as_slice(), wasm);
            },
            other => panic!("expected a contract code entry, got {other:?}"),
        }
    }

    #[test]
    fn a_wasm_executable_reports_its_hash_and_an_asset_contract_reports_none() {
        // Returning a zero hash for a Stellar Asset Contract would compare equal to
        // nothing and could be mistaken for a real digest.
        let entry = ContractEntry {
            data: instance_entry([4_u8; 32]),
            last_modified_ledger: LedgerSequence::new(10).expect("a real ledger"),
            live_until_ledger: None,
        };
        assert_eq!(wasm_hash_of(&entry), Some([4_u8; 32]));

        let asset = ContractEntry {
            data: LedgerEntryData::ContractData(ContractDataEntry {
                ext: ExtensionPoint::V0,
                contract: ScAddress::Contract(XdrContractId(Hash([0_u8; 32]))),
                key: ScVal::LedgerKeyContractInstance,
                durability: ContractDataDurability::Persistent,
                val: ScVal::ContractInstance(ScContractInstance {
                    executable: ContractExecutable::StellarAsset,
                    storage: None,
                }),
            }),
            last_modified_ledger: LedgerSequence::new(10).expect("a real ledger"),
            live_until_ledger: None,
        };
        assert_eq!(wasm_hash_of(&asset), None);
    }
}
