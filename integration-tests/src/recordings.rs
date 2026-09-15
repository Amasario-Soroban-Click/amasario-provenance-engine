//! Recorded endpoint responses, and what the engine must make of each.
//!
//! # These are recordings, not observations
//!
//! Nothing here was read off a live network, and nothing here is presented as an
//! analysis result. Each document is shaped like the documented Stellar RPC or Horizon
//! response it stands in for, so that the adapter's own parser is what decodes it. That
//! is the point: a mock that returned an engine struct would test the engine's ability
//! to read its own output, while a mock that returns the endpoint's bytes tests the
//! adapter - which is where an endpoint that moved or a field that changed is felt.
//!
//! # Why the errors are recorded too
//!
//! The distinction the specification insists on is between a request that failed and a
//! resource that is absent. A `404`, a `503`, a JSON-RPC "not found" code and a
//! truncated body are four different things, and the corpus holds one of each so that
//! the classification is asserted against bytes rather than against an enum variant
//! chosen by the test.

use serde_json::{Value, json};

/// One recorded response body, with the request it answers.
#[derive(Debug, Clone, Copy)]
pub struct Recording {
    /// The fixture directory the body is committed under.
    pub directory: &'static str,
    /// The fixture file name.
    pub file: &'static str,
    /// What the recording answers, in one sentence.
    pub note: &'static str,
    /// The body, as text, so that a fixture is the bytes a maintainer reviews.
    pub body: &'static str,
}

impl Recording {
    /// The body parsed as JSON.
    ///
    /// # Panics
    ///
    /// Panics when the recording is not valid JSON, which is a defect in this module
    /// rather than in the engine under test.
    #[must_use]
    pub fn value(&self) -> Value {
        serde_json::from_str(self.body).unwrap_or_else(|error| {
            panic!("the recording {} is not valid JSON: {error}", self.file)
        })
    }

    /// The body as it is written to `fixtures/`, pretty-printed and newline-terminated.
    ///
    /// A recording that is not valid JSON is written out verbatim, because that is the
    /// whole point of it: [`MALFORMED_BODY`] exists so that the engine's refusal to
    /// decode it can be asserted, and pretty-printing it would require first parsing
    /// it successfully, which is the one thing it must not do.
    #[must_use]
    pub fn rendered(&self) -> String {
        match serde_json::from_str::<Value>(self.body) {
            Ok(value) => {
                let mut text = serde_json::to_string_pretty(&value)
                    .expect("a value parsed from text serialises");
                text.push('\n');
                text
            },
            Err(_) => {
                let mut text = self.body.trim().to_owned();
                text.push('\n');
                text
            },
        }
    }

    /// Whether the body is valid JSON.
    #[must_use]
    pub fn is_json(&self) -> bool {
        serde_json::from_str::<Value>(self.body).is_ok()
    }
}

/// Horizon's page of transactions for one contract.
///
/// The two records differ in exactly one thing that matters: the second failed. A
/// dependency must be able to cite a failed invocation and must not present it as a
/// working call, so the adapter has to carry `successful` rather than assume it.
pub const HORIZON_TRANSACTIONS: Recording = Recording {
    directory: "transactions",
    file: "horizon-contract-transactions.json",
    note: "one page of a contract's Horizon transaction history: one successful call and one that failed",
    body: r#"
{
  "_links": {
    "self": { "href": "https://horizon-testnet.stellar.org/accounts/GABC/transactions?cursor=&limit=2&order=asc" },
    "next": { "href": "https://horizon-testnet.stellar.org/accounts/GABC/transactions?cursor=2&limit=2&order=asc" }
  },
  "_embedded": {
    "records": [
      {
        "id": "0000000000000000001",
        "hash": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        "ledger": 1043,
        "successful": true,
        "created_at": "2026-01-01T00:00:05Z",
        "source_account": "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF5",
        "operation_count": 1,
        "fee_charged": "100",
        "fee_bump": false,
        "envelope_xdr": "AAAAAA==",
        "result_xdr": "AAAAAA==",
        "result_meta_xdr": "AAAAAA==",
        "paging_token": "0000000000000000001"
      },
      {
        "id": "0000000000000000002",
        "hash": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
        "ledger": 1044,
        "successful": false,
        "created_at": "2026-01-01T00:00:10Z",
        "source_account": "GAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAWHF5",
        "operation_count": 1,
        "fee_charged": "100",
        "fee_bump": false,
        "envelope_xdr": "AAAAAA==",
        "result_xdr": "AAAAAA==",
        "result_meta_xdr": "AAAAAA==",
        "paging_token": "0000000000000000002"
      }
    ]
  }
}
"#,
};

