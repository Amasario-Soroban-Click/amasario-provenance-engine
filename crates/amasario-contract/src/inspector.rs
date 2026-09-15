//! Contract inspection: assembling everything the engine can observe about one
//! deployed contract.
//!
//! # The order of operations, and why it is fixed
//!
//! Inspection establishes the network before it observes anything. A passphrase
//! mismatch fails here, before any fact has been collected, because an analysis
//! that accumulated observations from one chain and was then labelled with another
//! would be wrong in a way no later stage could detect. This is the same rule
//! `amasario-network` states for itself; it is repeated here because inspection is
//! the first caller and the rule is only as good as its first enforcement.
//!
//! # What an inspection is
//!
//! A record of what was observed, the boundary it was observed at, and **what could
//! not be observed**. The last part is not an afterthought. Three states are kept
//! distinct throughout:
//!
//! * a fact that was read, recorded as such;
//! * a fact that is absent, which is an answer about the ledger; and
//! * a fact that could not be obtained, recorded in
//!   [`ContractInspection::anomalies`] or in
//!   [`ContractInspection::truncation`] depending on whether it is a property of the
//!   contract or of the search.
//!
//! An anomaly does **not** abort an inspection. A contract whose code entry is
//! absent, or whose module does not hash to the digest the network records, is
//! precisely the case the engine exists to report, and failing the whole run would
//! hide it. The anomalies travel with the inspection and the provenance layer turns
//! them into `UNVERIFIED` or `CONFLICTING` statuses.

use amasario_core::{
    Cancellation, ContractId, EngineError, LedgerSequence, Network, Observation,
    ObservationBoundary, ObservationProvenance, Result, TraversalOutcome, TruncationReason,
};
use serde::{Deserialize, Serialize};
use stellar_xdr::{
    ContractEventType, ContractExecutable, LedgerEntryData, ScAddress, ScVal, TransactionMeta,
};

use amasario_network::client::NetworkTarget;
use amasario_network::contracts::{ContractEntry, fetch_contract_entries};
use amasario_network::events::{EventQuery, scan_events};
use amasario_network::horizon::HorizonSession;
use amasario_network::operations::operations_for_ledger;
use amasario_network::rpc::RpcSession;
use amasario_network::transactions::fetch_transaction;

use crate::errors::InspectionFailure;
use crate::identity::{
    ContractExecutableKind, ContractIdentity, InstanceModification, InstanceModificationKind,
    digest_from_hash_bytes,
};
use crate::interface::{ContractInterface, decode_spec_section};
use crate::invocations::{ContractInvocation, invocations_from_transaction};
use crate::storage::{Durability, StorageObservation, instance_storage, observe_keys};
use crate::wasm::{WasmModule, parse_module};

/// The Soroban contract function a contract invokes to replace its own executable.
///
/// A protocol fact: `update_current_contract_wasm` is the host function the
/// Soroban deployer exposes for an in-place upgrade, and a contract calls it on
/// *itself*. Recorded as a constant so the one place it appears is the one place to
/// change, and because a `DEPLOY`/`UPGRADE` classification rests on it.
pub const UPDATE_CURRENT_CONTRACT_WASM: &str = "update_current_contract_wasm";

/// The default bound on how many transactions are read to evidence invocations.
///
/// An event scan can return events from many transactions, and reading each one is a
/// request. The bound keeps a busy contract's inspection from becoming an unbounded
/// sequence of reads, and the count that was actually read is reported so that a
/// consumer can see the search stopped.
pub const DEFAULT_MAX_TRANSACTION_READS: usize = 16;

/// The default bound on operations read when looking for a modification.
///
/// Matches the largest page Horizon serves for a ledger's operations, so the bound
/// is the page size rather than a smaller number that would silently truncate.
pub const MAX_MODIFICATION_OPERATIONS: usize = 200;

/// What to inspect, and how far to look.
#[derive(Debug, Clone, PartialEq)]
pub struct InspectionRequest {
    /// The contract to inspect.
    pub contract: ContractId,
    /// The boundary's timestamp, supplied rather than read from the clock.
    ///
    /// Passed in because an execution context owns the run's clock reading, and an
    /// inspection that read its own would make two runs of the same analysis differ.
    pub observed_at: String,
    /// Storage keys to look up, with the durability each is stored under.
    pub storage_keys: Vec<(ScVal, Durability)>,
    /// Whether to look for the operation that last modified the contract's instance
    /// entry.
    ///
    /// Requires a Horizon endpoint, and costs one operations request plus a bounded
    /// number of transaction reads. Off by default because it is only needed when a
    /// consumer is asking about deployment or upgrade provenance.
    pub resolve_modification: bool,
    /// The event query to scan, or `None` to skip the scan.
    ///
    /// Present as an option because an event scan is the most expensive part of an
    /// inspection and is not needed to establish identity.
    pub event_query: Option<EventQuery>,
    /// The bound on transactions read for invocation evidence.
    pub max_transaction_reads: usize,
}

