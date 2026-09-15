//! Stellar network adapters: endpoints, transports, observations and failure
//! classification.
//!
//! This crate is the only place in the engine that touches a network. Everything
//! above it works with decoded observations and classified errors, which is what
//! allows the rest of the engine to be tested without a network at all.
//!
//! # Three rules this crate enforces
//!
//! **A failure is never an absence.** Every lookup returns either a value, an
//! explicit absence, or an error, and the three are distinct in the type. The
//! specification's strongest requirement - that a network failure must remain
//! distinguishable from "nothing was found" - is met here or not at all, because a
//! layer above cannot recover a distinction that was thrown away here.
//!
//! **Nothing is observed until the network is identified.** A passphrase mismatch
//! ([`rpc::RpcSession::network_identity`]) is fatal and is checked before any
//! analysis fact is collected, so an analysis cannot accumulate observations from
//! one chain and be labelled with another.
//!
//! **Every scope is bounded.** Retries are bounded by
//! [`RetryPolicy`](amasario_core::RetryPolicy), event scans by
//! [`events::EventQuery`], ledger ranges by [`ledger::MAX_LEDGER_RANGE`] and
//! collection pages by [`horizon::MAX_COLLECTION_LIMIT`]. Nothing here can issue an
//! unbounded sequence of requests, and a scope that stops early reports that it
//! stopped.
//!
//! # What this crate does not claim
//!
//! Amasario is not a security scanner. Nothing in this crate returns a verdict
//! about a contract's safety, and the absence of an adverse finding means only that
//! the engine did not look for one.
//!
//! # Example
//!
//! ```no_run
//! use amasario_core::{Cancellation, EngineConfig, Result};
//! use amasario_network::client::{KnownNetwork, NetworkTarget};
//! use amasario_network::rpc::RpcSession;
//!
//! # async fn run() -> Result<()> {
//! let config = EngineConfig::default();
//! let target = NetworkTarget::resolve(KnownNetwork::Testnet, None, None, &config)?;
//!
//! let session = RpcSession::connect(&target)?;
//! let cancellation = Cancellation::new();
//!
//! // Establishes that the endpoint serves the chain that was asked for, before
//! // anything is observed. A mismatch fails here rather than corrupting a result.
//! let identity = session
//!     .network_identity(&cancellation, target.network().passphrase.as_str())
//!     .await?;
//!
//! // The boundary every observation in the run is qualified by.
//! let boundary = session.latest_ledger(&cancellation).await?;
//!
//! println!("observing {} at ledger {}", identity.passphrase, boundary.get());
//! # Ok(())
//! # }
//! ```
//!
//! # Notes on the Stellar dependencies
//!
//! `stellar-rpc-client`, `stellar-xdr` and `stellar-strkey` are the Stellar
//! organisation's own crates. They are used rather than reimplemented because
//! method names, request parameters, XDR layouts and strkey encoding are protocol
//! facts: a locally invented approximation of any of them would be a guess dressed
//! as an implementation. The versions are aligned deliberately and are documented
//! in the workspace manifest, because `stellar-rpc-client` exposes `stellar-xdr`
//! types in its public API and two versions of that crate in one graph would not
//! interoperate.

#![deny(missing_docs)]
#![deny(unsafe_code)]

pub mod client;
pub mod contracts;
pub mod errors;
pub mod events;
pub mod horizon;
pub mod ledger;
pub mod operations;
pub mod rpc;
#[cfg(test)]
mod test_support;
pub mod transactions;

pub use client::{
    HorizonEndpoint, KnownNetwork, NetworkTarget, RpcEndpoint, network_id_for_passphrase,
};
pub use contracts::{
    ContractEntries, ContractEntry, contract_code_key, contract_data_key, contract_instance_key,
    fetch_contract_code, fetch_contract_entries, fetch_contract_instance, wasm_hash_of,
};
pub use errors::{classify, classify_code, classify_status, status_is_absent, status_is_transient};
pub use events::{EventQuery, EventScan, scan_events};
pub use horizon::{HorizonSession, HorizonTransaction, MAX_COLLECTION_LIMIT};
pub use operations::{
    OperationRecord, operations_for_account, operations_for_ledger, operations_for_transaction,
};
pub use rpc::{LedgerEntry, NetworkIdentity, NodeStatus, RpcSession, worst_case_retry_wait};
pub use transactions::{
    TransactionObservation, created_contract_entries, envelope_operations, fetch_transaction,
    invoked_contracts,
};

#[cfg(test)]
mod tests {
    use super::*;
    use amasario_core::EngineConfig;

    #[test]
    fn the_adapters_are_reachable_from_the_crate_root() {
        // A consumer should not have to know which module a concept lives in to
        // name it, and a re-export that went missing would be a breaking change
        // discovered downstream rather than here.
        let config = EngineConfig::default();
        let target = NetworkTarget::resolve(KnownNetwork::Testnet, None, None, &config)
            .expect("testnet has defaults");
        assert_eq!(target.rpc().as_str(), client::TESTNET_RPC);

        let endpoint = RpcEndpoint::new("https://rpc.example.test").expect("valid");
        assert_eq!(endpoint.as_str(), "https://rpc.example.test");

        assert_eq!(
            network_id_for_passphrase("Test SDF Network ; September 2015"),
            KnownNetwork::Testnet.published_network_id()
        );
    }

    #[test]
    fn a_source_may_not_be_reached_by_a_convenient_default() {
        // The crate must not expose a way to build an endpoint without validating
        // it, because a credential-bearing URL would then travel into reports.
        assert!(RpcEndpoint::new("https://token:secret@rpc.example.test").is_err());
        assert!(HorizonEndpoint::new("https://token@horizon.example.test").is_err());
    }

    #[test]
    fn the_error_classification_is_reachable_without_naming_a_module() {
        use amasario_core::ErrorCategory;
        let error = classify_status("https://rpc.example.test", 429, "slow down");
        assert_eq!(error.category(), ErrorCategory::Network);
        assert!(error.retryable());
        assert!(status_is_absent(404));
        assert!(status_is_transient(503));
    }
}
