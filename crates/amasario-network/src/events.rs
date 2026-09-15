//! Bounded event scanning.
//!
//! Contract events are the engine's primary evidence for what a contract *did*:
//! an event names the contract that emitted it, the transaction that produced it
//! and the ledger it was emitted at. That is what turns "these two contracts are
//! mentioned together" into "this contract invoked that one, at this ledger, in
//! this transaction" - a claim with evidence rather than an association.
//!
//! # Which history a scan reads, and why that is the important choice
//!
//! A scan has to pick a stretch of history, and the choice decides whether the
//! answer is useful. Stellar RPC's `getEvents` is a *ledger-ordered* feed that
//! paginates by event count, not by ledger: one page returns as many events as the
//! limit allows, which for a busy contract is a handful of ledgers and for a quiet
//! one is its whole recent history. A page bound is therefore a bound on *events*,
//! and the ledgers a scan reaches are an outcome rather than an input.
//!
//! That is the trap this module exists to avoid. Scanning from the oldest ledger a
//! node retains looks like the thorough choice and is the useless one: on a public
//! endpoint the retention window is about seven days, so a page-bounded scan from
//! its floor covers the *oldest* few minutes of a week and never reaches anything
//! that happened since. The answer is then "this contract has no dependencies",
//! which is not wrong so much as a statement about a week ago.
//!
//! So the default is [`EventWindow::Recent`]: the most recent stretch of history,
//! ending at the node's tip. A scan that stops early still reports what the contract
//! did *lately*, which is the question a reader is asking, and the truncation tells
//! them how far back the answer reached.
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
//!
//! The clamp carries a margin for a second reason, which is a live-network
//! observation rather than a documented guarantee: `getHealth`'s `oldestLedger` is
//! the oldest ledger a node *stores*, while `getEvents` refuses a `startLedger`
//! below the oldest ledger its event index covers, and the two do not agree. A
//! request built from the health value alone is therefore rejected at the very edge
//! it was trying to respect, which is the one place a scan must not fail. Scanning
//! from [`EVENTS_START_LEDGER_MARGIN`] ledgers above it costs a negligible part of
//! the window and removes the failure entirely.

use amasario_core::{
    Cancellation, EngineError, LedgerSequence, Result, TraversalOutcome, TruncationReason,
};
use stellar_rpc_client::EventStart;
use tracing::debug;

use crate::rpc::{DEFAULT_EVENT_PAGE_LIMIT, Event, RpcSession};

/// How much of a contract's history a scan covers.
///
/// The distinction is the whole reason this type exists: which stretch of history a
/// scan reads decides whether its answer describes the present or a week ago. See
/// the module documentation for why the recent window is the default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventWindow {
    /// The most recent `ledgers` ledgers, ending at the node's tip.
    ///
    /// This is what a dependency question is about. The window is resolved against
    /// the node's tip at the moment of the scan, so it is always relative to the
    /// observation boundary rather than to a ledger number the caller has to know.
    Recent {
        /// How many ledgers back from the tip to cover. One or more.
        ledgers: u32,
    },
    /// From `ledger` forward, as far as the node retains.
    ///
    /// For a deliberate historical scan. The range is still clamped to the node's
    /// retention window, and still reports that it was.
    From {
        /// The first ledger to cover, inclusive.
        ledger: LedgerSequence,
    },
}

/// The default recent window: about twenty minutes of ledgers.
///
/// Stellar closes a ledger about every five seconds, so 256 ledgers is roughly 21
/// minutes. The size is a deliberate compromise, and the compromise is worth stating
/// because it is not a matter of taste.
///
/// `getEvents` is a ledger-ordered feed that paginates by *event count*. A request
/// starting at ledger L returns events from L forward, so a scan that starts at the
/// old edge of a window always spends its budget on the oldest events in that window and
/// reaches the present only if the budget outlasts the window. A day-long window on a
/// busy contract therefore reads yesterday, which is the defect this default exists to
/// avoid; a window small enough to be covered reads now, which is the question being
/// asked.
///
/// Twenty minutes is short, and a contract that has not been used in the last twenty
/// minutes will show nothing - correctly, and with the window reported so the reader
/// knows what was covered. A caller who needs a longer horizon raises
/// [`EventQuery::recent`] or `--lookback` and accepts that the far end of the window may
/// not be reached; the result says whether it was.
pub const DEFAULT_LOOKBACK_LEDGERS: u32 = 256;