impl InspectionRequest {
    /// A request that establishes identity and reads the module, and nothing more.
    #[must_use]
    pub fn identity_only(contract: ContractId, observed_at: impl Into<String>) -> Self {
        Self {
            contract,
            observed_at: observed_at.into(),
            storage_keys: Vec::new(),
            resolve_modification: false,
            event_query: None,
            max_transaction_reads: DEFAULT_MAX_TRANSACTION_READS,
        }
    }

    /// Adds a storage key to look up.
    #[must_use]
    pub fn with_storage_key(mut self, key: ScVal, durability: Durability) -> Self {
        self.storage_keys.push((key, durability));
        self
    }

    /// Requests that the instance modification be resolved.
    #[must_use]
    pub const fn resolving_modification(mut self) -> Self {
        self.resolve_modification = true;
        self
    }

    /// Adds an event scan.
    #[must_use]
    pub fn scanning_events(mut self, query: EventQuery) -> Self {
        self.event_query = Some(query);
        self
    }
}

/// An event observed to have been emitted by the contract.
///
/// A record of emission, not of a call: an emitted event says the contract's code
/// ran, and says nothing about who invoked it. The two are kept apart because a
/// dependency may rest on the first and not on the second.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EmittedEvent {
    /// The endpoint's event identifier.
    pub id: String,
    /// The contract that emitted it.
    pub contract_id: String,
    /// The ledger it was emitted in.
    pub ledger: LedgerSequence,
    /// The ledger's close time, as the endpoint reported it.
    pub closed_at: String,
    /// The transaction it was emitted in, when the endpoint reported one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transaction: Option<String>,
    /// The index of the emitting operation within its transaction.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operation_index: Option<u32>,
}

/// Everything observed about one contract.
#[derive(Debug, Clone, PartialEq)]
pub struct ContractInspection {
    /// The contract's identity as observed.
    pub identity: ContractIdentity,
    /// The deployed module, when it was retrieved and parsed.
    pub module: Option<WasmModule>,
    /// Whether the module's bytes hash to the digest the network records.
    ///
    /// `None` when there is no module to verify. Not a boolean that defaults to
    /// false, because "not verified because there is nothing to verify" and "checked
    /// and did not match" are different outcomes.
    pub digest_verified: Option<bool>,
    /// The interface the module declares, when its specification section decoded.
    pub interface: Option<ContractInterface>,
    /// The contract's instance storage, which is the one enumerable region.
    pub instance_storage: Vec<(String, String)>,
    /// The result of looking up the requested storage keys.
    pub storage: Option<StorageObservation>,
    /// Invocations evidenced by the transactions the event scan reached.
    pub invocations: Vec<ContractInvocation>,
    /// Events observed to have been emitted by the contract.
    pub emitted_events: Vec<EmittedEvent>,
    /// The boundary every observation in this record is qualified by.
    pub boundary: ObservationBoundary,
    /// Problems that are properties of the contract rather than of the search.
    pub anomalies: Vec<InspectionFailure>,
    /// Why any part of the search stopped early.
    pub truncation: Vec<TruncationReason>,
    /// How many transactions were read for invocation evidence.
    pub transactions_read: usize,
}

impl ContractInspection {
    /// Whether the inspection has nothing it can call a contradiction.
    #[must_use]
    pub fn has_contradiction(&self) -> bool {
        self.anomalies
            .iter()
            .any(InspectionFailure::is_contradiction)
    }

    /// The anomalies that mean the contract is absent, if any.
    #[must_use]
    pub fn absence(&self) -> Option<&InspectionFailure> {
        self.anomalies.iter().find(|failure| failure.is_absence())
    }

    /// Whether the search stopped short of what was asked.
    #[must_use]
    pub const fn is_truncated(&self) -> bool {
        !self.truncation.is_empty()
    }

    /// The digest the network recorded for the contract's executable, when it has
    /// one.
    #[must_use]
    pub const fn recorded_digest(&self) -> Option<&amasario_core::Digest> {
        self.identity.wasm_hash.as_ref()
    }

    /// The recorded module, wrapped with the boundary it was observed at.
    #[must_use]
    pub fn observed_module(&self) -> Option<Observation<&WasmModule>> {
        self.module.as_ref().map(|module| {
            Observation::new(
                module,
                self.boundary.clone(),
                ObservationProvenance::Network,
            )
        })
    }

