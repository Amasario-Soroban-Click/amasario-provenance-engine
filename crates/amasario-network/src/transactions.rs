//! Transaction observations.
//!
//! A transaction is where the engine's dependency evidence is strongest. A
//! deployment transaction whose envelope contains an `InvokeHostFunction` operation
//! naming contract `C` is *observed* to invoke `C`; that is a fact about the chain,
//! not an inference from two projects seeming related. The specification requires a
//! dependency to have a basis and evidence, and this module is where the
//! transaction-shaped evidence comes from.
//!
//! # What is extracted, and what is not
//!
//! [`invoked_contracts`] returns the contract addresses a transaction names,
//! collected from two independent places: the host functions in the envelope, and
//! the contract events in the result metadata. Both are direct readings of the
//! transaction. Nothing here infers a relationship from a name, a topic, or a
//! convention.
//!
//! The result is sorted and deduplicated, because two runs over the same
//! transaction must produce byte-identical output for the specification's
//! determinism requirement to hold.

use std::collections::BTreeSet;

use amasario_core::{Cancellation, EngineError, LedgerSequence, Result, TransactionHash};
use stellar_xdr::{
    ContractEvent, DiagnosticEvent, Hash, HostFunction, LedgerEntryData, Operation, OperationBody,
    ScAddress, TransactionEnvelope, TransactionEvent, TransactionMeta, TransactionResult,
};

use crate::rpc::RpcSession;

/// A transaction read from the network.
#[derive(Debug, Clone, PartialEq)]
pub struct TransactionObservation {
    /// The transaction hash, lowercase hex.
    pub hash: String,
    /// The ledger the transaction was included in, when the endpoint reported one.
    pub ledger: Option<LedgerSequence>,
    /// The endpoint's status word for the transaction.
    pub status: String,
    /// Whether the network reports the transaction as having succeeded.
    ///
    /// Derived from the endpoint's status rather than from the result XDR, and the
    /// raw result is preserved alongside so that a caller needing more than a
    /// boolean does not have to re-read the transaction. A successful transaction
    /// is not a *correct* or *safe* one: this is a statement about the ledger, and
    /// the engine attaches no other meaning to it.
    pub successful: bool,
    /// The position of the transaction within its ledger.
    pub application_order: Option<u32>,
    /// The decoded envelope.
    pub envelope: Option<TransactionEnvelope>,
    /// The decoded result.
    pub result: Option<TransactionResult>,
    /// The decoded result metadata, which carries the contract events.
    pub result_meta: Option<TransactionMeta>,
    /// Contract events emitted by the transaction, grouped by operation.
    pub contract_events: Vec<Vec<ContractEvent>>,
    /// Transaction-level events.
    pub transaction_events: Vec<TransactionEvent>,
    /// Diagnostic events, which describe execution rather than outcome.
    pub diagnostic_events: Vec<DiagnosticEvent>,
}

/// Reads a transaction by hash.
///
/// Returns `Ok(None)` when the network reports the transaction as absent. That is a
/// definite answer about the chain, and it is kept distinct from a failure to ask.
///
/// # Errors
///
/// Returns [`EngineError::Validation`] when the hash is not a 32-byte hexadecimal
/// value, and a classified error when the request fails.
pub async fn fetch_transaction(
    session: &RpcSession,
    cancellation: &Cancellation,
    hash: &TransactionHash,
) -> Result<Option<TransactionObservation>> {
    let bytes = decode_hash(hash)?;
    let Some(response) = session.transaction(cancellation, &bytes).await? else {
        return Ok(None);
    };

    let ledger = response.ledger.map(LedgerSequence::new).transpose()?;
    let successful = response.status.eq_ignore_ascii_case("SUCCESS");
    let diagnostic_events = diagnostic_events_of(
        response.result_meta.as_ref(),
        &response.events.diagnostic_events,
    );

    Ok(Some(TransactionObservation {
        hash: response.tx_hash.unwrap_or_else(|| hash.as_str().to_owned()),
        ledger,
        status: response.status,
        successful,
        application_order: response.application_order,
        envelope: response.envelope,
        result: response.result,
        result_meta: response.result_meta,
        contract_events: response.events.contract_events,
        transaction_events: response.events.transaction_events,
        diagnostic_events,
    }))
}

