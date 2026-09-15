//! Observed contract invocations.
//!
//! # What counts as an invocation
//!
//! An invocation is recorded only from evidence that a call actually happened:
//!
//! * an operation whose host function is `InvokeContract`, which is the transaction
//!   explicitly naming a contract and a function to call; and
//! * a `fn_call` diagnostic event, which the Soroban host emits on entry to every
//!   contract call, including nested ones.
//!
//! The second is what makes a call *graph* recoverable rather than a list. The host
//! emits `fn_call` on entry and `fn_return` on exit, in order, so replaying them
//! recovers the nesting: each `fn_call` was made by whichever call was most recently
//! entered. That is a reconstruction of what the host recorded, not an inference
//! from similarity, which is why an edge derived this way carries the
//! `OBSERVED_INVOCATION` basis rather than `INFERRED_INTERFACE`.
//!
//! # What does not count
//!
//! A contract mentioned in a transaction's metadata, or one whose address appears in
//! an unrelated argument, is not recorded. Nor is a contract that emitted an event
//! in the same transaction as another contract's invocation. The engine must not
//! turn an association into a dependency, and the only way to be sure it does not is
//! to admit only evidence that names both ends of the call.

use amasario_core::{Basis, LedgerSequence, Result, TransactionHash};
use serde::{Deserialize, Serialize};
use stellar_xdr::{
    ContractEvent, ContractEventBody, ContractId as XdrContractId, DiagnosticEvent, Hash,
    HostFunction, Operation, OperationBody, ScAddress, ScSymbol, ScVal, TransactionEnvelope,
};

use amasario_network::transactions::{
    TransactionObservation as NetworkObservation, contract_strkey,
};

/// The topic the Soroban host uses to announce a contract call.
///
/// A protocol fact, taken from the host's diagnostic-event vocabulary rather than
/// chosen here. Recorded as a constant so that the one place it appears is the one
/// place to change if the host's vocabulary changes.
const FN_CALL_TOPIC: &str = "fn_call";

/// The topic the Soroban host uses to announce a contract call's return.
const FN_RETURN_TOPIC: &str = "fn_return";

/// The topic of a host function that failed.
const FN_ERROR_TOPIC: &str = "fn_error";

/// One observed contract call.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ContractInvocation {
    /// The contract whose code was entered.
    pub callee: String,
    /// The contract that entered it, when the caller was another contract.
    ///
    /// `None` for a call made directly by a transaction. That is not missing
    /// information: a top-level invocation genuinely has no calling contract, and
    /// recording the transaction's source account here would attribute a call to an
    /// account as though it were a contract.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub caller: Option<String>,
    /// The function entered, when the evidence named one.
    ///
    /// A diagnostic event carries the function's name; an `InvokeContract` operation
    /// carries it too. Absent for evidence that names only the contract.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub function: Option<String>,
    /// The transaction the call occurred in.
    pub transaction: TransactionHash,
    /// The ledger the transaction was included in, when the endpoint reported it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ledger: Option<LedgerSequence>,
    /// The index of the operation within its transaction, when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operation_index: Option<u32>,
    /// The basis this invocation rests on.
    ///
    /// Only [`Basis::ObservedInvocation`] and [`Basis::ObservedEvent`] are produced
    /// by this module, and the type's constructors enforce it. A caller that wanted
    /// to record an inferred call has to use a different type, which is the point:
    /// an inference must not be able to arrive at the graph wearing an
    /// observation's basis.
    pub basis: Basis,
    /// The diagnostic event's identifier, for a call recovered from events.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub event_id: Option<String>,
    /// Whether the host reported the call as succeeding, when it said.
    pub successful: Option<bool>,
}

impl ContractInvocation {
    /// A top-level invocation named by an operation.
    const fn from_operation(
        callee: String,
        function: Option<String>,
        transaction: TransactionHash,
        ledger: Option<LedgerSequence>,
        operation_index: Option<u32>,
    ) -> Self {
        Self {
            callee,
            caller: None,
            function,
            transaction,
            ledger,
            operation_index,
            basis: Basis::ObservedInvocation,
            event_id: None,
            successful: None,
        }
    }

    /// A call recovered from a diagnostic event.
    const fn from_diagnostic(
        callee: String,
        caller: Option<String>,
        function: Option<String>,
        transaction: TransactionHash,
        ledger: Option<LedgerSequence>,
        operation_index: Option<u32>,
    ) -> Self {
        Self {
            callee,
            caller,
            function,
            transaction,
            ledger,
            operation_index,
            basis: Basis::ObservedEvent,
            event_id: None,
            successful: None,
        }
    }

    /// Records whether the call is known to have succeeded.
    ///
    /// Set from the evidence rather than left absent, because the absence of this is not
    /// neutral: the dependency rules refuse to establish runtime use from a transaction
    /// whose outcome is unknown, so an invocation that carries no outcome produces no
    /// dependency however much evidence is behind it. Leaving it unset is what made a
    /// live testnet contract report an empty dependency set while every call it made was
    /// sitting in the evidence.
    #[must_use]
    pub const fn with_success(mut self, successful: bool) -> Self {
        self.successful = Some(successful);
        self
    }