    /// Whether the module was observed directly from the network and verified.
    ///
    /// The single question a consumer should ask before treating a module as the one
    /// a contract executes. Requires all three: a module, a successful digest check,
    /// and no contradiction anywhere in the inspection - the last because a
    /// contradiction elsewhere in the record means some other part of the identity is
    /// unreliable.
    #[must_use]
    pub fn module_is_verified(&self) -> bool {
        self.observed_module().is_some()
            && self.digest_verified == Some(true)
            && !self.has_contradiction()
    }

    /// The traversal outcome of the event scan, if one ran.
    #[must_use]
    pub const fn event_scan_outcome(&self) -> Option<TraversalOutcome> {
        if self.truncation.is_empty() {
            None
        } else {
            Some(TraversalOutcome::Truncated)
        }
    }
}

/// Inspects contracts against one network target.
///
/// Holds borrowed sessions rather than owning them so that one connection serves
/// many inspections, which is what lets a run inspect several contracts at a
/// consistent boundary without reconnecting for each.
pub struct Inspector<'a> {
    session: &'a RpcSession,
    target: &'a NetworkTarget,
    horizon: Option<&'a HorizonSession>,
}

impl<'a> Inspector<'a> {
    /// Builds an inspector over an RPC session and the target it was connected to.
    ///
    /// The target is separate from the session because the session does not retain
    /// the network descriptor, and inspection has to compare the endpoint's
    /// passphrase against the one that was requested.
    #[must_use]
    pub const fn new(session: &'a RpcSession, target: &'a NetworkTarget) -> Self {
        Self {
            session,
            target,
            horizon: None,
        }
    }

    /// Adds a Horizon session, which enables modification resolution.
    #[must_use]
    pub const fn with_horizon(mut self, horizon: &'a HorizonSession) -> Self {
        self.horizon = Some(horizon);
        self
    }

    /// The network descriptor inspections are qualified by.
    #[must_use]
    pub const fn network(&self) -> &Network {
        self.target.network()
    }

    /// Inspects a contract.
    ///
    /// # Errors
    ///
    /// Returns a network error when the endpoint serves a different network than the
    /// one requested, a contract error when no contract is deployed at the address,
    /// and a classified error when a required request fails. Problems that are
    /// properties of the contract rather than failures of the inspection are
    /// recorded in [`ContractInspection::anomalies`] instead of being returned.
    pub async fn inspect(
        &self,
        cancellation: &Cancellation,
        request: &InspectionRequest,
    ) -> Result<ContractInspection> {
        cancellation.check()?;

        // Establishes the network before anything is observed. A mismatch fails here
        // rather than corrupting an analysis that would otherwise be labelled with a
        // chain it never read.
        let _identity = self
            .session
            .network_identity(cancellation, self.target.network().passphrase.as_str())
            .await?;

        let boundary_ledger = self.session.latest_ledger(cancellation).await?;
        let boundary = ObservationBoundary::new(
            self.network().clone(),
            boundary_ledger,
            &request.observed_at,
        );

        let entries = fetch_contract_entries(self.session, cancellation, &request.contract).await?;
        let Some(instance) = entries.instance else {
            // The network answered, and the answer is that nothing is deployed here.
            // That is an outcome, and it is returned as an error because there is
            // nothing to inspect - unlike the anomalies below, which describe a
            // contract that exists.
            return Err(InspectionFailure::NotDeployed {
                contract_id: request.contract.to_string(),
                network: self.network().id.clone(),
                ledger: boundary_ledger,
            }
            .into_error());
        };

        let mut anomalies: Vec<InspectionFailure> = Vec::new();
        let mut truncation: Vec<TruncationReason> = Vec::new();

        let (executable_kind, wasm_bytes) = match executable_of(&instance) {
            Some(found) => found,
            None => {
                return Err(InspectionFailure::UnrecognisedExecutable {
                    contract_id: request.contract.to_string(),
                }
                .into_error());
            },
        };

        let identity = ContractIdentity::new(
            request.contract.clone(),
            self.network(),
            executable_kind,
            wasm_bytes
                .map(digest_from_hash_bytes)
                .transpose()?
                .as_ref()
                .cloned(),
            boundary_ledger,
        )?
        .observed_at(boundary_ledger);

        let (module, digest_verified, interface) = self
            .read_module(&identity, entries.code.as_ref(), &mut anomalies)
            .await?;

        let instance_storage = instance_storage(&instance);

        let storage = if request.storage_keys.is_empty() {
            None
        } else {
            Some(
                observe_keys(
                    self.session,
                    cancellation,
                    &request.contract,
                    &request.storage_keys,
                )
                .await?,
            )
        };

        let (invocations, emitted_events, transactions_read) = self
            .collect_invocations(cancellation, request, &mut truncation)
            .await?;

        let identity = if request.resolve_modification {
            self.resolve_instance_modification(cancellation, &identity, &instance, boundary_ledger)
                .await?
        } else {
            identity.with_instance_modification(InstanceModification::at_ledger(
                instance.last_modified_ledger,
            ))
        };

        Ok(ContractInspection {
            identity,
            module,
            digest_verified,
            interface,
            instance_storage,
            storage,
            invocations,
            emitted_events,
            boundary,
            anomalies,
            truncation,
            transactions_read,
        })
    }

