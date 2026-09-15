//! Test-only responders for driving the RPC transport against a local server.
//!
//! # Why these exist
//!
//! The JSON-RPC client generates its own request identifiers and rejects a
//! response whose identifier does not match the call it is waiting on, with
//! "request ID=n is not a pending call". A canned response that hardcodes an
//! identifier therefore fails for reasons that have nothing to do with the
//! behaviour under test. These responders echo the identifier from the request
//! they are answering, so a test exercises the engine's handling of a response
//! rather than the client's bookkeeping.
//!
//! # Why the network is mocked at all
//!
//! The tests must not depend on the public testnet. A test that reaches the
//! internet is not reproducible, fails in a sandbox, and - most importantly -
//! cannot ask the endpoint to return a rate limit, a malformed body or an
//! out-of-retention error on demand. Every one of those paths is a behaviour this
//! crate is responsible for, so every one of them has to be reachable in a test.

use serde_json::Value;
use wiremock::{Request, Respond, ResponseTemplate};

/// A JSON-RPC success responder that echoes the caller's request identifier.
#[derive(Debug, Clone)]
pub struct RpcSuccess(pub Value);

impl Respond for RpcSuccess {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "jsonrpc": "2.0",
            "id": request_id(request),
            "result": self.0
        }))
    }
}

/// A JSON-RPC error responder that echoes the caller's request identifier.
#[derive(Debug, Clone)]
pub struct RpcFailure {
    /// The JSON-RPC error code.
    pub code: i32,
    /// The endpoint's message.
    pub message: String,
}

impl Respond for RpcFailure {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "jsonrpc": "2.0",
            "id": request_id(request),
            "error": { "code": self.code, "message": self.message }
        }))
    }
}

/// A successful JSON-RPC responder carrying `result`.
#[must_use]
pub fn result(result: Value) -> RpcSuccess {
    RpcSuccess(result)
}

/// A JSON-RPC error responder.
#[must_use]
pub fn failure(code: i32, message: &str) -> RpcFailure {
    RpcFailure {
        code,
        message: message.to_owned(),
    }
}

/// The identifier from a request body, defaulting to `1` when it cannot be read.
///
/// The default only applies to a body this crate did not generate, which would
/// already be a defect worth surfacing as a mismatch rather than as a panic here.
fn request_id(request: &Request) -> Value {
    serde_json::from_slice::<Value>(&request.body)
        .ok()
        .and_then(|body| body.get("id").cloned())
        .unwrap_or_else(|| Value::from(1))
}

/// A `getHealth` result for a node holding `oldest..=latest`.
#[must_use]
pub fn health(latest: u32, oldest: u32) -> Value {
    serde_json::json!({
        "status": "healthy",
        "latestLedger": latest,
        "oldestLedger": oldest,
        "ledgerRetentionWindow": latest - oldest
    })
}

/// A `getLedgerEntries` result carrying one entry.
#[must_use]
pub fn entries(latest_ledger: u32, entry: Value) -> Value {
    serde_json::json!({
        "entries": [entry],
        "latestLedger": latest_ledger
    })
}

/// An empty `getLedgerEntries` result.
#[must_use]
pub fn no_entries(latest_ledger: u32) -> Value {
    serde_json::json!({
        "entries": null,
        "latestLedger": latest_ledger
    })
}

/// A single ledger entry as the endpoint reports it.
#[must_use]
pub fn entry(key: &str, xdr: &str, last_modified: u32, live_until: u32) -> Value {
    serde_json::json!({
        "key": key,
        "xdr": xdr,
        "lastModifiedLedgerSeq": last_modified,
        "liveUntilLedgerSeq": live_until
    })
}