    /// Whether this invocation names a caller distinct from its callee.
    ///
    /// A self-call is recorded but is not an edge a dependency can rest on, and
    /// asking is cheaper and clearer than each caller testing the two strings.
    #[must_use]
    pub fn is_cross_contract(&self) -> bool {
        self.caller
            .as_deref()
            .is_some_and(|caller| caller != self.callee)
    }
}

/// Whether an invocation was resolved against the callee's declared interface.
///
/// Part of the evidence record rather than of the invocation, because it describes
/// what the engine could check rather than what happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum FunctionResolution {
    /// The called function's name appears in the callee's declared interface.
    Declared,
    /// The callee's interface is known and does not contain the name.
    ///
    /// This is a finding, not a failure: it means the call targeted an entry point
    /// the contract does not declare, which is possible for a contract compiled
    /// without a specification section or invoked through a compatibility shim.
    NotDeclared,
    /// The callee's interface is not available, so the name could not be checked.
    InterfaceUnavailable,
    /// The evidence named no function.
    NoFunctionNamed,
}

impl FunctionResolution {
    /// The stable wire name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Declared => "DECLARED",
            Self::NotDeclared => "NOT_DECLARED",
            Self::InterfaceUnavailable => "INTERFACE_UNAVAILABLE",
            Self::NoFunctionNamed => "NO_FUNCTION_NAMED",
        }
    }

    /// Whether the resolution supports treating the invocation as a call to the
    /// callee's own entry point.
    ///
    /// `NotDeclared` does not, and neither does an unresolved name. A dependency
    /// built on an invocation that targets an undeclared entry point is a weaker
    /// claim, and this is where that is decided rather than at each call site.
    #[must_use]
    pub const fn supports_declared_call(self) -> bool {
        matches!(self, Self::Declared | Self::NoFunctionNamed)
    }
}

/// A transaction's observed invocations, with the bounds that applied.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct InvocationObservations {
    /// The invocations, in the order the evidence recorded them.
    pub invocations: Vec<ContractInvocation>,
    /// Whether the transaction's diagnostic events were available.
    ///
    /// When they were not, nested calls are absent, and a consumer must not read the
    /// absence of an edge as evidence that no call was made. The engine cannot
    /// recover this after the fact, so it is recorded here.
    pub diagnostics_available: bool,
    /// Whether the call nesting was recovered.
    pub nesting_recovered: bool,
}

impl InvocationObservations {
    /// The invocations whose callee is `contract`.
    #[must_use]
    pub fn for_callee(&self, contract: &str) -> Vec<&ContractInvocation> {
        self.invocations
            .iter()
            .filter(|invocation| invocation.callee == contract)
            .collect()
    }

    /// The invocations that concern `contract`: the calls made *to* it, and the
    /// cross-contract calls made *by* it.
    ///
    /// Both directions are needed, and they answer different questions. A call to the
    /// subject establishes that the subject is used; a call by the subject establishes
    /// what it depends on. This is the filter to collect with, because a filter on the
    /// callee alone keeps the first and discards the second - and the second is exactly
    /// the set of edges a dependency can rest on, so collecting with it leaves the
    /// dependency graph empty by construction.
    ///
    /// Self-calls are kept when they arrive as calls *to* the subject, because the fact
    /// that the subject was entered still holds; the dependency detector is where a
    /// self-call is refused as an edge, since that is a question about relationships
    /// rather than about what was observed.
    #[must_use]
    pub fn touching(&self, contract: &str) -> Vec<&ContractInvocation> {
        self.invocations
            .iter()
            .filter(|invocation| {
                invocation.callee == contract
                    || (invocation.is_cross_contract()
                        && invocation.caller.as_deref() == Some(contract))
            })
            .collect()
    }

    /// The cross-contract edges, which are the ones a dependency may rest on.
    #[must_use]
    pub fn cross_contract_edges(&self) -> Vec<&ContractInvocation> {
        self.invocations
            .iter()
            .filter(|invocation| invocation.is_cross_contract())
            .collect()
    }
}