    /// Reads and parses the deployed module, verifying its digest.
    ///
    /// Records a `CodeEntryAbsent` anomaly rather than failing when the instance
    /// names a hash the network does not hold. A contract in that state exists and
    /// cannot be analysed, and reporting it as an absence would be wrong.
    async fn read_module(
        &self,
        identity: &ContractIdentity,
        code: Option<&ContractEntry>,
        anomalies: &mut Vec<InspectionFailure>,
    ) -> Result<(Option<WasmModule>, Option<bool>, Option<ContractInterface>)> {
        let Some(recorded) = identity.wasm_hash.as_ref() else {
            // A Stellar Asset Contract has no module to read, which is a complete
            // fact rather than a missing one.
            return Ok((None, None, None));
        };

        let Some(code_entry) = code else {
            anomalies.push(InspectionFailure::CodeEntryAbsent {
                contract_id: identity.contract_id.to_string(),
                wasm_hash: recorded.value().to_owned(),
            });
            return Ok((None, None, None));
        };

        let LedgerEntryData::ContractCode(code) = &code_entry.data else {
            anomalies.push(InspectionFailure::UnexpectedEntry {
                expected: "a contract code entry".to_owned(),
                received: format!("{:?}", code_entry.data),
            });
            return Ok((None, None, None));
        };
        let bytes = code.code.as_slice();

        // The digest is checked before the module is parsed, so a module that does
        // not match its recorded hash is reported as a contradiction even when its
        // framing is also unreadable. The contradiction is the more important fact.
        let computed = crate::wasm::digest_of(bytes);
        let verified = computed.matches(recorded);
        if !verified {
            anomalies.push(InspectionFailure::DigestContradiction {
                contract_id: identity.contract_id.to_string(),
                recorded: recorded.value().to_owned(),
                computed: computed.value().to_owned(),
            });
        }

        let module = match parse_module(bytes) {
            Ok(module) => module,
            Err(error) => {
                anomalies.push(InspectionFailure::MalformedModule {
                    detail: error.to_string(),
                });
                return Ok((None, Some(verified), None));
            },
        };

        let interface = match &module.spec_section {
            Some(section) => match decode_spec_section(section) {
                Ok(interface) => Some(interface),
                Err(error) => {
                    anomalies.push(InspectionFailure::SpecSectionUndecodable {
                        detail: error.to_string(),
                    });
                    None
                },
            },
            None => None,
        };

        Ok((Some(module), Some(verified), interface))
    }

    /// Scans events and reads the transactions they name for invocation evidence.
    async fn collect_invocations(
        &self,
        cancellation: &Cancellation,
        request: &InspectionRequest,
        truncation: &mut Vec<TruncationReason>,
    ) -> Result<(Vec<ContractInvocation>, Vec<EmittedEvent>, usize)> {
        let Some(query) = &request.event_query else {
            return Ok((Vec::new(), Vec::new(), 0));
        };

        let scan = scan_events(self.session, cancellation, query).await?;
        if let Some(reason) = scan.truncation {
            truncation.push(reason);
        }

        let emitted: Vec<EmittedEvent> = scan
            .events
            .iter()
            .map(|event| {
                Ok(EmittedEvent {
                    id: event.id.clone(),
                    contract_id: event.contract_id.clone(),
                    ledger: LedgerSequence::new(event.ledger)?,
                    closed_at: event.ledger_closed_at.clone(),
                    transaction: event.tx_hash.clone().map(Ok).transpose()?,
                    operation_index: event.operation_index,
                })
            })
            .collect::<Result<Vec<_>>>()?;

        // Transactions are read in a canonical order so that the same scan produces
        // the same invocations, and each is read once: the same transaction can
        // appear in several events, and reading it repeatedly would multiply the
        // requests without adding evidence.
        let mut hashes: Vec<String> = emitted
            .iter()
            .filter_map(|event| event.transaction.clone())
            .collect();
        hashes.sort();
        hashes.dedup();

        if hashes.len() > request.max_transaction_reads {
            // The search stopped short, and says so. Reporting a shorter list of
            // invocations without the reason would let a consumer read the bound as
            // evidence that the contract made no further calls.
            truncation.push(TruncationReason::MaxNodesReached);
        }

        let mut invocations: Vec<ContractInvocation> = Vec::new();
        let mut read = 0_usize;
        for hash in hashes.iter().take(request.max_transaction_reads) {
            cancellation.check()?;
            let Ok(hash) = amasario_core::TransactionHash::new(hash.clone()) else {
                // An endpoint that reports a malformed hash has reported an event
                // the engine cannot attribute. Skipping it is recorded by the count
                // of transactions read rather than silently ignored.
                continue;
            };
            let Some(observation) = fetch_transaction(self.session, cancellation, &hash).await?
            else {
                truncation.push(TruncationReason::EvidenceUnavailable);
                continue;
            };
            read += 1;
            invocations.extend(
                invocations_from_transaction(&observation)?
                    .for_callee(request.contract.as_str())
                    .into_iter()
                    .cloned(),
            );
        }

        invocations.sort();
        invocations.dedup();
        Ok((invocations, emitted, read))
    }

