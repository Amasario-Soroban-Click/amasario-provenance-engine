//! The callee half of the first-party reference contract.
//!
//! # Why this contract exists
//!
//! The engine's whole claim is that it reports what it observed rather than what it
//! assumes. Before this contract existed, the strongest evidence behind the
//! module-level half of that claim was a set of hand-assembled byte sequences
//! (`fixtures/wasm/`) plus a single borrowed testnet contract. The hand-assembled
//! modules are the right tool for what they do - they assert what the engine must
//! *refuse* to invent - but no module produced by the real Soroban SDK appeared
//! anywhere in the corpus. `contractspecv0` decoding, `contractenvmetav0` reading and
//! the section walk were therefore never exercised against bytes the SDK actually
//! emitted.
//!
//! So this is a real contract, built by the real SDK, whose interface and imports are
//! then asserted. It is deliberately small: every function here exists to give the
//! engine something specific to find.
//!
//! # What it deliberately contains
//!
//! * `record` - takes an [`Address`] and calls `require_auth` on it, so the module
//!   carries a real authorization surface rather than being a pure view contract.
//! * `record` also writes to persistent storage, extends its TTL, and emits an event.
//! * `sequence` - a read-only function over that storage, so the contract has a
//!   callable that performs no state change.
//!
//! Those three facts are what the paired `caller` contract's cross-contract call
//! resolves to at runtime, and what the integration suite asserts against.

#![no_std]

use soroban_sdk::{contract, contractimpl, contracttype, symbol_short, Address, Env};

/// Storage keys, declared as a `contracttype` so the engine decodes a real union
/// rather than only scalar parameter types.
#[contracttype]
#[derive(Clone)]
pub enum DataKey {
    /// The number of calls recorded for an account.
    Sequence(Address),
}

/// Events are an unauthenticated side channel; the engine's live analysis reads
/// diagnostic events, so a contract that emits none would be a weaker fixture.
const RECORDED: soroban_sdk::Symbol = symbol_short!("recorded");

#[contract]
pub struct Ledger;

#[contractimpl]
impl Ledger {
    /// Records a call against `account` and returns the new sequence number.
    ///
    /// `require_auth` is what makes this contract interesting on a ledger: it cannot
    /// be called by anyone but the account itself. The engine's live tests invoke it
    /// through the `caller` contract, which is the case that produces a real
    /// cross-contract dependency edge rather than a self-call.
    #[allow(deprecated)]
    pub fn record(env: Env, account: Address, amount: i128) -> u32 {
        account.require_auth();

        let key = DataKey::Sequence(account.clone());
        let mut sequence: u32 = env.storage().persistent().get(&key).unwrap_or(0);
        sequence += 1;

        env.storage().persistent().set(&key, &sequence);
        env.storage().persistent().extend_ttl(&key, 100, 1_000);

        env.events()
            .publish((RECORDED, account.clone()), (amount, sequence));

        sequence
    }

    /// Returns the sequence recorded for `account`, or zero if it has none.
    ///
    /// Read-only, and the function the `caller` contract calls, so the observed
    /// dependency edge is produced by a real invocation rather than inferred.
    pub fn sequence(env: Env, account: Address) -> u32 {
        env.storage()
            .persistent()
            .get(&DataKey::Sequence(account))
            .unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use soroban_sdk::{testutils::Address as _, Env};

    /// `require_auth` must actually gate the write, and the sequence must advance
    /// exactly once per recorded call.
    #[test]
    fn record_requires_auth_and_advances_the_sequence() {
        let env = Env::default();
        env.mock_all_auths();

        let contract_id = env.register(Ledger, ());
        let client = LedgerClient::new(&env, &contract_id);
        let account = Address::generate(&env);

        assert_eq!(client.sequence(&account), 0);
        assert_eq!(client.record(&account, &10), 1);
        assert_eq!(client.record(&account, &20), 2);
        assert_eq!(client.sequence(&account), 2);
    }
}