/// Extracts the invocations a transaction evidences.
///
/// # Errors
///
/// Returns [`amasario_core::EngineError::Validation`] when the observation's hash is
/// not a 32-byte hexadecimal value, which can only happen if the observation was
/// built by something other than the network layer.
pub fn invocations_from_transaction(
    observation: &NetworkObservation,
) -> Result<InvocationObservations> {
    let transaction = TransactionHash::new(observation.hash.clone())?;
    let ledger = observation.ledger;

    let mut invocations: Vec<ContractInvocation> = Vec::new();

    // The transaction's own outcome, which every invocation it produced inherits. It is
    // read from the observation rather than left absent, because the dependency rules
    // treat an unknown outcome as a reason to refuse: a call whose transaction is not
    // recorded as successful cannot establish runtime use, so an invocation that carries
    // no outcome establishes nothing and a live run reports no dependencies at all.
    let succeeded = observation.successful;

    // The top-level invocations, from the transaction's own operations. These are
    // read first because they are the root of any call tree.
    if let Some(envelope) = &observation.envelope {
        for (index, operation) in operations_of(envelope).into_iter().enumerate() {
            push_operation_invocations(
                operation,
                &transaction,
                ledger,
                u32::try_from(index).ok(),
                succeeded,
                &mut invocations,
            );
        }
    }

    let diagnostics_available = !observation.diagnostic_events.is_empty();
    let nesting_recovered = if diagnostics_available {
        let entered = invocations_from_diagnostics(
            &observation.diagnostic_events,
            &transaction,
            ledger,
            succeeded,
        );
        // Nesting was recovered when at least one call arrived from an enclosing call,
        // which is what a caller on the invocation means. It is *not* the same as "the
        // diagnostics yielded calls": a transaction that enters one contract and calls
        // nothing further has a root `fn_call` marker with no caller, so counting any
        // recovered call as recovered nesting reports nesting where the evidence shows
        // none - and "this transaction nested" and "this transaction did not nest" would
        // become indistinguishable, which is the distinction a control case exists for.
        let recovered = entered.iter().any(|invocation| invocation.caller.is_some());
        invocations.extend(entered);
        recovered
    } else {
        false
    };

    Ok(InvocationObservations {
        invocations: canonicalise(invocations),
        diagnostics_available,
        nesting_recovered,
    })
}

/// Orders and de-duplicates a transaction's invocations.
///
/// Two jobs, and the second is the reason this is a function rather than two calls.
///
/// **Ordering.** The same transaction read twice must produce the same list, so the
/// order is made canonical rather than left to the order the endpoint returned its
/// events in. The sequence within a transaction is not part of any answer the engine
/// gives - only which calls occurred - so fixing the order costs nothing.
///
/// **De-duplication.** One call is frequently visible twice: an `InvokeContract`
/// operation states it, and the host's `fn_call` diagnostic event records it. They
/// are the same fact observed through two pieces of evidence, and counting it twice
/// would inflate the callee's apparent fan-in and make an impact analysis report a
/// larger affected set than the evidence supports.
///
/// The deduplication key deliberately excludes the basis, and the sort places the
/// stronger basis first, so the record that survives is the one resting on the
/// strongest evidence. Equally, two invocations of the same function on the same
/// contract in one transaction are one fact, not two, and collapse to one record.
fn canonicalise(mut invocations: Vec<ContractInvocation>) -> Vec<ContractInvocation> {
    invocations.sort_by(|a, b| {
        (
            a.callee.as_str(),
            a.caller.as_deref(),
            a.function.as_deref(),
            a.transaction.as_str(),
        )
            .cmp(&(
                b.callee.as_str(),
                b.caller.as_deref(),
                b.function.as_deref(),
                b.transaction.as_str(),
            ))
            // `Basis` is declared strongest first, so ascending order puts the
            // strongest evidence at the front of each group.
            .then_with(|| a.basis.cmp(&b.basis))
    });
    invocations.dedup_by(|a, b| {
        a.callee == b.callee
            && a.caller == b.caller
            && a.function == b.function
            && a.transaction == b.transaction
    });
    invocations
}

/// Records the invocations named by one operation.
fn push_operation_invocations(
    operation: &Operation,
    transaction: &TransactionHash,
    ledger: Option<LedgerSequence>,
    operation_index: Option<u32>,
    succeeded: bool,
    out: &mut Vec<ContractInvocation>,
) {
    let OperationBody::InvokeHostFunction(invoke) = &operation.body else {
        return;
    };
    let HostFunction::InvokeContract(args) = &invoke.host_function else {
        // Uploading a module or creating a contract is a deployment, not an
        // invocation, and recording it here would make a deployment look like a
        // call. Those are handled by the provenance layer.
        return;
    };
    let Some(callee) = contract_strkey(&args.contract_address) else {
        return;
    };
    out.push(
        ContractInvocation::from_operation(
            callee,
            Some(symbol_text(&args.function_name)),
            transaction.clone(),
            ledger,
            operation_index,
        )
        .with_success(succeeded),
    );
}

/// The operations of an envelope, for either transaction kind.
fn operations_of(envelope: &TransactionEnvelope) -> Vec<&Operation> {
    amasario_network::transactions::envelope_operations(envelope)
}

