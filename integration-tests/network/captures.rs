//! The capture corpus, decoded by the adapters that will decode it live.
//!
//! # What separates this suite from [`network`](crate::recordings)
//!
//! The suite beside this one serves hand-written responses that are *shaped like* the
//! documented Stellar RPC and Horizon responses. It catches a path that moved and a
//! field that was renamed. It cannot catch a field that was never populated, because
//! the person who wrote the document populated it.
//!
//! This suite serves bytes a live testnet endpoint returned, verbatim, through a real
//! socket, into the adapters' own client. Three defects the engine has had could only
//! have been caught here, and each is asserted below:
//!
//! * the node publishes the host's diagnostic events at the top level of the
//!   transaction response and inside the transaction metadata, and leaves the
//!   `diagnosticEventsXdr` nested inside the response's `events` object - the one the
//!   bundled client reads - empty. An adapter that trusted the client's accessor read
//!   no diagnostics from any transaction, recovered no call nesting, and reported every
//!   contract as having no dependencies;
//! * a nested `fn_call` diagnostic carries the *caller* in the event's own
//!   `contract_id` and the callee in its second topic, the reverse of the reading that
//!   suggests itself, so a decoder that read `contract_id` first attributed every call
//!   to the contract that made it;
//! * the node's event index rejects a request at its own floor, which sits above the
//!   floor `getHealth` reports, so a scan measured against `getHealth` alone fails at
//!   the edge of the window instead of observing it.
//!
//! # What is asserted, and what is deliberately not
//!
//! Every assertion below is about what the engine makes of the captured bytes. None is
//! a claim that the analysis is correct, and none is a claim about testnet now: a
//! capture is a recording of what one endpoint answered on one day, which is why the
//! bytes are committed and the tests do not reach the network.

use std::convert::TryInto as _;

use amasario_contract::invocations_from_transaction;
use amasario_core::{Cancellation, EngineConfig, LedgerSequence, TruncationReason};
use amasario_integration_tests::captures::{self, Capture};
use amasario_integration_tests::documents::TESTNET_PASSPHRASE;
use amasario_network::{
    EVENTS_START_LEDGER_MARGIN, EventQuery, NetworkTarget, RpcSession, TransactionObservation,
    scan_events,
};
use serde_json::Value;
use stellar_rpc_client::{GetTransactionResponse, GetTransactionResponseRaw};
use wiremock::matchers::{body_string_contains, method};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

/// An endpoint that serves one captured envelope for one JSON-RPC method.
///
/// The captured `result` member is served unaltered - including the base64 XDR it holds,
/// which is the part that matters - while the envelope's `id` is set to the id the caller
/// sent. That rewrite is not tidying: the client rejects a response whose id is not its
/// own pending call (`request ID=1 is not a pending call`), so a capture served with the
/// id it was recorded under is a capture no adapter would accept. It is the one place
/// this suite departs from the recorded bytes, and it is documented here because the
/// behaviour it works around is worth knowing.
async fn mount(server: &MockServer, capture: &Capture) {
    /// Serves one captured envelope, answering under the id it was asked with.
    struct Responder {
        envelope: Value,
    }

    impl Respond for Responder {
        fn respond(&self, request: &Request) -> ResponseTemplate {
            let mut envelope = self.envelope.clone();
            if let Ok(sent) = serde_json::from_slice::<Value>(&request.body)
                && let Some(id) = sent.get("id")
            {
                envelope["id"] = id.clone();
            }
            ResponseTemplate::new(200).set_body_json(envelope)
        }
    }

    Mock::given(method("POST"))
        .and(body_string_contains(capture.method))
        .respond_with(Responder {
            envelope: capture.value(),
        })
        .mount(server)
        .await;
}

/// A session pointed at a mock endpoint, named as testnet because that is the chain the
/// captures were read from.
fn target(server: &MockServer) -> NetworkTarget {
    NetworkTarget::custom(
        "mock",
        TESTNET_PASSPHRASE,
        &server.uri(),
        None,
        &EngineConfig::default(),
    )
    .expect("a local endpoint is a valid target")
}