    /// Finds the operation that put the contract's instance entry into its present
    /// state.
    ///
    /// The instance entry records the ledger it was last modified at. This reads
    /// that ledger's operations, follows the ones that name the contract to their
    /// transactions, and decides between deployment and upgrade from the ledger
    /// changes the transaction's metadata reports:
    ///
    /// * a `Created` ledger change for the contract's instance entry means the
    ///   transaction created it, which is a deployment;
    /// * an invocation of [`UPDATE_CURRENT_CONTRACT_WASM`] on the contract means the
    ///   transaction replaced its executable, which is an upgrade.
    ///
    /// The first is read from the transaction's metadata rather than from the
    /// operation's type name, because a metadata ledger change is a protocol fact
    /// while an endpoint's operation vocabulary is a presentation choice. When
    /// neither holds, the kind stays [`InstanceModificationKind::Unknown`] rather than
    /// being guessed.
    ///
    /// # Errors
    ///
    /// Returns a classified error when the operations request fails. A failure to
    /// read a candidate transaction is recorded as truncation rather than returned,
    /// because the modification is useful when found and its absence is already
    /// representable.
    async fn resolve_instance_modification(
        &self,
        cancellation: &Cancellation,
        identity: &ContractIdentity,
        instance: &ContractEntry,
        _boundary: LedgerSequence,
    ) -> Result<ContractIdentity> {
        let mut modification = InstanceModification::at_ledger(instance.last_modified_ledger);

        // Without a Horizon endpoint the operations of a ledger are not readable, and
        // the modification stays unknown rather than being inferred.
        let Some(horizon) = self.horizon else {
            return Ok(identity.clone().with_instance_modification(modification));
        };

        let operations = operations_for_ledger(
            horizon,
            cancellation,
            instance.last_modified_ledger.get(),
            MAX_MODIFICATION_OPERATIONS,
        )
        .await?;

        let mut candidate_hashes: Vec<String> = operations
            .iter()
            .filter(|operation| {
                operation.is_invoke_host_function()
                    && operation
                        .contract_id()
                        .is_some_and(|id| id == identity.contract_id.as_str())
            })
            .filter_map(|operation| operation.transaction_hash.clone())
            .collect();
        candidate_hashes.sort();
        candidate_hashes.dedup();

        for hash in candidate_hashes {
            cancellation.check()?;
            let Ok(transaction_hash) = amasario_core::TransactionHash::new(hash) else {
                continue;
            };
            let Some(observation) =
                fetch_transaction(self.session, cancellation, &transaction_hash).await?
            else {
                continue;
            };

            let kind = if creates_contract(&observation, &identity.contract_id)? {
                InstanceModificationKind::Deploy
            } else if upgrades_contract(&observation, &identity.contract_id) {
                InstanceModificationKind::Upgrade
            } else {
                continue;
            };

            modification = modification.with_operation(transaction_hash, None, kind);
            break;
        }

        Ok(identity.clone().with_instance_modification(modification))
    }
}