/// Recovers nested calls from the host's diagnostic events.
///
/// The host emits `fn_call` on entry to a contract call and `fn_return` on exit,
/// in order, so the events describe a call stack. Replaying them with a stack
/// recovers which call made which: a `fn_call` was made by whichever call was most
/// recently entered.
///
/// Events are processed in the order they were emitted. A `fn_return` with an empty
/// stack is ignored rather than treated as a failure, because a transaction may be
/// observed part-way through in the metadata of a failed operation.
fn invocations_from_diagnostics(
    diagnostics: &[DiagnosticEvent],
    transaction: &TransactionHash,
    ledger: Option<LedgerSequence>,
    succeeded: bool,
) -> Vec<ContractInvocation> {
    let mut out: Vec<ContractInvocation> = Vec::new();
    let mut stack: Vec<String> = Vec::new();

    for event in diagnostics {
        let Some(topic) = first_symbol(&event.event) else {
            continue;
        };
        match topic.as_str() {
            FN_CALL_TOPIC => {
                let Some(callee) = fn_call_callee(&event.event) else {
                    continue;
                };
                let function = fn_call_function(&event.event);
                let caller = stack.last().cloned();
                // Every `fn_call` is an invocation, including a self-call: the host
                // emitted the marker because a call was entered, and a contract that
                // calls itself is a fact a consumer may need to see. The caller is
                // whatever was most recently entered, which is `None` at the root.
                // The transaction's outcome ends *in the host*, so a marker inside a
                // failed transaction is not a call that happened even though the
                // marker was emitted. Both signals are required, which is what
                // `in_successful_contract_call` is for.
                out.push(
                    ContractInvocation::from_diagnostic(
                        callee.clone(),
                        caller,
                        function,
                        transaction.clone(),
                        ledger,
                        None,
                    )
                    .with_success(succeeded && event.in_successful_contract_call),
                );
                stack.push(callee);
            },
            FN_RETURN_TOPIC | FN_ERROR_TOPIC => {
                // The stack is only meaningful while calls are open, so a return
                // pops. Popping an empty stack is ignored: it means the observed
                // slice of the transaction does not begin at its root.
                stack.pop();
            },
            _ => {},
        }
    }

    out
}

/// The first topic of an event, when it is a symbol.
fn first_symbol(event: &ContractEvent) -> Option<String> {
    let ContractEventBody::V0(body) = &event.body;
    match body.topics.first()? {
        ScVal::Symbol(symbol) => Some(symbol_text(symbol)),
        _ => None,
    }
}

/// The contract a `fn_call` diagnostic names as the callee.
///
/// The second topic, an `SCV_ADDRESS`, is the callee and it is authoritative. The
/// `contractID` field is **not**: on a `fn_call` marker it names the contract that
/// emitted the marker, which is the *enclosing* call. Reading the field as the callee
/// therefore attributes every nested call to its own caller, and the result is a graph
/// in which every contract appears to call nothing but itself - which is worse than an
/// empty graph, because it looks like an answer.
///
/// Both shapes are decoded from real testnet `diagnosticEventsXdr`, not inferred. A
/// top-level entry is `contractID = None,
/// topics = [Symbol("fn_call"), Address(<callee>), Symbol("place")]`; a nested one is
/// `contractID = Some(<caller>),
/// topics = [Symbol("fn_call"), Address(<callee>), Symbol("transfer")]`. The address
/// topic is the callee in both.
///
/// The field is kept as a fallback for a producer that puts the callee there and not in
/// the topics, which costs nothing and is the shape the earlier tests assumed.
fn fn_call_callee(event: &ContractEvent) -> Option<String> {
    let ContractEventBody::V0(body) = &event.body;
    if let Some(callee) = body.topics.get(1).and_then(address_contract) {
        return Some(callee);
    }
    event
        .contract_id
        .as_ref()
        .and_then(|id| contract_strkey(&ScAddress::Contract(id.clone())))
}

/// The function name a `fn_call` diagnostic names.
///
/// The first symbol after the marker, whichever position it lands in. The host's topics
/// are `[fn_call, address, function]`, so that is the third topic - but taking a fixed
/// index would be wrong in the other direction for a producer that sets `contractID` and
/// emits `[fn_call, function]`, where the name is second. Looking for the symbol rather
/// than counting positions reads both, and reads neither as the callee address, which is
/// not a symbol and would otherwise present a named call as an unnamed one.
///
/// A missing name is not treated as malformed: it means the evidence did not name the
/// function, which is already a distinguishable outcome in [`FunctionResolution`].
fn fn_call_function(event: &ContractEvent) -> Option<String> {
    let ContractEventBody::V0(body) = &event.body;
    body.topics.iter().skip(1).find_map(|topic| match topic {
        ScVal::Symbol(symbol) => Some(symbol_text(symbol)),
        _ => None,
    })
}

/// The contract a topic names, when it names one.
///
/// Two encodings have to be read, and the one that matters is not the obvious one. A
/// contract value in an ordinary event topic is an `SCV_ADDRESS`, but the callee of a
/// `fn_call` marker is emitted as `SCV_BYTES` holding the raw 32-byte contract id -
/// which is what testnet actually carries, confirmed by decoding the diagnostic events
/// of a live transaction: `topics = [Symbol("fn_call"), Bytes(<32 bytes>),
/// Symbol("transfer")]`. Accepting only the address form therefore recovers no nested
/// call at all, while accepting only the bytes form would miss the other.
///
/// Thirty-two bytes exactly. A `fn_call` topic is never a variable-length byte string,
/// so a value of any other length is refused rather than truncated into an identifier
/// that would name some other contract.
fn address_contract(value: &ScVal) -> Option<String> {
    match value {
        ScVal::Address(address) => contract_strkey(address),
        ScVal::Bytes(bytes) => {
            let raw: [u8; 32] = bytes.0.as_slice().try_into().ok()?;
            contract_strkey(&ScAddress::Contract(XdrContractId(Hash(raw))))
        },
        _ => None,
    }
}

/// Reads a symbol's text.
///
/// `ScSymbol` is a distinct generated type wrapping a bounded string rather than an
/// alias for one, so its text lives one level down.
fn symbol_text(symbol: &ScSymbol) -> String {
    symbol.0.to_utf8_string_lossy()
}

