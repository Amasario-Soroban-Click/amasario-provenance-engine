//! Bounded event scanning.
//!
//! Contract events are the engine's primary evidence for what a contract *did*:
//! an event names the contract that emitted it, the transaction that produced it
//! and the ledger it was emitted at. That is what turns "these two contracts are
//! mentioned together" into "this contract invoked that one, at this ledger, in
//! this transaction" - a claim with evidence rather than an association.
//!
//! # Why a scan is always bounded, and always says so
//!
//! Three separate bounds are in force, and each reports itself through
//! [`TraversalOutcome`] and
//! [`TruncationReason`]:
//!
//! * **pages**, because an unbounded scan is an unbounded number of requests;
//! * **the node's retention window**, because the documented behaviour of
//!   `getEvents` is to fail when the start ledger is outside what the node
//!   retains (<https://developers.stellar.org/docs/data/apis/rpc/api-reference/methods/getEvents>).
//!   The scan reads the window from the node and clamps to it, so a request
//!   outside it is never made;
//! * **cancellation**, so a caller that stops a run gets a result marked as
//!   stopped rather than one that looks complete.
//!
//! The clamping deserves emphasis. A caller asking to scan from a ledger the node
//! has forgotten cannot be given that range, and reporting only the range that
//! *was* available - without saying the rest was unavailable - would present a
//! partial answer as a whole one. The scan therefore records
//! [`EventScan::clamped_to_oldest_ledger`] when it discards part of the requested
//! range, and marks the outcome `Truncated`.

use amasario_core::{
    Cancellation, EngineError, LedgerSequence, Result, TraversalOutcome, TruncationReason,
};
use stellar_rpc_client::EventStart;
use tracing::debug;

use crate::rpc::{DEFAULT_EVENT_PAGE_LIMIT, Event, RpcSession};

/// What to scan for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventQuery {
    /// The contracts whose events are wanted. Empty means no contract filter, which
    /// is only meaningful for a scan that immediately filters by hand and is
    /// rejected here instead, because an unfiltered event scan over a busy network
    /// is the fastest way to be rate limited.
    pub contract_ids: Vec<String>,
    /// The ledger to scan from, inclusive. When absent the scan starts at the
    /// oldest ledger the node retains.
    pub from_ledger: Option<LedgerSequence>,
    /// How many events to request per page.
    pub page_limit: usize,
    /// How many pages may be read before the scan stops and says so.
    pub max_pages: usize,
}

impl EventQuery {
    /// A query for one contract, scanning from the oldest ledger the node retains.
    #[must_use]
    pub fn for_contract(contract_id: impl Into<String>) -> Self {
        Self {
            contract_ids: vec![contract_id.into()],
            from_ledger: None,
            page_limit: DEFAULT_EVENT_PAGE_LIMIT,
            max_pages: 10,
        }
    }

    /// Starts the scan at a specific ledger.
    #[must_use]
    pub const fn from(mut self, ledger: LedgerSequence) -> Self {
        self.from_ledger = Some(ledger);
        self
    }

    /// Sets the page size.
    #[must_use]
    pub const fn with_page_limit(mut self, limit: usize) -> Self {
        self.page_limit = limit;
        self
    }

    /// Sets the page bound.
    #[must_use]
    pub const fn with_max_pages(mut self, pages: usize) -> Self {
        self.max_pages = pages;
        self
    }
}

/// The result of a scan, including what it could not cover.
///
/// Not `PartialEq`, because the endpoint's own event type is not: comparing two
/// scans is a question about the events' content, which a consumer answers by
/// reading the events rather than by retaining the endpoint's wrapper.
#[derive(Debug, Clone)]
pub struct EventScan {
    /// The events found, in the order the endpoint returned them.
    pub events: Vec<Event>,
    /// Whether the scan reached the end of what it was asked to cover.
    pub outcome: TraversalOutcome,
    /// Why the scan stopped, when it did.
    pub truncation: Option<TruncationReason>,
    /// How many pages were read.
    pub pages_read: usize,
    /// The ledger the scan started from, after clamping.
    ///
    /// `None` when the scan stopped before it read the node's retention window,
    /// which happens only when it was cancelled first. Reporting a starting ledger
    /// it never used would claim an observation the scan did not make.
    pub scanned_from: Option<LedgerSequence>,
    /// Whether part of the requested range was outside the node's retention window
    /// and therefore not observed.
    pub clamped_to_oldest_ledger: bool,
}

impl EventScan {
    /// Whether the scan covered everything it was asked to.
    #[must_use]
    pub const fn is_complete(&self) -> bool {
        matches!(self.outcome, TraversalOutcome::Complete)
    }
}