/// The diagnostic events a transaction carries.
///
/// Read from the transaction's own metadata, with the endpoint's accessor as a fallback
/// only. The fallback is not the primary source, and the reason is worth recording
/// because it is invisible from this side of the API and cost the engine every nested
/// call it should have recovered.
///
/// On the current protocol the metadata is `TransactionMeta::V4`, whose diagnostic
/// events live in `meta.diagnostic_events`. A node also publishes the same events as a
/// top-level `diagnosticEventsXdr` on the transaction response. The bundled RPC client
/// reads a *third* place - a `diagnosticEventsXdr` nested inside the response's `events`
/// object - which the node does not populate: measured against testnet, a successful
/// transaction returned 49 diagnostic events at the top level, an `events` object
/// holding only `contractEventsXdr` and `transactionEventsXdr`, and an empty list from
/// the client's own accessor.
///
/// So a reader that trusts that accessor sees no diagnostic events on any transaction,
/// recovers no call nesting, and reports every contract as having no dependencies. The
/// metadata is checked first and the fallback is used only when the metadata yielded
/// nothing, so an endpoint or client that populates the other path is still read.
fn diagnostic_events_of(
    meta: Option<&TransactionMeta>,
    endpoint_events: &[DiagnosticEvent],
) -> Vec<DiagnosticEvent> {
    let from_meta: Vec<DiagnosticEvent> = match meta {
        Some(TransactionMeta::V4(v4)) => v4.diagnostic_events.to_vec(),
        Some(TransactionMeta::V3(v3)) => v3
            .soroban_meta
            .as_ref()
            .map(|soroban| soroban.diagnostic_events.to_vec())
            .unwrap_or_default(),
        // V0, V1 and V2 predate Soroban, so a transaction under them has no host
        // diagnostics to lose.
        _ => Vec::new(),
    };

    if from_meta.is_empty() {
        endpoint_events.to_vec()
    } else {
        from_meta
    }
}

/// The contract addresses a transaction names, in a stable order.
///
/// Collected from the envelope's host functions and from the emitted contract
/// events. Both are direct readings; a contract that is merely *mentioned* by
/// another entity's metadata does not appear here, which is what keeps an
/// association from being promoted to a dependency.
///
/// Addresses are returned as strkeys because that is how the rest of the engine
/// names a contract, and because a strkey carries its own checksum: a corrupted
/// byte becomes an invalid address rather than a valid-looking one for a different
/// contract.
#[must_use]
pub fn invoked_contracts(observation: &TransactionObservation) -> Vec<String> {
    let mut addresses: BTreeSet<String> = BTreeSet::new();

    if let Some(envelope) = &observation.envelope {
        for operation in envelope_operations(envelope) {
            // The body carries the operation kind; `Operation` itself is a struct
            // holding an optional source account and that body.
            if let OperationBody::InvokeHostFunction(invoke) = &operation.body {
                collect_from_host_function(&invoke.host_function, &mut addresses);
            }
        }
    }

    for events in &observation.contract_events {
        for event in events {
            if let Some(id) = &event.contract_id {
                addresses.insert(contract_strkey_of(id));
            }
        }
    }

    addresses.into_iter().collect()
}

/// The operations in an envelope, for either transaction kind.
///
/// A fee-bump envelope wraps the inner transaction, and its operations are the ones
/// that ran, so both forms are unwrapped rather than only the plain one.
#[must_use]
pub fn envelope_operations(envelope: &TransactionEnvelope) -> Vec<&Operation> {
    let operations = match envelope {
        TransactionEnvelope::Tx(v1) => &v1.tx.operations,
        TransactionEnvelope::TxV0(v0) => &v0.tx.operations,
        TransactionEnvelope::TxFeeBump(fee_bump) => match &fee_bump.tx.inner_tx {
            stellar_xdr::FeeBumpTransactionInnerTx::Tx(v1) => &v1.tx.operations,
        },
    };
    operations.iter().collect()
}

