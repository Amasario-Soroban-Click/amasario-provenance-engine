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
    use soroban_sdk::{
        testutils::{Address as _, Events as _},
        Env,
    };

    /// The number of contract events published by the invocation just made.
    fn events(env: &Env) -> usize {
        env.events().all().events().len()
    }

    /// A deployed instance and a funded-looking account, so no test repeats setup.
    fn ledger(env: &Env) -> (LedgerClient<'_>, Address) {
        env.mock_all_auths();
        let contract_id = env.register(Ledger, ());
        (
            LedgerClient::new(env, &contract_id),
            Address::generate(env),
        )
    }

    /// `require_auth` must actually gate the write, and the sequence must advance
    /// exactly once per recorded call.
    #[test]
    fn record_requires_auth_and_advances_the_sequence() {
        let env = Env::default();
        let (client, account) = ledger(&env);

        assert_eq!(client.sequence(&account), 0);
        assert_eq!(client.record(&account, &10), 1);
        assert_eq!(client.record(&account, &20), 2);
        assert_eq!(client.sequence(&account), 2);
    }

    /// The authorization surface is real, not decorative.
    ///
    /// `mock_all_auths` is deliberately *not* called here. `record` calls
    /// `require_auth` on an account that authorized nothing, so the host must refuse
    /// the invocation. A contract whose `require_auth` could be skipped would still
    /// produce a module the engine reads correctly, but `record` claims an
    /// authorization requirement in its interface, and this is what makes that claim
    /// checkable rather than aspirational.
    #[test]
    #[should_panic(expected = "InvalidAction")]
    fn record_is_refused_without_the_account_s_authorization() {
        let env = Env::default();
        let contract_id = env.register(Ledger, ());
        let client = LedgerClient::new(&env, &contract_id);
        let account = Address::generate(&env);

        client.record(&account, &1);
    }

    /// One account's calls must not be reported against another's.
    #[test]
    fn sequence_is_tracked_per_account() {
        let env = Env::default();
        let (client, first) = ledger(&env);
        let second = Address::generate(&env);

        client.record(&first, &1);
        client.record(&first, &1);
        client.record(&second, &1);

        assert_eq!(client.sequence(&first), 2);
        assert_eq!(client.sequence(&second), 1);
    }

    /// An account that has never been recorded reads as zero rather than as an error.
    ///
    /// Worth pinning: the engine's read-only cross-contract probe calls this on
    /// accounts with no history, so an unwrap on a missing key would turn the
    /// commonest invocation into a network error.
    #[test]
    fn sequence_of_an_account_that_never_recorded_is_zero() {
        let env = Env::default();
        let (client, _) = ledger(&env);

        assert_eq!(client.sequence(&Address::generate(&env)), 0);
    }

    /// The event the engine's live analysis reads must actually be published.
    #[test]
    fn record_publishes_one_event() {
        let env = Env::default();
        let (client, account) = ledger(&env);

        assert_eq!(events(&env), 0);
        client.record(&account, &5);
        assert_eq!(events(&env), 1);
    }

    /// ...and the read-only function must not publish one, because the engine
    /// classifies observed state changes from what an invocation emitted.
    ///
    /// `env.events().all()` reports the events of the *most recent* invocation, not of
    /// the whole test, so each assertion here follows the call it is about. That is
    /// worth pinning rather than working around: the engine's live analysis attributes
    /// an event to the invocation that emitted it, so a contract whose reads looked
    /// like writes would be reported as changing state it does not touch.
    #[test]
    fn sequence_publishes_no_event() {
        let env = Env::default();
        let (client, account) = ledger(&env);

        client.sequence(&account);
        assert_eq!(events(&env), 0);

        client.record(&account, &1);
        assert_eq!(events(&env), 1);

        client.sequence(&account);
        assert_eq!(events(&env), 0);
    }

    /// The amount is recorded but does not influence the sequence, so a reader can
    /// tell the two apart in the event rather than having to assume one from the
    /// other.
    #[test]
    fn the_amount_does_not_change_the_sequence() {
        let env = Env::default();
        let (client, account) = ledger(&env);

        assert_eq!(client.record(&account, &0), 1);
        assert_eq!(client.record(&account, &-1), 2);
        assert_eq!(client.record(&account, &i128::MAX), 3);
        assert_eq!(client.sequence(&account), 3);
    }
}