/// Scans events under the query's bounds.
///
/// # Errors
///
/// Returns [`EngineError::Configuration`] for a query with no contract filter or a
/// zero page limit, and a classified error when a page request fails. A failure
/// aborts the scan rather than returning the pages read so far, because a partial
/// event set whose shortfall is invisible is precisely the output the specification
/// forbids.
pub async fn scan_events(
    session: &RpcSession,
    cancellation: &Cancellation,
    query: &EventQuery,
) -> Result<EventScan> {
    if query.contract_ids.is_empty() {
        return Err(EngineError::Configuration(
            "an event scan needs at least one contract to filter by; an unfiltered scan over a \
             busy network is the fastest way to be rate limited"
                .to_owned(),
        ));
    }
    if query.page_limit == 0 {
        return Err(EngineError::Configuration(
            "an event page limit of zero would request nothing and could never make progress"
                .to_owned(),
        ));
    }
    if query.max_pages == 0 {
        return Err(EngineError::Configuration(
            "an event scan page bound of zero would observe nothing; use a positive bound"
                .to_owned(),
        ));
    }

    // A scan cancelled before it began is reported as a scan that stopped, not as
    // an error. `TruncationReason::Cancelled` exists so that a caller can tell
    // "nothing was observed because the run was stopped" from "nothing was observed
    // because there was nothing to observe", and returning an error here would
    // discard that distinction.
    if cancellation.is_cancelled() {
        return Ok(EventScan {
            events: Vec::new(),
            outcome: TraversalOutcome::Truncated,
            truncation: Some(TruncationReason::Cancelled),
            pages_read: 0,
            scanned_from: None,
            clamped_to_oldest_ledger: false,
        });
    }

    // Read the retention window so the request is never made outside it.
    let status = session.node_status(cancellation).await?;

    let requested_from = query.from_ledger.unwrap_or(status.oldest_ledger);
    let clamped = requested_from < status.oldest_ledger;
    let scanned_from = if clamped {
        status.oldest_ledger
    } else {
        requested_from
    };

    if clamped {
        debug!(
            requested = requested_from.get(),
            oldest = status.oldest_ledger.get(),
            "the requested start ledger is outside the endpoint's retention window and was clamped"
        );
    }

    let mut events = Vec::new();
    let mut pages_read = 0_usize;
    let mut cursor: Option<String> = None;
    let mut truncation = None;

    while pages_read < query.max_pages {
        if cancellation.is_cancelled() {
            // A cancelled scan returns what it read, marked as stopped. Reporting
            // it as complete would be the failure this module exists to prevent.
            truncation = Some(TruncationReason::Cancelled);
            break;
        }

        let start = match &cursor {
            Some(value) => EventStart::Cursor(value.clone()),
            None => EventStart::Ledger(scanned_from.get()),
        };

        let page = session
            .events_page(cancellation, start, &query.contract_ids, query.page_limit)
            .await?;

        pages_read += 1;
        let received = page.events.len();
        events.extend(page.events);

        // A short page is the endpoint's way of saying it has nothing further to
        // give for this filter, so the scan is complete.
        if received < query.page_limit {
            cursor = None;
            break;
        }

        cursor = Some(page.cursor.clone());
        if page.cursor.is_empty() {
            // A full page with no cursor would mean the scan cannot continue and
            // cannot prove it reached the end. Stopping here and saying so is the
            // only honest option.
            truncation = Some(TruncationReason::EvidenceUnavailable);
            break;
        }
    }

    if truncation.is_none() && pages_read >= query.max_pages && cursor.is_some() {
        // The bound was reached with a cursor still in hand, so more events exist.
        truncation = Some(TruncationReason::MaxNodesReached);
    }

    // Clamping discards part of the requested range, so the outcome is truncated
    // even when the scan itself ran to the end of what the node retains.
    if clamped && truncation.is_none() {
        truncation = Some(TruncationReason::BoundaryReached);
    }

    let outcome = if truncation.is_some() {
        TraversalOutcome::Truncated
    } else {
        TraversalOutcome::Complete
    };

    Ok(EventScan {
        events,
        outcome,
        truncation,
        pages_read,
        scanned_from: Some(scanned_from),
        clamped_to_oldest_ledger: clamped,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::NetworkTarget;
    use amasario_core::EngineConfig;
    use wiremock::matchers::{body_string_contains, method};
    use wiremock::{Mock, MockServer};

    const CONTRACT: &str = "CAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAD2KM";

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

    fn event(id: &str, ledger: u32) -> serde_json::Value {
        serde_json::json!({
            "type": "contract",
            "ledger": ledger,
            "ledgerClosedAt": "2026-09-15T00:00:00Z",
            "contractId": CONTRACT,
            "id": id,
            "topic": ["AAAADwAAAAh0cmFuc2Zlcg=="],
            "value": "AAAAAQ==",
            "txHash": "00"
        })
    }

    fn events(ids: &[&str], ledger: u32, cursor: &str) -> crate::test_support::RpcSuccess {
        crate::test_support::result(serde_json::json!({
            "events": ids.iter().map(|id| event(id, ledger)).collect::<Vec<_>>(),
            "latestLedger": 1000,
            "latestLedgerCloseTime": "0",
            "oldestLedger": 100,
            "oldestLedgerCloseTime": "0",
            "cursor": cursor
        }))
    }

    async fn mount_health(server: &MockServer, latest: u32, oldest: u32) {
        Mock::given(method("POST"))
            .and(body_string_contains("getHealth"))
            .respond_with(crate::test_support::result(crate::test_support::health(
                latest, oldest,
            )))
            .mount(server)
            .await;
    }

    #[tokio::test]
    async fn a_short_page_completes_the_scan() {
        let server = MockServer::start().await;
        mount_health(&server, 1000, 100).await;
        Mock::given(method("POST"))
            .and(body_string_contains("getEvents"))
            .respond_with(events(&["1-1", "1-2"], 500, "c1"))
            .expect(1)
            .mount(&server)
            .await;

        let session = RpcSession::connect(&target(&server)).expect("connects");
        let scan = scan_events(
            &session,
            &Cancellation::new(),
            &EventQuery::for_contract(CONTRACT).with_page_limit(10),
        )
        .await
        .expect("the scan succeeds");

        assert_eq!(scan.events.len(), 2);
        assert!(scan.is_complete());
        assert_eq!(scan.truncation, None);
        assert_eq!(scan.pages_read, 1);
        assert!(!scan.clamped_to_oldest_ledger);
    }

    #[tokio::test]
    async fn a_full_page_follows_the_cursor_and_reads_the_next_page() {
        let server = MockServer::start().await;
        mount_health(&server, 1000, 100).await;

        // First page is full, so the scan must ask again with the cursor.
        Mock::given(method("POST"))
            .and(body_string_contains("getEvents"))
            .and(body_string_contains("startLedger"))
            .respond_with(events(&["1-1", "1-2"], 500, "c1"))
            .up_to_n_times(1)
            .expect(1)
            .mount(&server)
            .await;

        Mock::given(method("POST"))
            .and(body_string_contains("getEvents"))
            .and(body_string_contains("cursor"))
            .respond_with(events(&["1-3"], 501, "c2"))
            .expect(1)
            .mount(&server)
            .await;

        let session = RpcSession::connect(&target(&server)).expect("connects");
        let scan = scan_events(
            &session,
            &Cancellation::new(),
            &EventQuery::for_contract(CONTRACT).with_page_limit(2),
        )
        .await
        .expect("the scan succeeds");

        assert_eq!(scan.events.len(), 3);
        assert_eq!(scan.pages_read, 2);
        assert!(
            scan.is_complete(),
            "the second page was short and ended the scan"
        );
    }

    #[tokio::test]
    async fn a_scan_that_hits_the_page_bound_reports_itself_as_truncated() {
        // This is the assertion the module exists for: a bounded scan must not look
        // like a complete one.
        let server = MockServer::start().await;
        mount_health(&server, 1000, 100).await;

        Mock::given(method("POST"))
            .and(body_string_contains("getEvents"))
            .respond_with(events(&["1-1", "1-2"], 500, "c1"))
            .mount(&server)
            .await;

        let session = RpcSession::connect(&target(&server)).expect("connects");
        let scan = scan_events(
            &session,
            &Cancellation::new(),
            &EventQuery::for_contract(CONTRACT)
                .with_page_limit(2)
                .with_max_pages(1),
        )
        .await
        .expect("the scan succeeds");

        assert_eq!(scan.pages_read, 1);
        assert!(!scan.is_complete());
        assert_eq!(scan.truncation, Some(TruncationReason::MaxNodesReached));
    }

    #[tokio::test]
    async fn a_request_before_the_retention_window_is_clamped_and_reported() {
        // The documented behaviour of `getEvents` is to fail outside the node's
        // retention window, so the scan must never ask outside it - and must say
        // that it discarded part of the requested range.
        let server = MockServer::start().await;
        mount_health(&server, 1000, 900).await;

        Mock::given(method("POST"))
            .and(body_string_contains("getEvents"))
            .and(body_string_contains("\"startLedger\":900"))
            .respond_with(events(&[], 950, "c1"))
            .expect(1)
            .mount(&server)
            .await;

        let session = RpcSession::connect(&target(&server)).expect("connects");
        let scan = scan_events(
            &session,
            &Cancellation::new(),
            &EventQuery::for_contract(CONTRACT)
                .from(LedgerSequence::new(10).expect("a real ledger")),
        )
        .await
        .expect("the scan succeeds");

        assert!(scan.clamped_to_oldest_ledger);
        assert_eq!(scan.scanned_from.map(LedgerSequence::get), Some(900));
        assert!(!scan.is_complete());
        assert_eq!(scan.truncation, Some(TruncationReason::BoundaryReached));
    }

    #[tokio::test]
    async fn a_cancelled_scan_returns_what_it_read_marked_as_stopped() {
        let server = MockServer::start().await;
        mount_health(&server, 1000, 100).await;
        Mock::given(method("POST"))
            .and(body_string_contains("getEvents"))
            .respond_with(events(&["1-1", "1-2"], 500, "c1"))
            .mount(&server)
            .await;

        let cancellation = Cancellation::new();
        let session = RpcSession::connect(&target(&server)).expect("connects");

        // Cancel after the first page by cancelling before reading further: the
        // scan is asked to read more than one page, and the loop stops itself.
        let query = EventQuery::for_contract(CONTRACT)
            .with_page_limit(2)
            .with_max_pages(5);

        // Cancel now: the loop's pre-check stops the scan before its first request,
        // which must still be reported as a stopped scan rather than a complete one.
        cancellation.cancel();
        let scan = scan_events(&session, &cancellation, &query)
            .await
            .expect("a cancelled scan is not an error");

        assert_eq!(scan.pages_read, 0);
        assert!(!scan.is_complete());
        assert_eq!(scan.truncation, Some(TruncationReason::Cancelled));
        assert!(scan.truncation.expect("truncated").is_transient());
        // The scan never reached the node, so it cannot claim a starting ledger.
        assert_eq!(scan.scanned_from, None);
    }

    #[tokio::test]
    async fn a_maximum_depth_bound_is_not_reported_as_transient() {
        // A deliberate bound and a transient condition need different responses, so
        // the reasons must not be interchangeable.
        let server = MockServer::start().await;
        mount_health(&server, 1000, 100).await;
        Mock::given(method("POST"))
            .and(body_string_contains("getEvents"))
            .respond_with(events(&["1-1", "1-2"], 500, "c1"))
            .mount(&server)
            .await;

        let session = RpcSession::connect(&target(&server)).expect("connects");
        let scan = scan_events(
            &session,
            &Cancellation::new(),
            &EventQuery::for_contract(CONTRACT)
                .with_page_limit(2)
                .with_max_pages(1),
        )
        .await
        .expect("the scan succeeds");

        assert!(!scan.truncation.expect("truncated").is_transient());
    }

    #[tokio::test]
    async fn a_scan_without_a_contract_filter_is_rejected_before_any_request() {
        let server = MockServer::start().await;
        // No mock is mounted, so any request would fail: rejection must happen
        // before the network is contacted.
        let session = RpcSession::connect(&target(&server)).expect("connects");

        let error = scan_events(
            &session,
            &Cancellation::new(),
            &EventQuery {
                contract_ids: Vec::new(),
                from_ledger: None,
                page_limit: 10,
                max_pages: 1,
            },
        )
        .await
        .expect_err("an unfiltered scan is refused");
        assert!(error.to_string().contains("at least one contract"));
    }

    #[tokio::test]
    async fn a_zero_page_limit_is_rejected_rather_than_looping_forever() {
        let server = MockServer::start().await;
        let session = RpcSession::connect(&target(&server)).expect("connects");
        let error = scan_events(
            &session,
            &Cancellation::new(),
            &EventQuery::for_contract(CONTRACT).with_page_limit(0),
        )
        .await
        .expect_err("a zero page limit cannot make progress");
        assert!(error.to_string().contains("zero"));
    }

    #[tokio::test]
    async fn a_full_page_without_a_cursor_stops_and_says_the_evidence_was_unavailable() {
        // Continuing is impossible and completion cannot be proved, so the scan
        // reports the weakest honest claim rather than either extreme.
        let server = MockServer::start().await;
        mount_health(&server, 1000, 100).await;
        Mock::given(method("POST"))
            .and(body_string_contains("getEvents"))
            .respond_with(events(&["1-1", "1-2"], 500, ""))
            .mount(&server)
            .await;

        let session = RpcSession::connect(&target(&server)).expect("connects");
        let scan = scan_events(
            &session,
            &Cancellation::new(),
            &EventQuery::for_contract(CONTRACT)
                .with_page_limit(2)
                .with_max_pages(5),
        )
        .await
        .expect("the scan succeeds");

        assert_eq!(scan.truncation, Some(TruncationReason::EvidenceUnavailable));
        assert!(!scan.is_complete());
    }
}