/// Records the contract addresses a host function names.
fn collect_from_host_function(function: &HostFunction, addresses: &mut BTreeSet<String>) {
    match function {
        HostFunction::InvokeContract(invoke) => {
            if let Some(address) = contract_strkey(&invoke.contract_address) {
                addresses.insert(address);
            }
        },
        // A creation names its target through a preimage, and only the address form
        // names a contract that can be looked up; a wasm-hash preimage produces an
        // address that is *derived* from the deployer, which the engine reports as
        // a deployment rather than guessing at here.
        HostFunction::CreateContractV2(create) => {
            if let stellar_xdr::ContractIdPreimage::Address(
                stellar_xdr::ContractIdPreimageFromAddress { address, .. },
            ) = &create.contract_id_preimage
                && let Some(value) = contract_strkey(address)
            {
                addresses.insert(value);
            }
        },
        // Uploading a module creates no contract, and a `CreateContract` preimage
        // is covered by its own case above where an address is available.
        HostFunction::UploadContractWasm(_) | HostFunction::CreateContract(_) => {},
    }
}

/// Encodes a contract address as a strkey, when it is one.
///
/// Returns `None` for an account address rather than its `G...` encoding: the rest
/// of the engine names contracts by strkey, and a `G...` value where a `C...` is
/// expected would be a category error rather than merely an unexpected value.
#[must_use]
pub fn contract_strkey(address: &ScAddress) -> Option<String> {
    match address {
        ScAddress::Contract(contract) => Some(contract_strkey_of(contract)),
        // Listed rather than caught by a wildcard, so that a future address kind
        // forces a decision here instead of being silently classified as
        // "not a contract".
        ScAddress::Account(_)
        | ScAddress::MuxedAccount(_)
        | ScAddress::ClaimableBalance(_)
        | ScAddress::LiquidityPool(_) => None,
    }
}

/// Encodes a contract identifier as a strkey.
///
/// Uses `format!` rather than `to_string`, because `stellar-strkey` provides an
/// inherent `to_string` that returns a fixed-capacity string. Naming `Display`
/// explicitly is what produces an ordinary owned `String`, and it is the same
/// encoding either way.
#[must_use]
fn contract_strkey_of(contract: &stellar_xdr::ContractId) -> String {
    format!("{}", stellar_strkey::Contract(contract.0.0))
}

/// Decodes a hexadecimal transaction hash into its 32 bytes.
fn decode_hash(hash: &TransactionHash) -> Result<Hash> {
    let bytes = hex::decode(hash.as_str()).map_err(|error| EngineError::Validation {
        path: "/transactionHash".to_owned(),
        // Unreachable through `TransactionHash`, which validates its own encoding,
        // and kept so that the conversion cannot silently produce a short hash if
        // that validation is ever relaxed.
        detail: format!("{hash} is not hexadecimal: {error}"),
    })?;
    let array: [u8; 32] = bytes.try_into().map_err(|_| EngineError::Validation {
        path: "/transactionHash".to_owned(),
        detail: format!("{hash} does not decode to 32 bytes"),
    })?;
    Ok(Hash(array))
}

