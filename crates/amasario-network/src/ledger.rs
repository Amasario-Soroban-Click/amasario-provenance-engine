//! Ledger observations.
//!
//! A ledger is where the engine's temporal model meets the network. Two things are
//! needed from it, and they are different questions.
//!
//! The **observation boundary** is the ledger an analysis is bounded by: it comes
//! from [`RpcSession::latest_ledger`](crate::rpc::RpcSession::latest_ledger) and
//! qualifies every fact in a document.
//!
//! A **ledger record** is a fact about the chain - its close time, its hash, its
//! metadata - and is what lets the engine say that something happened *at* a ledger
//! rather than merely having been *seen* at one. That distinction is the core's:
//! an observation is not a creation fact, and this module supplies the ledger close
//! time that turns a sequence number into something a reader can place in time.
//!
//! # Bounds
//!
//! [`ledger_range`] refuses a range larger than its bound rather than truncating
//! it silently, because a caller that asked for a range and received a shorter one
//! has no way to tell the difference afterwards.

use amasario_core::{Cancellation, EngineError, LedgerSequence, Result};
use stellar_rpc_client::LedgerStart;
use stellar_xdr::{LedgerCloseMeta, LedgerHeaderHistoryEntry};

use crate::rpc::RpcSession;

/// The largest number of ledgers one request may cover.
///
/// A deliberate bound rather than a protocol limit: each ledger record carries a
/// header and metadata, and asking for thousands in one response is how a tool is
/// rate limited or runs out of memory.
pub const MAX_LEDGER_RANGE: u32 = 200;

/// A ledger record read from the network.
#[derive(Debug, Clone, PartialEq)]
pub struct LedgerObservation {
    /// The ledger sequence.
    pub sequence: LedgerSequence,
    /// The ledger's hash, as the endpoint reported it.
    pub hash: String,
    /// The ledger's close time, as the endpoint reported it.
    pub close_time: String,
    /// The decoded header, when the endpoint supplied one.
    pub header: Option<LedgerHeaderHistoryEntry>,
    /// The decoded close metadata, when the endpoint supplied one.
    pub meta: Option<LedgerCloseMeta>,
}

/// Reads one ledger.
///
/// Returns `Ok(None)` when the endpoint does not have that ledger - which happens
/// for a ledger older than its retention window - so that "the endpoint cannot
/// show you this ledger" stays distinct from "the request failed".
///
/// # Errors
///
/// Returns a classified error when the request fails.
pub async fn fetch_ledger(
    session: &RpcSession,
    cancellation: &Cancellation,
    sequence: LedgerSequence,
) -> Result<Option<LedgerObservation>> {
    let mut records = fetch_ledgers(session, cancellation, sequence, 1).await?;
    Ok(records.pop())
}

/// Reads a bounded range of ledgers starting at `start`.
///
/// # Errors
///
/// Returns [`EngineError::Configuration`] when `limit` is zero or exceeds
/// [`MAX_LEDGER_RANGE`], and a classified error when the request fails.
pub async fn fetch_ledgers(
    session: &RpcSession,
    cancellation: &Cancellation,
    start: LedgerSequence,
    limit: u32,
) -> Result<Vec<LedgerObservation>> {
    if limit == 0 {
        return Err(EngineError::Configuration(
            "a ledger range of zero would request nothing".to_owned(),
        ));
    }
    if limit > MAX_LEDGER_RANGE {
        return Err(EngineError::Configuration(format!(
            "a ledger range of {limit} exceeds the bound of {MAX_LEDGER_RANGE}; a larger range is \
             refused rather than truncated, because a caller cannot tell a truncated range from a \
             complete one"
        )));
    }

    let response = session.ledgers(cancellation, start, limit as usize).await?;

    Ok(response
        .ledgers
        .into_iter()
        .map(|ledger| LedgerObservation {
            sequence: LedgerSequence::new(ledger.sequence).unwrap_or(start),
            hash: ledger.hash,
            close_time: ledger.ledger_close_time,
            header: ledger.header_json,
            meta: ledger.metadata_json,
        })
        .collect())
}

/// The latest ledger, which is the boundary an analysis is bounded by.
///
/// # Errors
///
/// Returns a classified error when the endpoint cannot be reached.
pub async fn observation_boundary(
    session: &RpcSession,
    cancellation: &Cancellation,
) -> Result<LedgerSequence> {
    session.latest_ledger(cancellation).await
}