/// The observation the engine builds from a captured transaction response.
///
/// The path a live run takes: the raw wire shape is deserialised, the client's own
/// conversion decodes the base64 XDR, and the engine's constructor maps that into the
/// observation every later stage reads. A test that constructed an observation directly
/// would skip the two steps where the defects were found.
fn observation(capture: &Capture, hash: &str) -> TransactionObservation {
    let raw: GetTransactionResponseRaw = serde_json::from_value(capture.result())
        .expect("the captured result is the wire shape the client declares");

    let response: GetTransactionResponse = raw
        .try_into()
        .expect("the captured base64 XDR decodes with the client's own reader");

    let hash = amasario_core::TransactionHash::new(hash.to_owned()).expect("a 32-byte hash");

    TransactionObservation::from_response(response, &hash)
        .expect("the captured response maps into an observation")
}

/// Every capture is committed, and each records the request that produced it.
///
/// The request is part of the fixture rather than a comment because a response without
/// its request is not reproducible: a reader cannot tell what question these bytes are
/// an answer to, and cannot ask it again.
#[test]
fn every_capture_is_committed_and_records_its_request() {
    for capture in captures::all() {
        let value = capture.value();

        assert_eq!(
            value["jsonrpc"],
            "2.0",
            "{} is a JSON-RPC envelope",
            capture.path()
        );
        assert!(
            value.get("result").is_some(),
            "{} must carry the result member, since a capture is a successful response",
            capture.path()
        );
        let request: Value = serde_json::from_str(capture.request).unwrap_or_else(|error| {
            panic!(
                "the request recorded beside {} is not a JSON-RPC envelope ({error}): {}",
                capture.path(),
                capture.request
            )
        });
        assert_eq!(
            request["method"],
            capture.method,
            "the request recorded beside {} must name the method it called",
            capture.path()
        );
        assert!(
            request.get("params").is_some(),
            "the recorded request for {} must carry its parameters, or the response it \
             produced cannot be repeated",
            capture.path()
        );
        assert_eq!(
            request["jsonrpc"], value["jsonrpc"],
            "the request and the response must agree on the protocol version"
        );
        assert_eq!(
            capture.network,
            "testnet",
            "{} must record the network it was read from",
            capture.path()
        );
        assert_eq!(
            capture.captured.len(),
            10,
            "{} must be dated as YYYY-MM-DD, so that a reader knows how stale it is",
            capture.path()
        );
    }
}

/// The floor `getHealth` reports is below the floor the event index serves.
///
/// This is the measurement the margin exists for, taken from the two captures rather
/// than from a comment. On a live testnet endpoint `getEvents` rejected a request at the
/// ledger `getHealth` named as its oldest, so a scan that clamped to the health value
/// asked for a ledger the index would not serve and failed at the edge of the window -
/// the opposite of observing it. The assertion is that the margin covers the gap the
/// two captures show, because if it did not, the clamp would be the bug rather than the
/// fix.
#[test]
fn the_captured_health_floor_sits_below_the_floor_the_event_index_serves() {
    let health = captures::TESTNET_HEALTH.result();
    let events = captures::TESTNET_EVENT_PAGE.result();

    let reported = health["oldestLedger"]
        .as_u64()
        .expect("getHealth reports its oldest ledger");
    let served = events["oldestLedger"]
        .as_u64()
        .expect("the event response reports the index's own oldest ledger");

    assert!(
        served > reported,
        "the captures must show the event index's floor above the health floor, which is \
         the discrepancy the margin exists for: served {served}, reported {reported}"
    );

    let gap = served - reported;
    assert!(
        gap <= u64::from(EVENTS_START_LEDGER_MARGIN),
        "the margin must clear the gap the captures show, or a clamped scan still asks \
         for a ledger the index refuses: gap {gap}, margin {EVENTS_START_LEDGER_MARGIN}"
    );
}

