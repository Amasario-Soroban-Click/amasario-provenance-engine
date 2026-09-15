//! Responses read off a live network, and what the engine must make of them.
//!
//! # Why these exist next to [`crate::recordings`]
//!
//! [`crate::recordings`] holds documents *shaped like* documented endpoint responses,
//! written by hand so that each adapter's parser is what decodes them. That catches a
//! field renamed or a path changed. What it cannot catch is the class of defect this
//! corpus exists for, because a hand-written document is written by someone who has
//! already assumed what the endpoint sends:
//!
//! * the host's diagnostic events for a nested call carry the callee as raw bytes in a
//!   topic and the caller in the event's own `contract_id` - the reverse of the reading
//!   that suggests itself, and a reading that reversed them recovered no call nesting
//!   and reported every contract as having no dependencies;
//! * the endpoint's top-level `diagnosticEventsXdr` is populated, and the
//!   `diagnosticEventsXdr` nested inside the response's `events` object - the one the
//!   bundled client's accessor reads - is empty;
//! * a `getEvents` request outside the event index's own floor is rejected outright,
//!   even though `getHealth` reports a lower floor than the index serves.
//!
//! Each of those was found by running the engine against the chain, and each is now
//! asserted against what the endpoint returned. These files are the only fixtures in
//! the corpus that are not produced by a builder: they are transcriptions, and
//! `captures::all` is the index of them.
//!
//! A transcription is faithful in content and re-indented in form. The response was read
//! as one line and is committed pretty-printed, because a fixture a reviewer cannot read
//! is a fixture nobody checks; no member was added, removed or altered by that, and the
//! base64 XDR is the same characters in the same order.
//!
//! # What they are not
//!
//! A capture is not a claim that the analysis of it is correct, and it is not a claim
//! about the chain now. It is a recording of what one endpoint answered one request on
//! one day, with the request and the context recorded beside it so that a reader can
//! repeat it. Nothing here is an analysis result.
//!
//! The capture date is recorded because a live network moves: testnet is periodically
//! reset, and a captured transaction hash will not resolve forever. A test that asserted
//! the captured transaction against the live chain would fail for that reason alone,
//! which is precisely why these bytes are committed instead.

use serde_json::Value;

use crate::corpus::Corpus;

/// One response captured from a live network, with the request that produced it.
#[derive(Debug, Clone, Copy)]
pub struct Capture {
    /// The fixture directory the response is committed under.
    pub directory: &'static str,
    /// The fixture file name.
    pub file: &'static str,
    /// The JSON-RPC method that produced it.
    pub method: &'static str,
    /// The network it was read from.
    pub network: &'static str,
    /// The day it was read, as `YYYY-MM-DD`.
    pub captured: &'static str,
    /// The request parameters, as the caller sent them.
    pub request: &'static str,
    /// What the engine must make of the response, in one sentence.
    pub note: &'static str,
}

impl Capture {
    /// The response as it was written to disk.
    ///
    /// # Panics
    ///
    /// Panics when the capture is absent, which is a defect in the corpus rather than a
    /// condition a test should tolerate.
    #[must_use]
    pub fn text(&self) -> String {
        Corpus::read(self.directory, self.file)
    }

    /// The response parsed as JSON.
    ///
    /// # Panics
    ///
    /// Panics when the capture is not valid JSON, which would mean the transcription had
    /// been damaged.
    #[must_use]
    pub fn value(&self) -> Value {
        serde_json::from_str(&self.text())
            .unwrap_or_else(|error| panic!("the capture {} is not valid JSON: {error}", self.file))
    }

    /// The path of the capture relative to the repository root.
    #[must_use]
    pub fn path(&self) -> String {
        Corpus::relative(self.directory, self.file)
    }

    /// The `result` member of the JSON-RPC envelope.
    ///
    /// # Panics
    ///
    /// Panics when the capture does not carry one, because a capture is a response to a
    /// call that succeeded.
    #[must_use]
    pub fn result(&self) -> Value {
        self.value()
            .get("result")
            .cloned()
            .unwrap_or_else(|| panic!("{} carries no result member", self.file))
    }
}

/// The contract whose activity the captured pages describe.
///
/// A live exchange contract on testnet: in the captured ledgers it is called by
/// accounts, and it calls another contract in turn, which makes it the smallest real
/// case of a nested call the corpus can hold. Nothing here is a claim about what the
/// contract is for.
pub const TESTNET_SUBJECT_CONTRACT: &str =
    "CAYPAQDKNWMHRATKU5DQ327VDHVRSIVK7UGVWT2A5SUZCUFTLUHXH2JA";