/// Expands a ledger range into its sequences.
///
/// # Errors
///
/// Returns [`EngineError::Configuration`] when `to` precedes `from`, or when the
/// range exceeds [`MAX_LEDGER_RANGE`]. An inverted range is an error rather than an
/// empty list because a caller that inverted it has a bug, and an empty list would
/// hide it.
pub fn ledger_range(from: LedgerSequence, to: LedgerSequence) -> Result<Vec<LedgerSequence>> {
    if to < from {
        return Err(EngineError::Configuration(format!(
            "ledger range is inverted: {} precedes {}",
            to.get(),
            from.get()
        )));
    }
    let span = to.get() - from.get() + 1;
    if span > MAX_LEDGER_RANGE {
        return Err(EngineError::Configuration(format!(
            "ledger range of {span} exceeds the bound of {MAX_LEDGER_RANGE}"
        )));
    }

    (from.get()..=to.get())
        .map(LedgerSequence::new)
        .collect::<Result<Vec<_>>>()
}

/// Starts a ledger read from a cursor, for a caller resuming an interrupted paging
/// loop.
#[must_use]
pub fn ledger_start_from_cursor(cursor: impl Into<String>) -> LedgerStart {
    LedgerStart::Cursor(cursor.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::NetworkTarget;
    use amasario_core::EngineConfig;
    use wiremock::matchers::{body_string_contains, method};
    use wiremock::{Mock, MockServer};

    fn ledger(n: u32) -> LedgerSequence {
        LedgerSequence::new(n).expect("a real ledger")
    }

    fn target(server: &MockServer) -> NetworkTarget {
        NetworkTarget::custom(
            "mock",
            "Test SDF Network ; September 2015",
            &server.uri(),
            None,
            &EngineConfig::default(),
        )
        .expect("a local endpoint is valid")
    }

    #[test]
    fn a_range_expands_inclusively() {
        let range = ledger_range(ledger(10), ledger(13)).expect("a small range");
        assert_eq!(
            range.iter().map(|l| l.get()).collect::<Vec<_>>(),
            vec![10, 11, 12, 13]
        );
        assert_eq!(
            ledger_range(ledger(10), ledger(10)).expect("single").len(),
            1
        );
    }

    #[test]
    fn an_inverted_range_is_refused_rather_than_returning_nothing() {
        // A caller that inverted the bounds has a bug, and an empty list would hide
        // it behind a plausible-looking result.
        let error = ledger_range(ledger(20), ledger(10)).expect_err("inverted");
        assert!(error.to_string().contains("inverted"));
    }

    #[test]
    fn a_range_larger_than_the_bound_is_refused_rather_than_truncated() {
        let error =
            ledger_range(ledger(1), ledger(MAX_LEDGER_RANGE + 1)).expect_err("over the bound");
        assert!(error.to_string().contains("exceeds the bound"));
    }

    #[tokio::test]
    async fn a_range_of_zero_is_refused_before_any_request() {
        let server = MockServer::start().await;
        let session = RpcSession::connect(&target(&server)).expect("connects");
        let error = fetch_ledgers(&session, &Cancellation::new(), ledger(5), 0)
            .await
            .expect_err("zero is refused");
        assert!(error.to_string().contains("zero"));
    }

    #[tokio::test]
    async fn a_ledger_is_read_with_its_close_time() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(body_string_contains("getLedgers"))
            .respond_with(crate::test_support::result(
                serde_json::json!({                    "latestLedger": 500,
                        "latestLedgerCloseTime": 0,
                        "oldestLedger": 1,
                        "oldestLedgerCloseTime": 0,
                    "cursor": "c",
                    "ledgers": [{
                        "hash": "abc",
                        "sequence": 12,
                        "ledgerCloseTime": "1757894400",
                        "headerXdr": "",
                        "metadataXdr": ""
                    }]
                }),
            ))
            .mount(&server)
            .await;

        let session = RpcSession::connect(&target(&server)).expect("connects");
        let observation = fetch_ledger(&session, &Cancellation::new(), ledger(12))
            .await
            .expect("the request succeeds")
            .expect("the ledger exists");

        assert_eq!(observation.sequence.get(), 12);
        assert_eq!(observation.hash, "abc");
        // The close time is what places the ledger in time; without it a sequence
        // number says nothing about when something happened.
        assert_eq!(observation.close_time, "1757894400");
    }

    #[tokio::test]
    async fn a_ledger_the_endpoint_does_not_hold_is_absent_rather_than_an_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(body_string_contains("getLedgers"))
            .respond_with(crate::test_support::result(
                serde_json::json!({                    "latestLedger": 500,
                        "latestLedgerCloseTime": 0,
                        "oldestLedger": 400,
                        "oldestLedgerCloseTime": 0,
                    "cursor": "c",
                    "ledgers": []
                }),
            ))
            .mount(&server)
            .await;

        let session = RpcSession::connect(&target(&server)).expect("connects");
        let observation = fetch_ledger(&session, &Cancellation::new(), ledger(5))
            .await
            .expect("the endpoint answered");
        assert!(observation.is_none());
    }
}