/// Horizon's record of one ledger.
pub const HORIZON_LEDGER: Recording = Recording {
    directory: "ledgers",
    file: "horizon-ledger-1044.json",
    note: "one closed ledger as Horizon reports it, with its transaction counts",
    body: r#"
{
  "id": "0000000000000000044",
  "sequence": 1044,
  "hash": "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
  "transaction_count": 2,
  "successful_transaction_count": 1,
  "failed_transaction_count": 1,
  "closed_at": "2026-01-01T00:00:10Z",
  "protocol_version": 22,
  "paging_token": "1044"
}
"#,
};

/// Stellar RPC's answer to `getLatestLedger`.
pub const RPC_LATEST_LEDGER: Recording = Recording {
    directory: "ledgers",
    file: "rpc-latest-ledger.json",
    note: "the RPC endpoint's view of the chain tip, which is what the boundary's ledger is read from",
    body: r#"
{
  "jsonrpc": "2.0",
  "id": 1,
  "result": {
    "id": "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd",
    "protocolVersion": 22,
    "sequence": 1044,
    "closeTime": "1751328010"
  }
}
"#,
};

/// Stellar RPC's answer to `getNetwork` on testnet.
pub const RPC_NETWORK_TESTNET: Recording = Recording {
    directory: "ledgers",
    file: "rpc-network-testnet.json",
    note: "the network identity the endpoint reports, which is what the passphrase check compares against",
    body: r#"
{
  "jsonrpc": "2.0",
  "id": 1,
  "result": {
    "passphrase": "Test SDF Network ; September 2015",
    "protocolVersion": 22,
    "friendbotUrl": "https://friendbot.stellar.org"
  }
}
"#,
};

/// Stellar RPC's answer from a chain whose passphrase is not testnet's.
///
/// An endpoint that reports this while the caller asked for testnet is a different
/// chain, and the engine must refuse the run rather than analyse it under the wrong
/// name.
pub const RPC_NETWORK_FUTURENET: Recording = Recording {
    directory: "ledgers",
    file: "rpc-network-futurenet.json",
    note: "a network identity for a chain that is not the one the caller named",
    body: r#"
{
  "jsonrpc": "2.0",
  "id": 1,
  "result": {
    "passphrase": "Test SDF Future Network ; October 2022",
    "protocolVersion": 22,
    "friendbotUrl": "https://friendbot-futurenet.stellar.org"
  }
}
"#,
};

/// A successful `getLedgerEntries` response with no entries.
///
/// This is how an absent resource actually arrives from Stellar RPC: a successful
/// response whose `entries` array is empty, not an error. The distinction is the one the
/// engine must keep - an empty result is an answer about the chain, and a failure is not
/// - so the corpus holds the former and the classification tests below cover the latter.
pub const RPC_EMPTY_ENTRIES: Recording = Recording {
    directory: "contracts",
    file: "rpc-empty-entries.json",
    note: "a successful response reporting that no ledger entry matches, which is an absence rather than a failure",
    body: r#"
{
  "jsonrpc": "2.0",
  "id": 1,
  "result": {
    "entries": [],
    "latestLedger": 1044
  }
}
"#,
};