/// The contract entries a transaction's metadata created, when it created any.
///
/// Used to answer "which contract did this transaction deploy", which is a
/// question about the ledger changes rather than about the envelope: an envelope
/// can *ask* to create a contract, and only the metadata says whether one exists.
#[must_use]
pub fn created_contract_entries(observation: &TransactionObservation) -> Vec<&LedgerEntryData> {
    let Some(TransactionMeta::V4(meta)) = &observation.result_meta else {
        // Earlier metadata versions do not carry the ledger changes in a form this
        // accessor can read. Returning nothing is correct: the engine does not
        // guess at a creation the metadata did not report.
        return Vec::new();
    };

    meta.operations
        .iter()
        .flat_map(|operation| operation.changes.iter())
        .filter_map(|change| match change {
            stellar_xdr::LedgerEntryChange::Created(entry) => Some(&entry.data),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::NetworkTarget;
    use amasario_core::EngineConfig;
    use stellar_xdr::{
        ContractDataDurability, ContractDataEntry, ContractId as XdrContractId, ExtensionPoint,
        ReadXdr, ScVal, WriteXdr,
    };
    use wiremock::matchers::{body_string_contains, method};
    use wiremock::{Mock, MockServer};

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

    fn observation(envelope: Option<TransactionEnvelope>) -> TransactionObservation {
        TransactionObservation {
            hash: "00".repeat(32),
            ledger: LedgerSequence::new(100).ok(),
            status: "SUCCESS".to_owned(),
            successful: true,
            application_order: Some(1),
            envelope,
            result: None,
            result_meta: None,
            contract_events: Vec::new(),
            transaction_events: Vec::new(),
            diagnostic_events: Vec::new(),
        }
    }

    #[test]
    fn a_contract_address_is_encoded_as_the_strkey_the_rest_of_the_engine_uses() {
        let payload = [1_u8; 32];
        let address = ScAddress::Contract(XdrContractId(Hash(payload)));
        let encoded = contract_strkey(&address).expect("a contract address encodes");
        assert_eq!(encoded, format!("{}", stellar_strkey::Contract(payload)));
        assert!(encoded.starts_with('C'));
    }

    #[test]
    fn an_account_address_names_no_contract() {
        // Returning the account's strkey here would put a `G...` value where the
        // rest of the engine expects a contract, and the two are not
        // interchangeable.
        let account = ScAddress::Account(stellar_xdr::AccountId(
            stellar_xdr::PublicKey::PublicKeyTypeEd25519(stellar_xdr::Uint256([0_u8; 32])),
        ));
        assert_eq!(contract_strkey(&account), None);
    }

    #[test]
    fn a_hash_that_is_not_hexadecimal_is_rejected() {
        let hash = TransactionHash::new("zz".repeat(32)).expect_err("not hex");
        // `TransactionHash` refuses it at construction, so the decode below is
        // defence in depth rather than the primary check.
        assert!(hash.to_string().contains("hexadecimal"));
    }

    #[tokio::test]
    async fn an_absent_transaction_is_none_rather_than_an_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(body_string_contains("getTransaction"))
            .respond_with(crate::test_support::result(serde_json::json!({
                "status": "NOT_FOUND"
            })))
            .mount(&server)
            .await;

        let session = RpcSession::connect(&target(&server)).expect("connects");
        let hash = TransactionHash::new("ab".repeat(32)).expect("a valid hash");
        let found = fetch_transaction(&session, &Cancellation::new(), &hash)
            .await
            .expect("the network answered");
        assert!(found.is_none());
    }

    #[tokio::test]
    async fn a_successful_transaction_is_reported_with_its_ledger() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(body_string_contains("getTransaction"))
            .respond_with(crate::test_support::result(serde_json::json!({
                "status": "SUCCESS",
                "ledger": 4242,
                "applicationOrder": 3,
                "txHash": "ab"
            })))
            .mount(&server)
            .await;

        let session = RpcSession::connect(&target(&server)).expect("connects");
        let hash = TransactionHash::new("ab".repeat(32)).expect("a valid hash");
        let found = fetch_transaction(&session, &Cancellation::new(), &hash)
            .await
            .expect("the request succeeds")
            .expect("the transaction exists");

        assert!(found.successful);
        assert_eq!(found.ledger.map(LedgerSequence::get), Some(4242));
        assert_eq!(found.application_order, Some(3));
    }

    #[tokio::test]
    async fn a_failed_transaction_is_observed_but_not_reported_as_successful() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(body_string_contains("getTransaction"))
            .respond_with(crate::test_support::result(serde_json::json!({
                "status": "FAILED",
                "ledger": 10
            })))
            .mount(&server)
            .await;

        let session = RpcSession::connect(&target(&server)).expect("connects");
        let hash = TransactionHash::new("cd".repeat(32)).expect("a valid hash");
        let found = fetch_transaction(&session, &Cancellation::new(), &hash)
            .await
            .expect("the request succeeds")
            .expect("the transaction exists");

        // A failed transaction still names the contracts it attempted to reach,
        // which is evidence; what it does not do is claim success.
        assert!(!found.successful);
        assert_eq!(found.status, "FAILED");
    }

    #[test]
    fn envelope_operations_are_read_from_every_envelope_kind() {
        let tx = stellar_xdr::Transaction {
            source_account: stellar_xdr::MuxedAccount::Ed25519(stellar_xdr::Uint256([0_u8; 32])),
            fee: 100,
            seq_num: stellar_xdr::SequenceNumber(1),
            cond: stellar_xdr::Preconditions::None,
            memo: stellar_xdr::Memo::None,
            operations: stellar_xdr::VecM::try_from(vec![Operation {
                source_account: None,
                body: OperationBody::InvokeHostFunction(stellar_xdr::InvokeHostFunctionOp {
                    host_function: HostFunction::InvokeContract(stellar_xdr::InvokeContractArgs {
                        contract_address: ScAddress::Contract(XdrContractId(Hash([2_u8; 32]))),
                        function_name: stellar_xdr::ScSymbol::try_from("transfer")
                            .expect("a symbol"),
                        args: stellar_xdr::VecM::try_from(vec![]).expect("no args"),
                    }),
                    auth: stellar_xdr::VecM::try_from(vec![]).expect("no auth"),
                }),
            }])
            .expect("one operation"),
            ext: stellar_xdr::TransactionExt::V0,
        };

        let envelope = TransactionEnvelope::Tx(stellar_xdr::TransactionV1Envelope {
            tx,
            signatures: stellar_xdr::VecM::try_from(vec![]).expect("no signatures"),
        });

        let observed = observation(Some(envelope));
        let contracts = invoked_contracts(&observed);
        assert_eq!(contracts.len(), 1);
        assert_eq!(
            contracts[0],
            format!("{}", stellar_strkey::Contract([2_u8; 32]))
        );
    }

    #[test]
    fn a_contract_named_only_by_an_event_is_still_an_invocation() {
        // Events are an independent reading of the same transaction, and a contract
        // that emitted one was invoked even if the envelope is not available.
        let mut observed = observation(None);
        observed.contract_events = vec![vec![ContractEvent {
            ext: ExtensionPoint::V0,
            contract_id: Some(XdrContractId(Hash([6_u8; 32]))),
            type_: stellar_xdr::ContractEventType::Contract,
            body: stellar_xdr::ContractEventBody::V0(stellar_xdr::ContractEventV0 {
                topics: stellar_xdr::VecM::try_from(vec![]).expect("no topics"),
                data: ScVal::Void,
            }),
        }]];

        let contracts = invoked_contracts(&observed);
        assert_eq!(
            contracts,
            vec![format!("{}", stellar_strkey::Contract([6_u8; 32]))]
        );
    }

    #[test]
    fn the_same_contract_named_twice_appears_once() {
        // Determinism requires a stable, deduplicated order; a caller comparing two
        // runs must not see a difference that is only an ordering artefact.
        let mut observed = observation(None);
        let address = XdrContractId(Hash([7_u8; 32]));
        observed.contract_events = vec![
            vec![ContractEvent {
                ext: ExtensionPoint::V0,
                contract_id: Some(address.clone()),
                type_: stellar_xdr::ContractEventType::Contract,
                body: stellar_xdr::ContractEventBody::V0(stellar_xdr::ContractEventV0 {
                    topics: stellar_xdr::VecM::try_from(vec![]).expect("no topics"),
                    data: ScVal::Void,
                }),
            }],
            vec![ContractEvent {
                ext: ExtensionPoint::V0,
                contract_id: Some(address),
                type_: stellar_xdr::ContractEventType::Contract,
                body: stellar_xdr::ContractEventBody::V0(stellar_xdr::ContractEventV0 {
                    topics: stellar_xdr::VecM::try_from(vec![]).expect("no topics"),
                    data: ScVal::Void,
                }),
            }],
        ];
        assert_eq!(invoked_contracts(&observed).len(), 1);
    }

    #[test]
    fn a_created_contract_entry_is_read_from_v4_metadata_and_absent_otherwise() {
        // The engine does not guess at a creation the metadata did not report, so
        // earlier metadata versions yield nothing rather than a plausible answer.
        let observed = observation(None);
        assert!(created_contract_entries(&observed).is_empty());
    }

    #[test]
    fn a_contract_data_entry_encodes_and_decodes_through_xdr() {
        // Guards the assumption the event and entry paths both rely on: the XDR
        // round trip is lossless for the entries the engine reads.
        let entry = LedgerEntryData::ContractData(ContractDataEntry {
            ext: ExtensionPoint::V0,
            contract: ScAddress::Contract(XdrContractId(Hash([0_u8; 32]))),
            key: ScVal::LedgerKeyContractInstance,
            durability: ContractDataDurability::Persistent,
            val: ScVal::Void,
        });
        let encoded = entry
            .to_xdr_base64(stellar_xdr::Limits::none())
            .expect("encodes");
        let decoded = LedgerEntryData::from_xdr_base64(&encoded, stellar_xdr::Limits::none())
            .expect("decodes");
        assert_eq!(decoded, entry);
    }
}