/// The captured event page is read by the event scan, and a full page is not the end.
///
/// The page is full and carries a cursor, which is the endpoint saying that more events
/// exist beyond it. A scan that treated a full page as the end would silently report a
/// contract's first five events as all of them, so the assertion is that it reports
/// itself bounded instead.
#[tokio::test]
async fn the_captured_event_page_is_read_and_a_full_page_is_reported_as_bounded() {
    let server = MockServer::start().await;
    mount(&server, &captures::TESTNET_HEALTH).await;
    mount(&server, &captures::TESTNET_EVENT_PAGE).await;

    let session = RpcSession::connect(&target(&server)).expect("the session connects");
    let scan = scan_events(
        &session,
        &Cancellation::new(),
        &EventQuery::for_contract(captures::TESTNET_SUBJECT_CONTRACT)
            .with_page_limit(5)
            .with_max_pages(1),
    )
    .await
    .expect("the captured page is readable");

    assert_eq!(
        scan.events.len(),
        5,
        "the capture holds five events for the subject contract"
    );
    assert_eq!(scan.pages_read, 1);

    for event in &scan.events {
        assert_eq!(
            event.contract_id,
            captures::TESTNET_SUBJECT_CONTRACT,
            "every event in the capture was emitted by the subject contract"
        );
        assert_eq!(
            event.tx_hash.as_deref(),
            Some(captures::TESTNET_NESTED_TRANSACTION),
            "the captured page and the captured transaction must describe the same \
             activity, or the corpus describes two unrelated moments"
        );
        assert_eq!(
            event.ledger,
            captures::TESTNET_NESTED_LEDGER,
            "the events are from the ledger the captured transaction was included in"
        );
    }

    assert!(
        !scan.is_complete(),
        "a full page with a cursor is not the end of a scan, and reporting it as complete \
         is how a bounded scan becomes a wrong answer"
    );
    assert_eq!(
        scan.truncation,
        Some(TruncationReason::MaxNodesReached),
        "the page bound was reached with a cursor still in hand"
    );
}

/// A start below the captured floor is clamped to it, and the request shows the clamp.
///
/// The clamp is asserted on the bytes the adapter actually sent, not only on the scan's
/// own report: a scan that claimed to clamp and then requested the unclamped ledger
/// would be lying about what it observed while failing at the endpoint.
#[tokio::test]
async fn a_start_below_the_captured_floor_is_clamped_and_the_request_follows() {
    let server = MockServer::start().await;
    mount(&server, &captures::TESTNET_HEALTH).await;
    mount(&server, &captures::TESTNET_EVENT_PAGE).await;

    let reported = captures::TESTNET_HEALTH.result()["oldestLedger"]
        .as_u64()
        .expect("a reported floor");
    let below = u32::try_from(reported - 1_000).expect("a ledger below the floor");
    let expected_from = u32::try_from(reported)
        .expect("a ledger")
        .saturating_add(EVENTS_START_LEDGER_MARGIN);

    let session = RpcSession::connect(&target(&server)).expect("the session connects");
    let scan = scan_events(
        &session,
        &Cancellation::new(),
        &EventQuery::for_contract(captures::TESTNET_SUBJECT_CONTRACT)
            .from(LedgerSequence::new(below).expect("a valid ledger"))
            .with_page_limit(5)
            .with_max_pages(1),
    )
    .await
    .expect("the scan succeeds against the captured window");

    assert!(
        scan.clamped_to_oldest_ledger,
        "a start below the endpoint's floor is part of the range the scan could not cover, \
         and it must say so rather than presenting a partial window as the whole of it"
    );
    assert_eq!(
        scan.scanned_from.map(LedgerSequence::get),
        Some(expected_from),
        "the scan must report the ledger it actually started from, after clamping"
    );

    let requests = server
        .received_requests()
        .await
        .expect("the mock recorded its requests");
    let event_request = requests
        .iter()
        .find(|request| {
            let body = String::from_utf8_lossy(&request.body);
            body.contains("getEvents")
        })
        .expect("the scan asked the index for events");

    let body: Value =
        serde_json::from_slice(&event_request.body).expect("the request is a JSON-RPC envelope");
    assert_eq!(
        body["params"]["startLedger"].as_u64(),
        Some(u64::from(expected_from)),
        "the adapter must ask the index from the clamped ledger, not from the requested one: \
         {}",
        String::from_utf8_lossy(&event_request.body)
    );
}