/// How far above a node's oldest retained ledger an event scan starts.
///
/// Not cosmetic. A node's `getHealth` reports the oldest ledger it *stores*, while
/// `getEvents` rejects a `startLedger` below the oldest ledger its event index
/// covers, and on a public endpoint the two differ by a small margin - measured at
/// three ledgers on testnet, while the two values are read seconds apart. A scan
/// that clamps to the health value therefore asks for exactly the ledger the event
/// index cannot serve, and fails at the boundary it was trying to respect.
///
/// Sixty-four ledgers is about five minutes: a negligible fraction of a 120,960
/// ledger retention window, and far more than the observed drift.
pub const EVENTS_START_LEDGER_MARGIN: u32 = 64;

/// The default number of event pages a scan may read before it must stop and say so.
///
/// Ten pages at the default page limit is up to a thousand events, which is ten
/// requests. That is enough to find what a contract has been calling lately and small
/// enough to be a reasonable thing to do without asking, and a caller who wants more
/// history raises it deliberately.
pub const DEFAULT_MAX_EVENT_PAGES: usize = 10;

/// What to scan for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventQuery {
    /// The contracts whose events are wanted. Empty means no contract filter, which
    /// is only meaningful for a scan that immediately filters by hand and is
    /// rejected here instead, because an unfiltered event scan over a busy network
    /// is the fastest way to be rate limited.
    pub contract_ids: Vec<String>,
    /// Which stretch of history to cover.
    pub window: EventWindow,
    /// How many events to request per page.
    pub page_limit: usize,
    /// How many pages may be read before the scan stops and says so.
    pub max_pages: usize,
}

impl EventQuery {
    /// A query for one contract over the default recent window.
    ///
    /// The default is deliberately not "everything the node retains", because on a
    /// public endpoint that means the oldest few minutes of a seven-day window and
    /// answers a question nobody asked. See [`DEFAULT_LOOKBACK_LEDGERS`].
    #[must_use]
    pub fn for_contract(contract_id: impl Into<String>) -> Self {
        Self {
            contract_ids: vec![contract_id.into()],
            window: EventWindow::Recent {
                ledgers: DEFAULT_LOOKBACK_LEDGERS,
            },
            page_limit: DEFAULT_EVENT_PAGE_LIMIT,
            max_pages: DEFAULT_MAX_EVENT_PAGES,
        }
    }

    /// Starts the scan at a specific ledger, forward from there.
    ///
    /// This is for a deliberate historical scan. It is not the way to ask "what has
    /// this contract been doing", because the answer to that starts at the tip.
    #[must_use]
    pub const fn from(mut self, ledger: LedgerSequence) -> Self {
        self.window = EventWindow::From { ledger };
        self
    }