#[cfg(test)]
mod tests {
    use super::*;
    use stellar_xdr::{
        ContractEventV0, ContractId as XdrContractId, ExtensionPoint, Hash, InvokeContractArgs,
        InvokeHostFunctionOp, Operation, ScSymbol, VecM,
    };

    fn hash(seed: u8) -> TransactionHash {
        TransactionHash::new(hex::encode([seed; 32])).expect("32 bytes of hex")
    }

    fn address(payload: [u8; 32]) -> String {
        format!("{}", stellar_strkey::Contract(payload))
    }

    fn ledger() -> Option<LedgerSequence> {
        Some(LedgerSequence::new(100).expect("a real ledger"))
    }

    fn operation(callee: [u8; 32], function: &str) -> Operation {
        Operation {
            source_account: None,
            body: OperationBody::InvokeHostFunction(InvokeHostFunctionOp {
                host_function: HostFunction::InvokeContract(InvokeContractArgs {
                    contract_address: ScAddress::Contract(XdrContractId(Hash(callee))),
                    function_name: ScSymbol(function.parse().expect("a short symbol")),
                    args: VecM::default(),
                }),
                auth: VecM::default(),
            }),
        }
    }

    fn envelope(operations: Vec<Operation>) -> TransactionEnvelope {
        TransactionEnvelope::Tx(stellar_xdr::TransactionV1Envelope {
            tx: stellar_xdr::Transaction {
                source_account: stellar_xdr::MuxedAccount::Ed25519(stellar_xdr::Uint256(
                    [0_u8; 32],
                )),
                fee: 100,
                seq_num: stellar_xdr::SequenceNumber(1),
                cond: stellar_xdr::Preconditions::None,
                memo: stellar_xdr::Memo::None,
                operations: operations.try_into().expect("a few operations"),
                ext: stellar_xdr::TransactionExt::V0,
            },
            signatures: VecM::default(),
        })
    }

    fn diagnostic(topics: Vec<ScVal>, contract: Option<[u8; 32]>) -> DiagnosticEvent {
        DiagnosticEvent {
            in_successful_contract_call: true,
            event: ContractEvent {
                ext: ExtensionPoint::V0,
                contract_id: contract.map(|payload| XdrContractId(Hash(payload))),
                type_: stellar_xdr::ContractEventType::Diagnostic,
                body: ContractEventBody::V0(ContractEventV0 {
                    topics: topics.try_into().expect("a few topics"),
                    data: ScVal::Void,
                }),
            },
        }
    }

    fn symbol(value: &str) -> ScVal {
        ScVal::Symbol(ScSymbol(value.parse().expect("a short symbol")))
    }

    fn address_value(payload: [u8; 32]) -> ScVal {
        ScVal::Address(ScAddress::Contract(XdrContractId(Hash(payload))))
    }

    /// A `fn_call` marker in the shape the host actually emits on the wire.
    ///
    /// This is the detail the earlier tests got wrong, and getting it wrong is why they
    /// passed while the engine recovered nothing from a live network. The host does not
    /// set `contractID` on these events - a diagnostic event has no emitting contract,
    /// the callee is the payload - so the callee is `topics[1]` as an address and the
    /// function is `topics[2]`. A test that puts the callee in `contractID` and the
    /// function in `topics[1]` is testing a shape no node produces.
    fn fn_call(callee: [u8; 32], function: &str) -> DiagnosticEvent {
        diagnostic(
            vec![symbol("fn_call"), address_value(callee), symbol(function)],
            None,
        )
    }

    fn observation(
        envelope: Option<TransactionEnvelope>,
        diagnostics: Vec<DiagnosticEvent>,
    ) -> NetworkObservation {
        NetworkObservation {
            hash: hash(1).as_str().to_owned(),
            ledger: ledger(),
            status: "SUCCESS".to_owned(),
            successful: true,
            application_order: Some(1),
            envelope,
            result: None,
            result_meta: None,
            contract_events: Vec::new(),
            transaction_events: Vec::new(),
            diagnostic_events: diagnostics,
        }
    }

    #[test]
    fn an_invoke_operation_records_a_top_level_invocation_with_no_caller() {
        let observations = invocations_from_transaction(&observation(
            Some(envelope(vec![operation([5_u8; 32], "transfer")])),
            Vec::new(),
        ))
        .expect("the observation is well formed");

        assert_eq!(observations.invocations.len(), 1);
        let invocation = &observations.invocations[0];
        assert_eq!(invocation.callee, address([5_u8; 32]));
        assert_eq!(invocation.function.as_deref(), Some("transfer"));
        assert_eq!(
            invocation.caller, None,
            "a top-level call has no calling contract, and an account is not a contract"
        );
        assert!(!invocation.is_cross_contract());
        assert_eq!(invocation.basis, Basis::ObservedInvocation);
        assert_eq!(invocation.ledger, ledger());
        assert_eq!(invocation.operation_index, Some(0));
    }