/// The JSON-RPC error an endpoint returns for a method it does not implement.
pub const RPC_ERROR_METHOD_NOT_FOUND: Recording = Recording {
    directory: "contracts",
    file: "rpc-error-method-not-found.json",
    note: "a method the endpoint does not implement, which is a permanent failure rather than a retryable one",
    body: r#"
{
  "jsonrpc": "2.0",
  "id": 1,
  "error": { "code": -32601, "message": "method not found" }
}
"#,
};

/// An HTTP response that arrived but was cut off part way.
pub const MALFORMED_BODY: Recording = Recording {
    directory: "contracts",
    file: "malformed-response.json",
    note: "a body that is not valid JSON, which must be reported as a defect rather than retried",
    body: r#"{ "jsonrpc": "2.0", "id": 1, "result": { "entries": ["#,
};

/// Every recording, so the generator and the drift check agree on the set.
#[must_use]
pub fn all() -> Vec<&'static Recording> {
    vec![
        &HORIZON_TRANSACTIONS,
        &HORIZON_LEDGER,
        &RPC_LATEST_LEDGER,
        &RPC_NETWORK_TESTNET,
        &RPC_NETWORK_FUTURENET,
        &RPC_EMPTY_ENTRIES,
        &RPC_ERROR_METHOD_NOT_FOUND,
        &MALFORMED_BODY,
    ]
}

/// A recorded HTTP status, and what the classification of it must be.
#[derive(Debug, Clone, Copy)]
pub struct StatusRecording {
    /// The status the endpoint returned.
    pub status: u16,
    /// Whether the engine must treat it as absent rather than as a failure.
    pub absent: bool,
    /// Whether the engine must treat it as worth retrying.
    pub transient: bool,
    /// The message the endpoint carried, where it carried one.
    pub detail: &'static str,
}

/// The statuses whose classification the network suite asserts.
///
/// Chosen so that each of the four combinations of "absent" and "transient" appears at
/// least once, because the interesting defect is a tool that collapses them: a `404`
/// treated as transient retries forever, and a `503` treated as absent reports a
/// contract as nonexistent because a server was busy.
#[must_use]
pub fn statuses() -> Vec<StatusRecording> {
    vec![
        StatusRecording {
            status: 404,
            absent: true,
            transient: false,
            detail: "Resource Missing",
        },
        StatusRecording {
            status: 429,
            absent: false,
            transient: true,
            detail: "Too Many Requests",
        },
        StatusRecording {
            status: 408,
            absent: false,
            transient: true,
            detail: "Request Timeout",
        },
        StatusRecording {
            status: 500,
            absent: false,
            transient: true,
            detail: "Internal Server Error",
        },
        StatusRecording {
            status: 502,
            absent: false,
            transient: true,
            detail: "Bad Gateway",
        },
        StatusRecording {
            status: 400,
            absent: false,
            transient: false,
            detail: "Bad Request",
        },
    ]
}

/// A recording that is a JSON body, for a test that wants to assert on its shape.
///
/// # Panics
///
/// Panics when the name is not one of [`all`], which is a defect in the test rather
/// than a runtime condition.
#[must_use]
pub fn named(file: &str) -> &'static Recording {
    all()
        .into_iter()
        .find(|recording| recording.file == file)
        .unwrap_or_else(|| panic!("{file} is not a recorded endpoint response"))
}

/// The transaction hashes the Horizon page carries, in the order Horizon returned them.
///
/// # Panics
///
/// Panics when the recording does not have the shape Horizon documents, which would
/// mean the recording had been edited into something the adapter cannot read.
#[must_use]
pub fn horizon_transaction_hashes() -> Vec<String> {
    HORIZON_TRANSACTIONS.value()["_embedded"]["records"]
        .as_array()
        .expect("Horizon embeds its records in an array")
        .iter()
        .map(|record| {
            record["hash"]
                .as_str()
                .expect("every Horizon transaction record has a hash")
                .to_owned()
        })
        .collect()
}

/// A JSON-RPC error body for one code.
#[must_use]
pub fn rpc_error(code: i32, message: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": 1,
        "error": { "code": code, "message": message }
    })
}