/// The captured nested call reconciles into the cross-contract edge the engine needed.
///
/// This is the assertion the corpus was collected for. The transaction enters the
/// subject contract, which calls another contract, and the engine must recover that
/// second call from the host's own diagnostics. Everything the flagship analysis does
/// rests on it: with no edge recovered, the dependency set is empty by construction, and
/// a live testnet contract with a thousand events in the window reported nothing at all.
#[test]
fn the_captured_nested_call_yields_a_cross_contract_edge() {
    let capture = &captures::TESTNET_NESTED_CALL;
    let observation = observation(capture, captures::TESTNET_NESTED_TRANSACTION);

    assert!(
        observation.successful,
        "the capture is a successful transaction, and its outcome is what lets a call \
         establish runtime use"
    );
    assert_eq!(
        observation.ledger.map(LedgerSequence::get),
        Some(captures::TESTNET_NESTED_LEDGER)
    );
    assert!(
        !observation.diagnostic_events.is_empty(),
        "the host's diagnostics are the only record of the nested call, and the endpoint \
         publishes them in the transaction's metadata rather than in the accessor the \
         client reads - an empty list here is the defect, not an absence of nesting"
    );

    let invocations =
        invocations_from_transaction(&observation).expect("the observation is well formed");

    assert!(
        invocations.diagnostics_available,
        "the scan must record that diagnostics were available, because their absence and \
         an absence of calls are different claims"
    );
    assert!(
        invocations.nesting_recovered,
        "the nested call must be recovered from the captured diagnostics"
    );

    let edges = invocations.cross_contract_edges();
    assert!(
        !edges.is_empty(),
        "the capture contains a nested call, so the engine must recover at least one \
         cross-contract edge from it; recovering none is the defect this capture is for"
    );

    let subject = captures::TESTNET_SUBJECT_CONTRACT;
    let made_by_the_subject: Vec<_> = edges
        .iter()
        .filter(|edge| edge.caller.as_deref() == Some(subject))
        .collect();

    assert!(
        !made_by_the_subject.is_empty(),
        "the subject contract is the caller of the nested call in the capture, so at least \
         one edge must name it as the caller: {:?}",
        edges
            .iter()
            .map(|edge| (edge.caller.as_deref(), edge.callee.as_str()))
            .collect::<Vec<_>>()
    );

    for edge in &edges {
        assert!(
            edge.caller.as_deref() != Some(edge.callee.as_str()),
            "a self-call is not a cross-contract edge: {:?}",
            edge
        );
        assert_eq!(
            edge.successful,
            Some(true),
            "each recovered call inherits the transaction's outcome, and an invocation \
             that carries no outcome is refused by the dependency rules however much \
             evidence stands behind it"
        );
    }
}

/// The captured direct call yields no cross-contract edge.
///
/// The control for the test above, and the reason it is worth having both: a decoder
/// that manufactured a nested call from any transaction would pass the nested assertion
/// and be wrong about every contract that calls one thing and nothing else.
#[test]
fn the_captured_direct_call_yields_no_cross_contract_edge() {
    let capture = &captures::TESTNET_DIRECT_CALL;
    let observation = observation(capture, captures::TESTNET_DIRECT_TRANSACTION);

    assert!(
        observation.successful,
        "the capture is a successful transaction"
    );
    assert_eq!(
        observation.ledger.map(LedgerSequence::get),
        Some(captures::TESTNET_DIRECT_LEDGER)
    );

    let invocations =
        invocations_from_transaction(&observation).expect("the observation is well formed");

    assert!(
        !invocations.nesting_recovered,
        "the capture records no nested call, so recovering one would be a nesting the \
         evidence does not contain: {:?}",
        invocations
            .cross_contract_edges()
            .iter()
            .map(|edge| (edge.caller.as_deref(), edge.callee.as_str()))
            .collect::<Vec<_>>()
    );
    assert!(
        invocations.cross_contract_edges().is_empty(),
        "no call in the capture was made by a contract, so no cross-contract edge exists \
         to recover"
    );

    // The top-level call is still a call. This is the half that a decoder which simply
    // gave up when there was no nesting would get wrong, and it is why the control case
    // is worth having: "no nesting" and "no call" are different answers.
    assert_eq!(
        invocations.invocations.len(),
        1,
        "the capture holds one `InvokeContract` operation, so exactly one invocation is \
         expected: {:?}",
        invocations.invocations
    );
    let invocation = &invocations.invocations[0];
    assert_eq!(
        invocation.callee,
        captures::TESTNET_DIRECT_CONTRACT,
        "the operation named one contract, and it must be the contract recorded"
    );
    assert_eq!(
        invocation.caller, None,
        "a top-level call has no calling contract: it was made by an account, and an \
         account is not a contract"
    );
    assert_eq!(
        invocation.successful,
        Some(true),
        "the invocation inherits the transaction's recorded outcome"
    );
}