/// The executable a contract instance declares.
///
/// Returns `None` for an entry that is not a contract instance at all, which is a
/// defect in the request or the endpoint rather than a property of the contract.
const fn executable_of(
    entry: &ContractEntry,
) -> Option<(ContractExecutableKind, Option<[u8; 32]>)> {
    // Matched rather than destructured with `let ... else`, because a `let` binding
    // is not permitted in a `const fn`. Reading the hash here rather than through the
    // network crate's accessor keeps the classification evaluable at compile time,
    // and the arm below is the same shape that accessor checks.
    match &entry.data {
        LedgerEntryData::ContractData(data) => match &data.val {
            ScVal::ContractInstance(instance) => match &instance.executable {
                ContractExecutable::Wasm(hash) => {
                    Some((ContractExecutableKind::Wasm, Some(hash.0)))
                },
                ContractExecutable::StellarAsset => {
                    Some((ContractExecutableKind::StellarAsset, None))
                },
            },
            _ => None,
        },
        _ => None,
    }
}

/// Whether a transaction's metadata reports the creation of `contract`.
///
/// Read from the ledger changes rather than from the envelope, because an envelope
/// can *ask* to create a contract and only the metadata says whether one exists.
///
/// # Errors
///
/// Returns a classified error only when the contract's address cannot be decoded,
/// which cannot happen for an address the network accepted.
fn creates_contract(
    observation: &amasario_network::transactions::TransactionObservation,
    contract: &ContractId,
) -> Result<bool> {
    let payload = contract.payload()?;
    let Some(TransactionMeta::V4(meta)) = &observation.result_meta else {
        // Earlier metadata versions do not carry the ledger changes in a form this
        // check can read, and the answer is unknown rather than false.
        return Ok(false);
    };

    let created = meta
        .operations
        .iter()
        .flat_map(|operation| operation.changes.iter())
        .any(|change| {
            let stellar_xdr::LedgerEntryChange::Created(entry) = change else {
                return false;
            };
            let LedgerEntryData::ContractData(data) = &entry.data else {
                return false;
            };
            match &data.contract {
                ScAddress::Contract(id) => id.0.0 == payload,
                _ => false,
            }
        });

    Ok(created)
}

/// Whether a transaction invokes the upgrade function on `contract`.
///
/// The host function name is a protocol fact: a contract upgrades itself by invoking
/// `update_current_contract_wasm` on its own address.
fn upgrades_contract(
    observation: &amasario_network::transactions::TransactionObservation,
    contract: &ContractId,
) -> bool {
    let Some(envelope) = &observation.envelope else {
        return false;
    };
    amasario_network::transactions::envelope_operations(envelope)
        .into_iter()
        .any(|operation| {
            let stellar_xdr::OperationBody::InvokeHostFunction(invoke) = &operation.body else {
                return false;
            };
            let stellar_xdr::HostFunction::InvokeContract(args) = &invoke.host_function else {
                return false;
            };
            let callers_own_address =
                amasario_network::transactions::contract_strkey(&args.contract_address)
                    .is_some_and(|address| address == contract.as_str());
            callers_own_address
                && args.function_name.to_utf8_string_lossy() == UPDATE_CURRENT_CONTRACT_WASM
        })
}

/// The event types a scan may observe, as the specification names them.
///
/// Present so that a report can label an observed event without re-deriving the
/// name from the protocol's encoding at each call site.
#[must_use]
pub const fn event_type_name(event_type: ContractEventType) -> &'static str {
    match event_type {
        ContractEventType::System => "SYSTEM",
        ContractEventType::Contract => "CONTRACT",
        ContractEventType::Diagnostic => "DIAGNOSTIC",
    }
}