    #[test]
    fn a_deployment_operation_is_not_recorded_as_an_invocation() {
        // Uploading a module or creating a contract is a deployment; recording it
        // here would make a deployment look like a call.
        let upload = Operation {
            source_account: None,
            body: OperationBody::InvokeHostFunction(InvokeHostFunctionOp {
                host_function: HostFunction::UploadContractWasm(
                    stellar_xdr::BytesM::try_from(vec![0_u8; 8]).expect("eight bytes"),
                ),
                auth: VecM::default(),
            }),
        };
        let observations =
            invocations_from_transaction(&observation(Some(envelope(vec![upload])), Vec::new()))
                .expect("well formed");
        assert!(observations.invocations.is_empty());
        assert!(!observations.diagnostics_available);
    }

    #[test]
    fn a_non_soroban_operation_is_ignored() {
        let payment = Operation {
            source_account: None,
            body: OperationBody::BumpSequence(stellar_xdr::BumpSequenceOp {
                bump_to: stellar_xdr::SequenceNumber(5),
            }),
        };
        let observations =
            invocations_from_transaction(&observation(Some(envelope(vec![payment])), Vec::new()))
                .expect("well formed");
        assert!(observations.invocations.is_empty());
    }

    #[test]
    fn diagnostic_events_recover_the_nesting_of_a_call_tree() {
        // A -> B -> C, as the host records it: enter A, enter B, enter C, return C,
        // return B, return A. Recovering this is what turns a list of calls into a
        // graph, and it is the difference between an observed edge and a guess.
        let a = [1_u8; 32];
        let b = [2_u8; 32];
        let c = [3_u8; 32];
        let diagnostics = vec![
            fn_call(a, "a_fn"),
            fn_call(b, "b_fn"),
            fn_call(c, "c_fn"),
            diagnostic(vec![symbol("fn_return")], Some(c)),
            diagnostic(vec![symbol("fn_return")], Some(b)),
            diagnostic(vec![symbol("fn_return")], Some(a)),
        ];

        let observations =
            invocations_from_transaction(&observation(None, diagnostics)).expect("well formed");

        assert!(observations.diagnostics_available);
        assert!(observations.nesting_recovered);
        assert_eq!(observations.invocations.len(), 3);

        let into_a = observations
            .for_callee(&address(a))
            .into_iter()
            .next()
            .expect("A was entered");
        assert_eq!(into_a.caller, None, "A was entered by the transaction");

        let into_b = observations
            .for_callee(&address(b))
            .into_iter()
            .next()
            .expect("B was entered");
        assert_eq!(into_b.caller.as_deref(), Some(address(a).as_str()));
        assert_eq!(into_b.function.as_deref(), Some("b_fn"));
        assert_eq!(into_b.basis, Basis::ObservedEvent);

        let into_c = observations
            .for_callee(&address(c))
            .into_iter()
            .next()
            .expect("C was entered");
        assert_eq!(into_c.caller.as_deref(), Some(address(b).as_str()));

        assert_eq!(observations.cross_contract_edges().len(), 2);
    }

    #[test]
    fn a_fn_call_names_its_callee_in_the_topics_rather_than_the_contract_field() {
        // The regression. A `fn_call` diagnostic carries no `contractID`, so a reader
        // that insists on that field recovers nothing and reports a contract as having
        // no dependencies - when a contract's calls are the only thing it has.
        let a = [1_u8; 32];
        let b = [2_u8; 32];
        let diagnostics = vec![fn_call(a, "enter"), fn_call(b, "transfer")];

        let observations =
            invocations_from_transaction(&observation(None, diagnostics)).expect("well formed");

        assert!(
            observations.nesting_recovered,
            "the callee is in the topics, so the call tree is recoverable without contractID"
        );
        let into_b = observations
            .for_callee(&address(b))
            .into_iter()
            .next()
            .expect("B was entered, and its address came from the second topic");
        assert_eq!(into_b.caller.as_deref(), Some(address(a).as_str()));
        assert_eq!(
            into_b.function.as_deref(),
            Some("transfer"),
            "the function is the third topic; reading the second would return the address"
        );
        assert_eq!(observations.cross_contract_edges().len(), 1);
    }

    #[test]
    fn a_nested_fn_call_names_the_callee_not_the_contract_that_emitted_the_marker() {
        // The shape that produced a graph of self-calls. A nested marker carries the
        // *caller* in `contractID`, because that is the contract whose execution
        // emitted it, while the callee is the address topic. Reading the field first
        // makes every nested call look like a call to the caller, so a contract that
        // calls the fee contract appears to call itself and no edge is ever found.
        let caller = [1_u8; 32];
        let callee = [2_u8; 32];
        let diagnostics = vec![diagnostic(
            vec![symbol("fn_call"), address_value(callee), symbol("transfer")],
            Some(caller),
        )];

        let observations =
            invocations_from_transaction(&observation(None, diagnostics)).expect("well formed");
        let invocation = observations
            .invocations
            .first()
            .expect("the call was recovered");

        assert_eq!(
            invocation.callee,
            address(callee),
            "the callee is the address topic, not the emitting contract"
        );
        assert_ne!(
            invocation.callee,
            address(caller),
            "attributing the call to the caller is the defect this guards"
        );
    }