/// The transaction whose response holds the observed nested call.
pub const TESTNET_NESTED_TRANSACTION: &str =
    "9c4c8fc72b1a676f4b7d533e12bc2e0298ca66c088cded01a44631a3da9fb8b1";

/// The ledger [`TESTNET_NESTED_TRANSACTION`] was included in.
pub const TESTNET_NESTED_LEDGER: u32 = 4_693_490;

/// A transaction that called a contract directly, with no nested call.
///
/// Held alongside the nested one because the interesting assertion is a difference
/// between two real transactions: the same code path must recover nesting from one and
/// must not manufacture it for the other. This one carries a single `InvokeContract`
/// operation and nothing further, so its only call has no calling contract and there is
/// no edge to recover.
pub const TESTNET_DIRECT_TRANSACTION: &str =
    "dd2e50e6f73d49ef32f0279ced2f8451133019420e1fa32690a85d70f6f0879f";

/// The ledger [`TESTNET_DIRECT_TRANSACTION`] was included in.
pub const TESTNET_DIRECT_LEDGER: u32 = 4_696_781;

/// The contract [`TESTNET_DIRECT_TRANSACTION`] invoked, and nothing beyond it.
///
/// The operation named it as the callee of a `set_price` call. It is recorded here so
/// that the direct-call assertion can state which contract the transaction reached
/// rather than only that it reached something.
pub const TESTNET_DIRECT_CONTRACT: &str =
    "CAAL5BZAQ3W2G5CZX4HVLWNQIJISQ62ZNXXYCX2LONG4FFLRRYEYONZC";

/// `getHealth` on testnet, which is where the retention window's floor comes from.
pub const TESTNET_HEALTH: Capture = Capture {
    directory: "ledgers",
    file: "rpc-get-health.json",
    method: "getHealth",
    network: "testnet",
    captured: "2026-09-15",
    request: "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"getHealth\",\"params\":null}",
    note: "the node's own view of its retention window: the tip, the oldest ledger it \
           retains, and the window's length - the floor a bounded scan is measured against",
};

/// `getEvents` for one contract on testnet.
pub const TESTNET_EVENT_PAGE: Capture = Capture {
    directory: "ledgers",
    file: "rpc-get-events-contract.json",
    method: "getEvents",
    network: "testnet",
    captured: "2026-09-15",
    request: "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"getEvents\",\"params\":\
              {\"startLedger\":4693490,\"endLedger\":4696490,\"filters\":[{\"type\":\"contract\",\
              \"contractIds\":[\"CAYPAQDKNWMHRATKU5DQ327VDHVRSIVK7UGVWT2A5SUZCUFTLUHXH2JA\"]}],\
              \"pagination\":{\"limit\":5}}}",
    note: "a page of contract events for one contract, with the cursor a following page \
           is asked for: the shape the event scan reads, and the reason a full page is \
           not the end of a scan",
};

/// `getTransaction` for a transaction that made a nested call.
pub const TESTNET_NESTED_CALL: Capture = Capture {
    directory: "transactions",
    file: "rpc-get-transaction-nested-call.json",
    method: "getTransaction",
    network: "testnet",
    captured: "2026-09-15",
    request: "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"getTransaction\",\
              \"params\":{\"hash\":\"9c4c8fc72b1a676f4b7d533e12bc2e0298ca66c088cded01a44631a3da9fb8b1\"}}",
    note: "a successful transaction that entered one contract which called another: the \
           only real evidence of nesting in the corpus, and the response that disproved \
           the reading of the host's `fn_call` diagnostic that the engine had",
};

/// `getTransaction` for a transaction that called a contract directly.
pub const TESTNET_DIRECT_CALL: Capture = Capture {
    directory: "transactions",
    file: "rpc-get-transaction-direct-call.json",
    method: "getTransaction",
    network: "testnet",
    captured: "2026-09-15",
    request: "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"getTransaction\",\
              \"params\":{\"hash\":\"dd2e50e6f73d49ef32f0279ced2f8451133019420e1fa32690a85d70f6f0879f\"}}",
    note: "a successful transaction whose only call is one top-level `InvokeContract`: \
           the control case that separates \"this transaction made no nested call\" \
           from \"this engine cannot read nested calls\"",
};

/// Every capture, so that the corpus's index and the suites agree on the set.
#[must_use]
pub fn all() -> Vec<&'static Capture> {
    vec![
        &TESTNET_HEALTH,
        &TESTNET_EVENT_PAGE,
        &TESTNET_NESTED_CALL,
        &TESTNET_DIRECT_CALL,
    ]
}
