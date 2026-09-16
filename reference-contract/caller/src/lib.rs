//! The caller half of the first-party reference contract.
//!
//! # What makes this the interesting half
//!
//! `contractimport!` reads the *compiled* `callee` module and generates a typed client
//! from its `contractspecv0` section. The client is how this contract reaches the
//! callee, so at runtime there is a genuine cross-contract invocation to observe, and
//! the call graph the live test expects is written down in this file.
//!
//! # What it does *not* do, because measuring said so
//!
//! The intuitive expectation is that importing a specification republishes it, so that
//! the callee's functions would appear among this contract's. Built and decoded, they
//! do not: this module's `contractspecv0` declares `observe` and `record_via` and
//! nothing else. The dependency is real, but it is not a fact this module's interface
//! section states, and the integration suite asserts the absence rather than assuming
//! the presence - a tool that merged an imported specification into its importer's
//! interface would credit a contract with functions it does not declare.
//!
//! Both halves are ours, both are built from source in this repository, and the
//! expected call graph is written down. That is the difference between this and the
//! single borrowed testnet contract the live suite used to depend on: nothing here can
//! go quiet, and a failure means the engine changed rather than the world did.

#![no_std]

use soroban_sdk::{contract, contractimpl, Address, Env};

/// The callee's specification, embedded at compile time from its built module.
mod callee {
    soroban_sdk::contractimport!(file = "wasm/reference_callee.wasm");
}

#[contract]
pub struct Observer;

#[contractimpl]
impl Observer {
    /// Returns the callee's recorded sequence for `account`.
    ///
    /// A read-only cross-contract call: the caller changes no state, so this is the
    /// invocation the live test uses to establish the edge without needing the
    /// account's signature.
    pub fn observe(env: Env, ledger: Address, account: Address) -> u32 {
        callee::Client::new(&env, &ledger).sequence(&account)
    }

    /// Records a call against `account` in the callee and returns the new sequence.
    ///
    /// The authenticated path: the callee's `require_auth` is satisfied by the account
    /// authorizing through this contract, which is the nested-authorization case a
    /// provenance tool most needs to report accurately.
    pub fn record_via(env: Env, ledger: Address, account: Address, amount: i128) -> u32 {
        callee::Client::new(&env, &ledger).record(&account, &amount)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::{testutils::Address as _, Env};

    /// The constructed call graph, asserted with an oracle independent of the engine.
    ///
    /// This is the ground truth the integration suite compares the engine against: the
    /// SDK's own test host executes the same `caller -> callee` call, so if this passes
    /// and the engine disagrees, the engine is wrong rather than the world.
    #[test]
    fn caller_invokes_callee_and_reports_its_state() {
        let env = Env::default();
        // `mock_all_auths` is not enough here, and the difference between the two is the
        // reason this test is worth having. That call mocks authorization for the *root*
        // invocation only: it errors when `require_auth` is reached for an address that
        // did not authorize the invocation the test itself made. `record_via` calls no
        // `require_auth` - the *callee* does, one frame down - so the account never
        // appears in the root invocation and the call fails with `Auth: InvalidAction`.
        // The SDK's own documentation for the second variant names this exact shape as
        // its reason to exist. Worth recording because the failure is a runtime panic
        // rather than a type error, so nothing about the signature suggests it.
        env.mock_all_auths_allowing_non_root_auth();

        let ledger_id = env.register(callee::WASM, ());
        let observer_id = env.register(Observer, ());
        let account = Address::generate(&env);

        let ledger = callee::Client::new(&env, &ledger_id);
        let observer = ObserverClient::new(&env, &observer_id);

        // Nothing recorded yet, and the caller must report the callee's own state
        // rather than a default of its own.
        assert_eq!(observer.observe(&ledger_id, &account), 0);

        // The authenticated path, driven through the caller, must reach the callee.
        assert_eq!(observer.record_via(&ledger_id, &account, &7), 1);
        assert_eq!(ledger.sequence(&account), 1);
        assert_eq!(observer.observe(&ledger_id, &account), 1);
    }
}