    #[test]
    fn a_fn_call_that_does_carry_its_callee_in_the_contract_field_is_still_read() {
        // A producer that sets the field is describing the same fact, so both readings
        // are accepted. The topics form is the one the host emits; this guards the
        // other against removal by someone who only ever sees testnet.
        let a = [1_u8; 32];
        let diagnostics = vec![diagnostic(vec![symbol("fn_call"), symbol("a_fn")], Some(a))];

        let observations =
            invocations_from_transaction(&observation(None, diagnostics)).expect("well formed");
        let invocation = observations
            .for_callee(&address(a))
            .into_iter()
            .next()
            .expect("the callee came from the field");
        assert_eq!(invocation.callee, address(a));
        assert_eq!(invocation.function.as_deref(), Some("a_fn"));
    }

    #[test]
    fn touching_keeps_both_the_calls_made_to_a_subject_and_the_calls_it_makes() {
        // The second half of the same defect. A call to the subject says the subject is
        // used; a call by the subject says what it depends on. Filtering on the callee
        // alone keeps the first and discards the second, which is the entire edge set
        // a dependency can rest on - so the dependency graph comes out empty by
        // construction, however much evidence was read.
        let subject = [1_u8; 32];
        let dep = [2_u8; 32];
        let unrelated = [3_u8; 32];
        let diagnostics = vec![
            fn_call(subject, "enter"),
            fn_call(dep, "transfer"),
            fn_call(unrelated, "elsewhere"),
        ];

        let observations =
            invocations_from_transaction(&observation(None, diagnostics)).expect("well formed");
        let subject_str = address(subject);
        let dep_str = address(dep);

        let touching = observations.touching(&subject_str);
        assert_eq!(
            touching.len(),
            2,
            "the call into the subject and the call out of it"
        );
        assert_eq!(
            observations.for_callee(&subject_str).len(),
            1,
            "the filter it replaced kept only the incoming call"
        );

        let outgoing = touching
            .iter()
            .find(|invocation| invocation.callee == dep_str)
            .expect("the outgoing edge is kept");
        assert_eq!(outgoing.caller.as_deref(), Some(subject_str.as_str()));
        assert!(
            outgoing.is_cross_contract(),
            "an outgoing call is the edge a dependency rests on"
        );
    }

    #[test]
    fn a_diagnostic_event_that_is_not_a_call_marker_is_ignored() {
        let diagnostics = vec![
            diagnostic(vec![symbol("transfer"), symbol("amount")], Some([1_u8; 32])),
            diagnostic(vec![ScVal::U32(7)], Some([1_u8; 32])),
        ];
        let observations =
            invocations_from_transaction(&observation(None, diagnostics)).expect("well formed");

        assert!(
            observations.invocations.is_empty(),
            "an emitted event is not a call marker; treating it as one would invent an edge"
        );
        assert!(
            !observations.nesting_recovered,
            "nothing was recovered, and claiming otherwise would overstate the evidence"
        );
    }

    #[test]
    fn a_single_top_level_call_is_not_recovered_nesting() {
        // The distinction a control case exists for, and one that was wrong: a
        // transaction that enters one contract and calls nothing further emits a root
        // `fn_call` marker whose caller is absent. Counting any recovered call as
        // recovered nesting made this transaction indistinguishable from one that
        // nested, so a consumer asking whether the tree was reconstructed was told yes
        // when all that had been read was a single entry. Diagnostics being available
        // and nesting having been found are different facts, and both are reported.
        let a = [1_u8; 32];
        let diagnostics = vec![fn_call(a, "set_price")];

        let observations =
            invocations_from_transaction(&observation(None, diagnostics)).expect("well formed");

        assert!(
            observations.diagnostics_available,
            "the host's diagnostics were read, which is a fact about the reading"
        );
        assert!(
            !observations.nesting_recovered,
            "one call with no caller is a call tree of depth one, which is not recovered \
             nesting"
        );
        assert_eq!(
            observations.invocations.len(),
            1,
            "the call is still recorded: reading no nesting does not mean reading no call"
        );
        assert_eq!(
            observations.invocations[0].caller, None,
            "a root call has no calling contract"
        );
        assert!(
            observations.cross_contract_edges().is_empty(),
            "a root call is not an edge between contracts"
        );
    }

    #[test]
    fn a_return_without_a_matching_call_does_not_corrupt_the_reconstruction() {
        // The observed slice of a transaction need not begin at its root, and a
        // stray return must not pop a call that is still open.
        let a = [1_u8; 32];
        let b = [2_u8; 32];
        let diagnostics = vec![
            diagnostic(vec![symbol("fn_return")], None),
            fn_call(a, "a_fn"),
            fn_call(b, "b_fn"),
        ];

        let observations =
            invocations_from_transaction(&observation(None, diagnostics)).expect("well formed");
        let into_b = observations
            .for_callee(&address(b))
            .into_iter()
            .next()
            .expect("B was entered");
        assert_eq!(
            into_b.caller.as_deref(),
            Some(address(a).as_str()),
            "A was still on the stack when B was entered"
        );
    }