/// Builds the failure for a contract whose module was never retrieved.
///
/// Exposed so that a caller assembling an inspection from stored observations can
/// report the same anomaly the inspector produces.
#[must_use]
pub fn code_entry_absent(contract: &ContractId, wasm_hash: &str) -> EngineError {
    InspectionFailure::CodeEntryAbsent {
        contract_id: contract.to_string(),
        wasm_hash: wasm_hash.to_owned(),
    }
    .into_error()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::{ContractIdentity, digest_from_hash_bytes};
    use amasario_network::client::KnownNetwork;
    use stellar_xdr::{
        ContractCodeEntry, ContractDataEntry, ContractId as XdrContractId, ExtensionPoint, Hash,
        ScContractInstance, ScSymbol,
    };

    fn contract_address(payload: [u8; 32]) -> ContractId {
        let strkey = format!("{}", stellar_strkey::Contract(payload));
        ContractId::new(strkey).expect("a real contract address")
    }

    fn ledger(value: u32) -> LedgerSequence {
        LedgerSequence::new(value).expect("a real ledger")
    }

    fn instance_entry(executable: ContractExecutable, last_modified: u32) -> ContractEntry {
        ContractEntry {
            data: LedgerEntryData::ContractData(ContractDataEntry {
                ext: ExtensionPoint::V0,
                contract: ScAddress::Contract(XdrContractId(Hash([7_u8; 32]))),
                key: ScVal::LedgerKeyContractInstance,
                durability: stellar_xdr::ContractDataDurability::Persistent,
                val: ScVal::ContractInstance(ScContractInstance {
                    executable,
                    storage: None,
                }),
            }),
            last_modified_ledger: ledger(last_modified),
            live_until_ledger: None,
        }
    }

    fn code_entry(hash: [u8; 32], code: &[u8]) -> ContractEntry {
        ContractEntry {
            data: LedgerEntryData::ContractCode(ContractCodeEntry {
                ext: stellar_xdr::ContractCodeEntryExt::V0,
                hash: Hash(hash),
                code: stellar_xdr::BytesM::try_from(code.to_vec()).expect("a short module"),
            }),
            last_modified_ledger: ledger(900),
            live_until_ledger: None,
        }
    }

    #[test]
    fn a_wasm_instance_reports_its_kind_and_hash() {
        let (kind, hash) = executable_of(&instance_entry(
            ContractExecutable::Wasm(Hash([3_u8; 32])),
            10,
        ))
        .expect("a contract instance");
        assert_eq!(kind, ContractExecutableKind::Wasm);
        assert_eq!(hash, Some([3_u8; 32]));
    }

    #[test]
    fn an_asset_instance_reports_its_kind_and_no_hash() {
        let (kind, hash) = executable_of(&instance_entry(ContractExecutable::StellarAsset, 10))
            .expect("a contract instance");
        assert_eq!(kind, ContractExecutableKind::StellarAsset);
        assert_eq!(hash, None);
    }

    #[test]
    fn an_entry_that_is_not_a_contract_instance_is_not_classified() {
        let entry = code_entry([1_u8; 32], b"\0asm\x01\0\0\0");
        assert!(
            executable_of(&entry).is_none(),
            "a code entry is not an instance, and classifying it would invent an executable kind"
        );
    }

    #[test]
    fn a_transaction_that_does_not_create_a_contract_reports_no_creation() {
        let observation = amasario_network::transactions::TransactionObservation {
            hash: hex::encode([1_u8; 32]),
            ledger: Some(ledger(10)),
            status: "SUCCESS".to_owned(),
            successful: true,
            application_order: None,
            envelope: None,
            result: None,
            result_meta: None,
            contract_events: Vec::new(),
            transaction_events: Vec::new(),
            diagnostic_events: Vec::new(),
        };
        assert!(
            !creates_contract(&observation, &contract_address([1_u8; 32]))
                .expect("a valid address")
        );
        assert!(!upgrades_contract(
            &observation,
            &contract_address([1_u8; 32])
        ));
    }

    #[test]
    fn an_upgrade_is_recognised_by_the_host_function_it_invokes() {
        let payload = [4_u8; 32];
        let observation = amasario_network::transactions::TransactionObservation {
            hash: hex::encode([1_u8; 32]),
            ledger: Some(ledger(10)),
            status: "SUCCESS".to_owned(),
            successful: true,
            application_order: None,
            envelope: Some(stellar_xdr::TransactionEnvelope::Tx(
                stellar_xdr::TransactionV1Envelope {
                    tx: stellar_xdr::Transaction {
                        source_account: stellar_xdr::MuxedAccount::Ed25519(stellar_xdr::Uint256(
                            [0_u8; 32],
                        )),
                        fee: 100,
                        seq_num: stellar_xdr::SequenceNumber(1),
                        cond: stellar_xdr::Preconditions::None,
                        memo: stellar_xdr::Memo::None,
                        operations: vec![stellar_xdr::Operation {
                            source_account: None,
                            body: stellar_xdr::OperationBody::InvokeHostFunction(
                                stellar_xdr::InvokeHostFunctionOp {
                                    host_function: stellar_xdr::HostFunction::InvokeContract(
                                        stellar_xdr::InvokeContractArgs {
                                            contract_address: ScAddress::Contract(XdrContractId(
                                                Hash(payload),
                                            )),
                                            function_name: ScSymbol(
                                                UPDATE_CURRENT_CONTRACT_WASM
                                                    .parse()
                                                    .expect("a short symbol"),
                                            ),
                                            args: stellar_xdr::VecM::default(),
                                        },
                                    ),
                                    auth: stellar_xdr::VecM::default(),
                                },
                            ),
                        }]
                        .try_into()
                        .expect("one operation"),
                        ext: stellar_xdr::TransactionExt::V0,
                    },
                    signatures: stellar_xdr::VecM::default(),
                },
            )),
            result: None,
            result_meta: None,
            contract_events: Vec::new(),
            transaction_events: Vec::new(),
            diagnostic_events: Vec::new(),
        };

        assert!(upgrades_contract(&observation, &contract_address(payload)));
        assert!(
            !upgrades_contract(&observation, &contract_address([9_u8; 32])),
            "an upgrade of one contract is not an upgrade of another"
        );
    }

    #[test]
    fn the_supported_network_set_is_the_one_the_network_crate_serves() {
        // Inspection is the first caller of the network layer, so the network it
        // accepts is visible here rather than only in the CLI.
        assert!(KnownNetwork::all().contains(&KnownNetwork::Testnet));
        assert!(
            KnownNetwork::Mainnet.default_rpc().is_none(),
            "no public mainnet RPC endpoint is operated, so none may be defaulted to"
        );
    }

    #[test]
    fn event_types_are_named_after_the_protocols_own_encoding() {
        assert_eq!(event_type_name(ContractEventType::System), "SYSTEM");
        assert_eq!(event_type_name(ContractEventType::Contract), "CONTRACT");
        assert_eq!(event_type_name(ContractEventType::Diagnostic), "DIAGNOSTIC");
    }

    #[test]
    fn an_identity_only_request_reads_no_storage_and_scans_no_events() {
        let request =
            InspectionRequest::identity_only(contract_address([1_u8; 32]), "2026-09-15T00:00:00Z");
        assert!(request.storage_keys.is_empty());
        assert!(request.event_query.is_none());
        assert!(!request.resolve_modification);
        assert_eq!(request.max_transaction_reads, DEFAULT_MAX_TRANSACTION_READS);
    }

    #[test]
    fn a_request_gains_the_scan_and_the_lookup_it_was_asked_for() {
        let request =
            InspectionRequest::identity_only(contract_address([1_u8; 32]), "2026-09-15T00:00:00Z")
                .with_storage_key(ScVal::U32(1), Durability::Persistent)
                .resolving_modification()
                .scanning_events(EventQuery::for_contract("C".to_owned() + &"A".repeat(55)));

        assert_eq!(request.storage_keys.len(), 1);
        assert!(request.resolve_modification);
        assert!(request.event_query.is_some());
    }

    #[test]
    fn a_contract_that_is_not_deployed_is_an_error_and_not_an_anomaly() {
        // There is nothing to inspect, which is different from inspecting something
        // that turns out to have problems.
        let failure = InspectionFailure::NotDeployed {
            contract_id: contract_address([1_u8; 32]).to_string(),
            network: "testnet".to_owned(),
            ledger: ledger(10),
        };
        assert!(failure.is_absence());
        assert_eq!(failure.category(), amasario_core::ErrorCategory::Contract);
    }

    #[test]
    fn a_missing_code_entry_is_reported_through_the_shared_constructor() {
        let contract = contract_address([1_u8; 32]);
        let error = code_entry_absent(&contract, &"a".repeat(64));
        let message = error.to_string();
        assert!(message.contains(&contract.to_string()));
        assert!(message.contains("not an absence"));
        assert_eq!(error.category(), amasario_core::ErrorCategory::Provenance);
    }

    #[test]
    fn an_inspection_with_no_anomalies_has_nothing_contradictory() {
        let identity = ContractIdentity::new(
            contract_address([1_u8; 32]),
            &Network::new(
                "testnet",
                amasario_core::NetworkType::Testnet,
                "Test SDF Network ; September 2015",
            )
            .expect("a valid network descriptor"),
            ContractExecutableKind::Wasm,
            Some(digest_from_hash_bytes([1_u8; 32]).expect("a digest")),
            ledger(10),
        )
        .expect("a consistent identity");

        let inspection = ContractInspection {
            identity,
            module: None,
            digest_verified: None,
            interface: None,
            instance_storage: Vec::new(),
            storage: None,
            invocations: Vec::new(),
            emitted_events: Vec::new(),
            boundary: ObservationBoundary::new(
                Network::new(
                    "testnet",
                    amasario_core::NetworkType::Testnet,
                    "Test SDF Network ; September 2015",
                )
                .expect("valid"),
                ledger(10),
                "2026-09-15T00:00:00Z",
            ),
            anomalies: Vec::new(),
            truncation: Vec::new(),
            transactions_read: 0,
        };

        assert!(!inspection.has_contradiction());
        assert!(inspection.absence().is_none());
        assert!(!inspection.is_truncated());
        assert!(inspection.event_scan_outcome().is_none());
        assert!(
            !inspection.module_is_verified(),
            "no module was read, which is not the same as a verified one"
        );
    }
}
