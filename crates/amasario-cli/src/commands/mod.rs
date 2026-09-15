//! The commands, and the observation step several of them share.
//!
//! # One observation, many answers
//!
//! `inspect`, `discover`, `dependencies`, `graph`, `impact`, `snapshot`, `verify` and
//! `report` all begin the same way: connect to a network, establish its identity, inspect
//! one contract at a boundary. That step lives here once rather than eight times, because
//! the specification's strongest requirement - that a network failure is never an empty
//! result - is a property of that step, and eight copies would be eight chances to get it
//! wrong.
//!
//! # The observation is bounded before it starts
//!
//! The event scan and the transaction bound come from [`BoundsArgs`], and the depth bound
//! reaches dependency resolution. Nothing here issues an unbounded sequence of requests:
//! an event scan is bounded by pages, a transaction read by a count, and a traversal by a
//! depth and a node count, all of which the specification requires and all of which travel
//! with the result rather than being left in a log.

pub mod dependencies;
pub mod diff;
pub mod discover;
pub mod export;
pub mod graph;
pub mod impact;
pub mod inspect;
pub mod provenance;
pub mod report;
pub mod snapshot;
pub mod verify;

use amasario_contract::{ContractInspection, InspectionRequest, Inspector};
use amasario_core::{
    Cancellation, ContractId, EngineError, EntityKind, EntityRef, LedgerSequence, Result,
    TruncationReason,
};
use amasario_dependency::{DependencySet, detect_from_invocations, resolve};
use amasario_graph::Graph;
use amasario_network::{EventQuery, RpcSession};
use serde::Serialize;

use crate::config::{BoundsArgs, TargetArgs, now_rfc3339};

/// Inspects the target contract as the command requested.
///
/// # Errors
///
/// Returns the classification the network layer produced, including the two outcomes that
/// are not failures of the tool: a contract that is not deployed, and a network that
/// answers with a different passphrase. Both are returned rather than turned into an
/// empty inspection.
pub async fn observe(target: &TargetArgs, bounds: &BoundsArgs) -> Result<ContractInspection> {
    // The bounds are validated before anything is contacted, so a depth beyond what the
    // specification permits fails with the argument that caused it rather than part-way
    // through a traversal.
    bounds.engine_config()?;

    let network = target.resolve()?;
    let contract = ContractId::new(&target.contract)?;

    // Connecting here establishes nothing about the chain; `inspect` checks the
    // passphrase before it records anything, which is why the check is inside the
    // inspection rather than here.
    let session = RpcSession::connect(&network)?;
    let cancellation = Cancellation::new();
    let inspector = Inspector::new(&session, &network);

    let mut request = InspectionRequest::identity_only(contract, now_rfc3339());
    request.max_transaction_reads = bounds.max_transactions;
    if bounds.scan_events {
        // The window is built here rather than defaulted inside the network layer, so
        // that the two choices a caller can make - how far back, and how many pages -
        // are visible in one place. The default is the recent window; see
        // `DEFAULT_LOOKBACK_LEDGERS` for why starting at the retention floor is the
        // one thing a dependency scan must not do.
        let mut query = EventQuery::for_contract(target.contract.clone())
            .with_max_pages(bounds.max_event_pages);
        query = match bounds.from_ledger {
            Some(ledger) => query.from(LedgerSequence::new(ledger)?),
            None => query.recent(bounds.lookback),
        };
        request = request.scanning_events(query);
    }

    inspector.inspect(&cancellation, &request).await
}

/// The entity reference for the target contract.
///
/// # Errors
///
/// Returns a validation error when the address is not a well-formed entity identifier.
pub fn subject_of(target: &TargetArgs) -> Result<EntityRef> {
    EntityRef::new(EntityKind::Contract, target.contract.as_str())
}

/// Resolves what the inspected contract depends on.
///
/// # Errors
///
/// Returns a dependency error when a candidate cannot be constructed, which the detection
/// pass already refuses to do, and a configuration error when the depth bound is zero.
pub fn dependencies_of(
    subject: &EntityRef,
    inspection: &ContractInspection,
    depth: u32,
) -> Result<DependencySet> {
    let detection =
        detect_from_invocations(subject, &inspection.boundary, &inspection.invocations)?;
    resolve(
        subject.clone(),
        Some(inspection.boundary.clone()),
        &detection.candidates,
        depth as usize,
    )
}

/// Builds the typed graph from a resolved dependency set.
///
/// # Errors
///
/// Returns a graph error when the set contains an edge the graph may not hold.
pub fn graph_of(subject: &EntityRef, set: &DependencySet) -> Result<Graph> {
    Graph::from_dependencies(subject.to_string(), set)
}

/// Serialises a value, classifying a failure as an internal defect.
///
/// # Errors
///
/// Returns an internal error when a type the engine owns cannot be serialised, which is
/// always a defect in the engine rather than in the input.
pub fn to_value<T: Serialize>(value: &T) -> Result<serde_json::Value> {
    serde_json::to_value(value).map_err(|error| {
        EngineError::Internal(format!(
            "a type the engine owns could not be serialised: {error}"
        ))
    })
}

/// The truncation reasons as their stable wire names.
#[must_use]
pub fn truncation_names(reasons: &[TruncationReason]) -> Vec<&'static str> {
    reasons.iter().map(|reason| reason.as_str()).collect()
}