    #[test]
    fn a_self_call_is_recorded_but_is_not_a_cross_contract_edge() {
        let a = [1_u8; 32];
        let diagnostics = vec![fn_call(a, "outer"), fn_call(a, "inner")];

        let observations =
            invocations_from_transaction(&observation(None, diagnostics)).expect("well formed");
        assert_eq!(
            observations.invocations.len(),
            2,
            "both entries are real calls: the outer one is entered by the transaction, the inner \
             one by the contract itself"
        );
        let self_call = observations
            .invocations
            .iter()
            .find(|invocation| invocation.caller.is_some())
            .expect("the self-call is recorded");
        assert_eq!(self_call.caller.as_deref(), Some(address(a).as_str()));
        assert_eq!(self_call.callee, address(a));
        assert!(
            !self_call.is_cross_contract(),
            "a self-call names one contract, so it is not an edge between two"
        );
    }

    #[test]
    fn a_call_tree_without_diagnostics_says_the_nesting_was_not_recovered() {
        // The engine must not let the absence of an edge read as evidence that no
        // call was made.
        let observations = invocations_from_transaction(&observation(
            Some(envelope(vec![operation([5_u8; 32], "transfer")])),
            Vec::new(),
        ))
        .expect("well formed");

        assert!(!observations.diagnostics_available);
        assert!(!observations.nesting_recovered);
        assert_eq!(observations.invocations.len(), 1);
    }

    #[test]
    fn the_same_transaction_read_twice_produces_the_same_invocations() {
        let a = [1_u8; 32];
        let diagnostics = vec![
            diagnostic(vec![symbol("fn_call"), symbol("a_fn")], Some(a)),
            diagnostic(vec![symbol("fn_call"), symbol("b_fn")], Some([2_u8; 32])),
            diagnostic(vec![symbol("fn_return")], Some([2_u8; 32])),
        ];
        let observation = observation(
            Some(envelope(vec![
                operation([4_u8; 32], "first"),
                operation([5_u8; 32], "second"),
            ])),
            diagnostics,
        );

        let first = invocations_from_transaction(&observation).expect("well formed");
        for _ in 0..16 {
            assert_eq!(
                invocations_from_transaction(&observation).expect("well formed"),
                first
            );
        }
        assert_eq!(
            serde_json::to_string(&first).expect("serialises"),
            serde_json::to_string(
                &invocations_from_transaction(&observation).expect("well formed")
            )
            .expect("serialises")
        );
    }

    #[test]
    fn duplicate_evidence_for_one_call_produces_one_invocation() {
        // The same call can be visible both as an operation and as a diagnostic
        // event. Reporting it twice would double a contract's apparent fan-in.
        let callee = [5_u8; 32];
        let diagnostics = vec![diagnostic(
            vec![symbol("fn_call"), symbol("transfer")],
            Some(callee),
        )];
        let observations = invocations_from_transaction(&observation(
            Some(envelope(vec![operation(callee, "transfer")])),
            diagnostics,
        ))
        .expect("well formed");

        let matching: Vec<&ContractInvocation> = observations
            .invocations
            .iter()
            .filter(|invocation| {
                invocation.callee == address(callee) && invocation.caller.is_none()
            })
            .collect();
        assert_eq!(
            matching.len(),
            1,
            "the same call observed twice must not be counted twice"
        );
    }

    #[test]
    fn a_malformed_transaction_hash_is_rejected_rather_than_recorded() {
        let mut observation = observation(None, Vec::new());
        observation.hash = "not a hash".to_owned();
        invocations_from_transaction(&observation).expect_err("the hash is not a hash");
    }

    #[test]
    fn only_observation_bases_are_produced_by_this_module() {
        // An inference must not be able to arrive at the graph wearing an
        // observation's basis.
        let observations = invocations_from_transaction(&observation(
            Some(envelope(vec![operation([5_u8; 32], "transfer")])),
            vec![diagnostic(
                vec![symbol("fn_call"), symbol("nested")],
                Some([6_u8; 32]),
            )],
        ))
        .expect("well formed");

        assert!(!observations.invocations.is_empty());
        for invocation in &observations.invocations {
            assert!(
                matches!(
                    invocation.basis,
                    Basis::ObservedInvocation | Basis::ObservedEvent
                ),
                "an invocation cannot rest on {}",
                invocation.basis
            );
        }
    }

    #[test]
    fn a_function_resolution_decides_whether_it_supports_a_declared_call() {
        assert!(FunctionResolution::Declared.supports_declared_call());
        assert!(
            FunctionResolution::NoFunctionNamed.supports_declared_call(),
            "no name was claimed, so nothing is contradicted"
        );
        assert!(!FunctionResolution::NotDeclared.supports_declared_call());
        assert!(!FunctionResolution::InterfaceUnavailable.supports_declared_call());
    }

    #[test]
    fn an_invocation_round_trips_through_json() {
        let observations = invocations_from_transaction(&observation(
            Some(envelope(vec![operation([5_u8; 32], "transfer")])),
            Vec::new(),
        ))
        .expect("well formed");
        let json = serde_json::to_string(&observations).expect("serialises");
        assert_eq!(
            serde_json::from_str::<InvocationObservations>(&json).expect("deserialises"),
            observations
        );
    }
}