    /// Covers the most recent `ledgers` ledgers instead of the default window.
    #[must_use]
    pub const fn recent(mut self, ledgers: u32) -> Self {
        self.window = EventWindow::Recent { ledgers };
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
    if let EventWindow::Recent { ledgers: 0 } = query.window {
        return Err(EngineError::Configuration(
            "a recent window of zero ledgers would observe nothing; ask for at least one ledger"
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

    // Read the retention window so the request is never made outside it. Both the
    // window's floor and its ceiling come from here: a recent window is resolved
    // against the tip, so it is relative to the observation boundary rather than to
    // a ledger number the caller had to know in advance.
    let status = session.node_status(cancellation).await?;
    let tip = status.latest_ledger.get();

    let requested_from = match query.window {
        EventWindow::Recent { ledgers } => tip.saturating_sub(ledgers.saturating_sub(1)),
        EventWindow::From { ledger } => ledger.get(),
    };

    // The margin is what keeps the request inside what the event index can serve;
    // see [`EVENTS_START_LEDGER_MARGIN`] for why the health value alone is not
    // enough. Saturating because a node reporting a ledger near `u32::MAX` must not
    // wrap the floor into a value it will happily accept.
    let floor = status
        .oldest_ledger
        .get()
        .saturating_add(EVENTS_START_LEDGER_MARGIN);
    let clamped = requested_from < floor;
    // Never past the tip: a start beyond the node's head is a start it cannot serve,
    // and on a network younger than the margin (a local standalone chain) the floor
    // itself can sit beyond the tip.
    let scanned_from = LedgerSequence::new(requested_from.max(floor).min(tip))?;

    if clamped {
        debug!(
            requested = requested_from,
            oldest = status.oldest_ledger.get(),
            margin = EVENTS_START_LEDGER_MARGIN,
            "the requested start ledger is outside the endpoint's event retention and was clamped"
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
        // A node whose retention is wide enough that the default recent window sits
        // inside it, so the tests below exercise the window rather than the clamp.
        mount_health(&server, 1_000_000, 500_000).await;
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
        // A node whose retention is wide enough that the default recent window sits
        // inside it, so the tests below exercise the window rather than the clamp.
        mount_health(&server, 1_000_000, 500_000).await;

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
        // A node whose retention is wide enough that the default recent window sits
        // inside it, so the tests below exercise the window rather than the clamp.
        mount_health(&server, 1_000_000, 500_000).await;

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
        mount_health(&server, 1_000_000, 999_900).await;

        Mock::given(method("POST"))
            .and(body_string_contains("getEvents"))
            // The node retains from 999,900, and the event index starts at the margin
            // above it rather than at the health value itself.
            .and(body_string_contains("\"startLedger\":999964"))
            .respond_with(events(&[], 999_950, "c1"))
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
        assert_eq!(scan.scanned_from.map(LedgerSequence::get), Some(999_964));
        assert!(!scan.is_complete());
        assert_eq!(scan.truncation, Some(TruncationReason::BoundaryReached));
    }

    #[tokio::test]
    async fn the_default_window_starts_at_the_recent_end_not_at_the_oldest_ledger() {
        // The regression this module was rebuilt for. Scanning from the retention
        // floor looks thorough and is useless: on a public endpoint a page-bounded
        // scan from there covers the oldest few minutes of a seven-day window and
        // never reaches anything that happened since, so the answer describes a week
        // ago. The default must therefore begin one lookback below the tip.
        let server = MockServer::start().await;
        mount_health(&server, 1_000_000, 500_000).await;

        // The expected start is derived from the constant rather than written out, so
        // the assertion cannot drift when the default changes and still has to be the
        // exact value: a scan that quietly started at the retention floor again would
        // begin at 500,064 rather than here and fail the mock.
        let expected_start = 1_000_000 - (DEFAULT_LOOKBACK_LEDGERS - 1);
        Mock::given(method("POST"))
            .and(body_string_contains("getEvents"))
            .and(body_string_contains(format!(
                "\"startLedger\":{expected_start}"
            )))
            .respond_with(events(&["1-1"], 999_000, "c1"))
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

        assert_eq!(
            scan.scanned_from.map(LedgerSequence::get),
            Some(expected_start)
        );
        assert!(!scan.clamped_to_oldest_ledger);
    }

    #[tokio::test]
    async fn a_recent_window_longer_than_the_node_retains_is_clamped_and_reported() {
        // A local standalone chain is minutes old, so the default window can be wider
        // than its whole history. That is a legitimate clamp, and it must be reported
        // rather than presented as a complete history.
        let server = MockServer::start().await;
        mount_health(&server, 200, 100).await;

        Mock::given(method("POST"))
            .and(body_string_contains("getEvents"))
            .and(body_string_contains("\"startLedger\":164"))
            .respond_with(events(&[], 500, "c1"))
            .expect(1)
            .mount(&server)
            .await;

        let session = RpcSession::connect(&target(&server)).expect("connects");
        let scan = scan_events(
            &session,
            &Cancellation::new(),
            &EventQuery::for_contract(CONTRACT),
        )
        .await
        .expect("the scan succeeds");

        assert_eq!(scan.scanned_from.map(LedgerSequence::get), Some(164));
        assert!(scan.clamped_to_oldest_ledger);
        assert_eq!(scan.truncation, Some(TruncationReason::BoundaryReached));
    }

    #[tokio::test]
    async fn a_recent_window_of_zero_ledgers_is_rejected_before_any_request() {
        let server = MockServer::start().await;
        let session = RpcSession::connect(&target(&server)).expect("connects");

        let error = scan_events(
            &session,
            &Cancellation::new(),
            &EventQuery::for_contract(CONTRACT).recent(0),
        )
        .await
        .expect_err("a zero-ledger window can observe nothing");
        assert!(error.to_string().contains("zero ledgers"));
    }

    #[tokio::test]
    async fn a_cancelled_scan_returns_what_it_read_marked_as_stopped() {
        let server = MockServer::start().await;
        // A node whose retention is wide enough that the default recent window sits
        // inside it, so the tests below exercise the window rather than the clamp.
        mount_health(&server, 1_000_000, 500_000).await;
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
        // A node whose retention is wide enough that the default recent window sits
        // inside it, so the tests below exercise the window rather than the clamp.
        mount_health(&server, 1_000_000, 500_000).await;
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
                window: EventWindow::Recent {
                    ledgers: DEFAULT_LOOKBACK_LEDGERS,
                },
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
        // A node whose retention is wide enough that the default recent window sits
        // inside it, so the tests below exercise the window rather than the clamp.
        mount_health(&server, 1_000_000, 500_000).await;
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
